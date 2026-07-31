use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

/// Stable launch-profile identifier used by settings, actions, and workspaces.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ProfileId(String);

impl ProfileId {
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidProfileId> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 128
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            });
        if !valid {
            return Err(InvalidProfileId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProfileId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ProfileId {
    type Err = InvalidProfileId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for ProfileId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidProfileId;

impl fmt::Display for InvalidProfileId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .write_str("profile ID must use 1 to 128 ASCII letters, numbers, '-', '_', '.', or ':'")
    }
}

impl std::error::Error for InvalidProfileId {}

/// Resolved process inputs used to launch one terminal pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchSpec {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub environment: BTreeMap<OsString, OsString>,
}

impl LaunchSpec {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            arguments: Vec::new(),
            working_directory: None,
            environment: BTreeMap::new(),
        }
    }

    pub fn with_arguments(
        mut self,
        arguments: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Self {
        self.arguments = arguments.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_working_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(directory.into());
        self
    }

    pub fn with_environment(
        mut self,
        environment: impl IntoIterator<Item = (impl Into<OsString>, impl Into<OsString>)>,
    ) -> Self {
        self.environment = environment
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        self
    }

    pub fn default_shell() -> Self {
        Self::new(default_shell_executable())
    }

    pub fn executable(&self) -> &OsStr {
        self.executable.as_os_str()
    }
}

#[cfg(windows)]
fn default_shell_executable() -> OsString {
    OsString::from("pwsh.exe")
}

#[cfg(unix)]
fn default_shell_executable() -> OsString {
    std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/sh"))
}

#[cfg(not(any(windows, unix)))]
fn default_shell_executable() -> OsString {
    OsString::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_stable_profile_ids() {
        assert_eq!(
            ProfileId::new("wsl:Ubuntu-24.04").unwrap().as_str(),
            "wsl:Ubuntu-24.04"
        );
        assert!(ProfileId::new("").is_err());
        assert!(ProfileId::new("PowerShell 7").is_err());
        assert!(ProfileId::new("é").is_err());
    }

    #[test]
    fn rejects_invalid_ids_during_deserialization() {
        assert!(serde_json::from_str::<ProfileId>(r#""PowerShell 7""#).is_err());
    }

    #[test]
    fn launch_spec_keeps_process_inputs_separate() {
        let spec = LaunchSpec::new("pwsh.exe")
            .with_arguments(["-NoLogo"])
            .with_working_directory("C:\\work")
            .with_environment([("TERM", "xterm-256color")]);

        assert_eq!(spec.executable, PathBuf::from("pwsh.exe"));
        assert_eq!(spec.arguments, [OsString::from("-NoLogo")]);
        assert_eq!(spec.working_directory, Some(PathBuf::from("C:\\work")));
        assert_eq!(
            spec.environment.get(OsStr::new("TERM")),
            Some(&OsString::from("xterm-256color"))
        );
    }
}
