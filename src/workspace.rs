//! Serializable workspace state and local workspace files.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Deserializer, Serialize};

use crate::profile::ProfileId;
use crate::settings::settings_path;
use crate::split::{PaneId, SplitNode};

const WORKSPACE_VERSION: u32 = 1;
const MAX_WORKSPACE_TABS: usize = 9;
static NEXT_TAB_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct WorkspaceId(String);

impl WorkspaceId {
    pub fn new(value: impl Into<String>) -> Result<Self, WorkspaceError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 96
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
        if !valid {
            return Err(WorkspaceError::new(
                "workspace ID must use 1 to 96 ASCII letters, numbers, '-', '_', or '.'",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn default_workspace() -> Self {
        Self("default".to_string())
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for WorkspaceId {
    type Err = WorkspaceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for WorkspaceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TabId(u64);

impl TabId {
    pub fn fresh() -> Self {
        Self(NEXT_TAB_ID.fetch_add(1, Ordering::Relaxed))
    }

    pub(crate) fn reserve_after(ids: impl IntoIterator<Item = Self>) {
        let next = ids
            .into_iter()
            .map(|id| id.0)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        NEXT_TAB_ID.fetch_max(next, Ordering::Relaxed);
    }

    #[cfg(test)]
    const fn test(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceLayout {
    pub version: u32,
    pub id: WorkspaceId,
    pub active_tab_id: TabId,
    pub tabs: Vec<WorkspaceTab>,
}

impl WorkspaceLayout {
    pub fn new(id: WorkspaceId, active_tab_id: TabId, tabs: Vec<WorkspaceTab>) -> Self {
        Self {
            version: WORKSPACE_VERSION,
            id,
            active_tab_id,
            tabs,
        }
    }

    pub fn validate(&self) -> Result<(), WorkspaceError> {
        if self.version != WORKSPACE_VERSION {
            return Err(WorkspaceError::new(format!(
                "workspace version {} is unsupported",
                self.version
            )));
        }
        if self.tabs.is_empty() || self.tabs.len() > MAX_WORKSPACE_TABS {
            return Err(WorkspaceError::new(
                "workspace must contain from 1 through 9 tabs",
            ));
        }
        let mut tab_ids = BTreeSet::new();
        for tab in &self.tabs {
            if !tab_ids.insert(tab.id) {
                return Err(WorkspaceError::new("workspace contains a duplicate tab ID"));
            }
            tab.validate()?;
        }
        if !tab_ids.contains(&self.active_tab_id) {
            return Err(WorkspaceError::new("workspace active tab does not exist"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTab {
    pub id: TabId,
    pub title: String,
    pub active_pane_id: PaneId,
    pub root: SplitNode,
    pub panes: BTreeMap<PaneId, WorkspacePane>,
}

impl WorkspaceTab {
    fn validate(&self) -> Result<(), WorkspaceError> {
        if self.title.trim().is_empty() {
            return Err(WorkspaceError::new("workspace tab title must not be empty"));
        }
        let leaf_ids = pane_ids(&self.root);
        let unique_leaf_ids = leaf_ids.iter().copied().collect::<BTreeSet<_>>();
        let pane_ids = self.panes.keys().copied().collect::<BTreeSet<_>>();
        if unique_leaf_ids.len() != leaf_ids.len() || unique_leaf_ids != pane_ids {
            return Err(WorkspaceError::new(
                "workspace pane records must match split-tree leaves",
            ));
        }
        if !pane_ids.contains(&self.active_pane_id) {
            return Err(WorkspaceError::new("workspace active pane does not exist"));
        }
        validate_ratios(&self.root)?;
        for pane in self.panes.values() {
            if let Some(directory) = &pane.working_directory
                && !trusted_working_directory(directory)
            {
                return Err(WorkspaceError::new(
                    "workspace working directories must be trusted local absolute paths",
                ));
            }
        }
        Ok(())
    }
}

pub fn trusted_working_directory(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};

        matches!(
            path.components().next(),
            Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
        )
    }
    #[cfg(not(windows))]
    {
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePane {
    pub profile_id: ProfileId,
    pub working_directory: Option<PathBuf>,
}

pub struct WorkspaceStore {
    directory: PathBuf,
}

impl WorkspaceStore {
    pub fn open_default() -> Self {
        let directory = settings_path()
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("workspaces");
        Self { directory }
    }

    pub fn list(&self) -> Result<Vec<WorkspaceId>, WorkspaceError> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(WorkspaceError::io("list workspaces", error)),
        };
        let mut ids = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| WorkspaceError::io("read workspace entry", error))?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if let Ok(id) = WorkspaceId::new(stem) {
                ids.push(id);
            }
        }
        ids.sort();
        Ok(ids)
    }

    pub fn save(&self, layout: &WorkspaceLayout) -> Result<PathBuf, WorkspaceError> {
        layout.validate()?;
        fs::create_dir_all(&self.directory)
            .map_err(|error| WorkspaceError::io("create workspace directory", error))?;
        let path = self.path(&layout.id);
        let json = serde_json::to_vec_pretty(layout)
            .map_err(|error| WorkspaceError::new(format!("serialize workspace: {error}")))?;
        fs::write(&path, json).map_err(|error| WorkspaceError::io("write workspace", error))?;
        Ok(path)
    }

    pub fn load(&self, id: &WorkspaceId) -> Result<WorkspaceLayout, WorkspaceError> {
        let path = self.path(id);
        let json = fs::read(&path).map_err(|error| WorkspaceError::io("read workspace", error))?;
        let layout: WorkspaceLayout = serde_json::from_slice(&json)
            .map_err(|error| WorkspaceError::new(format!("parse workspace: {error}")))?;
        if layout.id != *id {
            return Err(WorkspaceError::new(
                "workspace file ID does not match its file name",
            ));
        }
        layout.validate()?;
        Ok(layout)
    }

    fn path(&self, id: &WorkspaceId) -> PathBuf {
        self.directory.join(format!("{id}.json"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceError {
    message: String,
}

impl WorkspaceError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn io(operation: &str, error: std::io::Error) -> Self {
        Self::new(format!("{operation}: {error}"))
    }
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for WorkspaceError {}

fn pane_ids(node: &SplitNode) -> Vec<PaneId> {
    match node {
        SplitNode::Leaf { pane_id } => vec![*pane_id],
        SplitNode::Branch { first, second, .. } => {
            let mut ids = pane_ids(first);
            ids.extend(pane_ids(second));
            ids
        }
    }
}

fn validate_ratios(node: &SplitNode) -> Result<(), WorkspaceError> {
    let SplitNode::Branch {
        ratio,
        first,
        second,
        ..
    } = node
    else {
        return Ok(());
    };
    if !ratio.is_finite() || !(0.05..=0.95).contains(ratio) {
        return Err(WorkspaceError::new(
            "workspace split ratios must be from 0.05 through 0.95",
        ));
    }
    validate_ratios(first)?;
    validate_ratios(second)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::split::SplitAxis;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    static TEMP_DIRECTORY_NUMBER: AtomicUsize = AtomicUsize::new(0);

    fn layout() -> WorkspaceLayout {
        let first = PaneId::test(1);
        let second = PaneId::test(2);
        WorkspaceLayout::new(
            WorkspaceId::new("work").unwrap(),
            TabId::test(1),
            vec![WorkspaceTab {
                id: TabId::test(1),
                title: "Work".to_string(),
                active_pane_id: second,
                root: SplitNode::Branch {
                    axis: SplitAxis::Horizontal,
                    ratio: 0.4,
                    first: Box::new(SplitNode::Leaf { pane_id: first }),
                    second: Box::new(SplitNode::Leaf { pane_id: second }),
                },
                panes: BTreeMap::from([
                    (
                        first,
                        WorkspacePane {
                            profile_id: ProfileId::new("powershell").unwrap(),
                            working_directory: None,
                        },
                    ),
                    (
                        second,
                        WorkspacePane {
                            profile_id: ProfileId::new("git-bash").unwrap(),
                            working_directory: None,
                        },
                    ),
                ]),
            }],
        )
    }

    #[test]
    fn workspace_round_trips_without_runtime_objects() {
        let layout = layout();
        let json = serde_json::to_string_pretty(&layout).unwrap();
        let restored: WorkspaceLayout = serde_json::from_str(&json).unwrap();

        assert_eq!(restored, layout);
        assert!(!json.contains("Entity"));
        assert!(!json.contains("process"));
        restored.validate().unwrap();
    }

    #[test]
    fn rejects_mismatched_pane_records_and_invalid_ratios() {
        let mut layout = layout();
        layout.tabs[0].panes.remove(&PaneId::test(2));
        assert!(layout.validate().is_err());

        let mut layout = self::layout();
        let SplitNode::Branch { ratio, .. } = &mut layout.tabs[0].root else {
            panic!("test root is a branch");
        };
        *ratio = f32::NAN;
        assert!(layout.validate().is_err());
    }

    #[test]
    fn workspace_ids_cannot_escape_the_workspace_directory() {
        assert!(WorkspaceId::new("../outside").is_err());
        assert!(WorkspaceId::new("team-a").is_ok());
    }

    #[test]
    fn workspace_store_saves_and_loads_validated_layouts() {
        let number = TEMP_DIRECTORY_NUMBER.fetch_add(1, AtomicOrdering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "mightty-workspace-test-{}-{number}",
            std::process::id()
        ));
        let store = WorkspaceStore {
            directory: directory.clone(),
        };
        let layout = layout();

        store.save(&layout).unwrap();
        assert_eq!(
            store.list().unwrap().as_slice(),
            std::slice::from_ref(&layout.id)
        );
        assert_eq!(store.load(&layout.id).unwrap(), layout);

        fs::remove_dir_all(directory).unwrap();
    }
}
