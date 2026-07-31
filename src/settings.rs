//! Typed settings, profile discovery, validation, and safe reload.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::action::{ActionBinding, default_action_bindings, normalize_chord};
use crate::profile::{LaunchSpec, ProfileId};
use crate::widget::{CursorStyle, TerminalClipboardPolicy, TerminalConfig, TerminalTheme};

const SETTINGS_FILE_NAME: &str = "settings.json";
const PORTABLE_MARKER_FILE_NAME: &str = "mightty.portable";
const MAX_SCROLLBACK: usize = 10_000_000;

/// User-editable settings. Missing fields use the application defaults.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UserSettings {
    pub app: AppSettings,
    pub terminal: TerminalSettings,
    pub profiles: Vec<LaunchProfile>,
    pub default_profile: Option<ProfileId>,
    pub key_bindings: Vec<ActionBinding>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppSettings {
    pub sidebar_visible: bool,
    pub bell_notifications: bool,
    pub quick_terminal: QuickTerminalSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            sidebar_visible: true,
            bell_notifications: true,
            quick_terminal: QuickTerminalSettings::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct QuickTerminalSettings {
    pub enabled: bool,
    pub hotkey: String,
    pub width_ratio: f32,
    pub height_ratio: f32,
    pub hide_on_focus_loss: bool,
}

impl Default for QuickTerminalSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            hotkey: "ctrl-`".to_string(),
            width_ratio: 0.8,
            height_ratio: 0.45,
            hide_on_focus_loss: true,
        }
    }
}

/// Terminal settings remain optional until they resolve over `TerminalConfig`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalSettings {
    pub initial_rows: Option<u16>,
    pub initial_cols: Option<u16>,
    pub scrollback: Option<usize>,
    pub cursor_style: Option<CursorStyle>,
    pub cursor_blink: Option<bool>,
    pub blink_interval_ms: Option<u64>,
    pub terminal_clipboard_policy: Option<TerminalClipboardPolicy>,
    pub font_family: Option<String>,
    pub font_size_px: Option<f32>,
    pub theme: ThemeSettings,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeSettings {
    pub foreground: Option<String>,
    pub background: Option<String>,
    pub cursor: Option<String>,
    pub selection: Option<String>,
    pub palette: Option<Vec<String>>,
}

/// One discovered or user-defined terminal launch profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchProfile {
    pub id: ProfileId,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub executable: Option<PathBuf>,
    #[serde(default)]
    pub arguments: Option<Vec<String>>,
    #[serde(default)]
    pub starting_directory: Option<PathBuf>,
    #[serde(default, deserialize_with = "deserialize_environment")]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub icon: Option<PathBuf>,
}

impl LaunchProfile {
    fn complete(id: &str, label: impl Into<String>, executable: impl Into<PathBuf>) -> Self {
        Self {
            id: ProfileId::new(id).expect("built-in profile ID is valid"),
            label: Some(label.into()),
            executable: Some(executable.into()),
            arguments: Some(Vec::new()),
            starting_directory: None,
            environment: BTreeMap::new(),
            icon: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedLaunchProfile {
    pub id: ProfileId,
    pub label: String,
    pub launch: LaunchSpec,
    pub icon: Option<PathBuf>,
}

/// Immutable settings used by the running application.
#[derive(Clone, Debug)]
pub struct ResolvedSettings {
    pub app: AppSettings,
    pub terminal: TerminalConfig,
    pub profiles: BTreeMap<ProfileId, ResolvedLaunchProfile>,
    pub default_profile: ProfileId,
    pub key_bindings: Vec<ActionBinding>,
}

impl ResolvedSettings {
    pub fn terminal_config(
        &self,
        profile_id: Option<&ProfileId>,
    ) -> Result<TerminalConfig, SettingsError> {
        let profile_id = profile_id.unwrap_or(&self.default_profile);
        let profile = self
            .profiles
            .get(profile_id)
            .ok_or_else(|| SettingsError::new(format!("profile '{profile_id}' does not exist")))?;
        let mut config = self.terminal.clone();
        config.launch = profile.launch.clone();
        Ok(config)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsDiagnostic {
    pub path: PathBuf,
    pub message: String,
}

impl std::fmt::Display for SettingsDiagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}: {}",
            self.path.to_string_lossy(),
            self.message
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsError {
    message: String,
}

impl SettingsError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SettingsError {}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FileObservation {
    Missing,
    Present(Vec<u8>),
}

/// Owns the last valid settings and rejects invalid reloads atomically.
pub struct SettingsStore {
    path: PathBuf,
    current: ResolvedSettings,
    diagnostic: Option<SettingsDiagnostic>,
    last_observation: FileObservation,
    last_discovered: Vec<LaunchProfile>,
    generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReloadOutcome {
    Unchanged,
    Applied { generation: u64 },
    Rejected(SettingsDiagnostic),
}

impl SettingsStore {
    pub fn open_default() -> Self {
        Self::open(settings_path())
    }

    pub fn open(path: PathBuf) -> Self {
        let discovered = discover_profiles();
        let (observation, resolved) = match read_observation(&path) {
            Ok(observation) => {
                let resolved = resolve_observation(&observation, &path, discovered.clone());
                (observation, resolved)
            }
            Err(error) => (
                FileObservation::Missing,
                Err(SettingsError::new(format!("cannot read settings: {error}"))),
            ),
        };
        let (current, diagnostic) = match resolved {
            Ok(settings) => (settings, None),
            Err(error) => {
                let fallback = resolve_user_settings(
                    UserSettings::default(),
                    discovered.clone(),
                    settings_directory(&path),
                )
                .expect("built-in settings must resolve");
                (
                    fallback,
                    Some(SettingsDiagnostic {
                        path: path.clone(),
                        message: error.to_string(),
                    }),
                )
            }
        };

        Self {
            path,
            current,
            diagnostic,
            last_observation: observation,
            last_discovered: discovered,
            generation: 1,
        }
    }

    pub fn current(&self) -> &ResolvedSettings {
        &self.current
    }

    pub fn diagnostic(&self) -> Option<&SettingsDiagnostic> {
        self.diagnostic.as_ref()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn reload_if_changed(&mut self) -> ReloadOutcome {
        let observation = match read_observation(&self.path) {
            Ok(observation) => observation,
            Err(error) => {
                return self.reject(format!("cannot read settings: {error}"));
            }
        };
        let discovered = discover_profiles();
        if observation == self.last_observation && discovered == self.last_discovered {
            return ReloadOutcome::Unchanged;
        }

        let resolved = resolve_observation(&observation, &self.path, discovered.clone());
        self.last_observation = observation;
        self.last_discovered = discovered;
        match resolved {
            Ok(settings) => {
                self.current = settings;
                self.diagnostic = None;
                self.generation = self.generation.saturating_add(1);
                ReloadOutcome::Applied {
                    generation: self.generation,
                }
            }
            Err(error) => self.reject(error.to_string()),
        }
    }

    fn reject(&mut self, message: String) -> ReloadOutcome {
        let diagnostic = SettingsDiagnostic {
            path: self.path.clone(),
            message,
        };
        self.diagnostic = Some(diagnostic.clone());
        ReloadOutcome::Rejected(diagnostic)
    }
}

pub fn settings_path() -> PathBuf {
    if let Some(path) = std::env::var_os("MIGHTTY_CONFIG_FILE") {
        return PathBuf::from(path);
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        let portable = directory.join(SETTINGS_FILE_NAME);
        if portable.is_file() || directory.join(PORTABLE_MARKER_FILE_NAME).is_file() {
            return portable;
        }
    }

    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("mightty")
            .join(SETTINGS_FILE_NAME)
    }

    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("mightty")
            .join(SETTINGS_FILE_NAME)
    }
}

pub fn parse_and_resolve(
    contents: &[u8],
    directory: &Path,
    discovered: Vec<LaunchProfile>,
) -> Result<ResolvedSettings, SettingsError> {
    let settings: UserSettings = serde_json::from_slice(contents)
        .map_err(|error| SettingsError::new(format!("invalid JSON: {error}")))?;
    resolve_user_settings(settings, discovered, directory)
}

fn resolve_observation(
    observation: &FileObservation,
    path: &Path,
    discovered: Vec<LaunchProfile>,
) -> Result<ResolvedSettings, SettingsError> {
    match observation {
        FileObservation::Missing => resolve_user_settings(
            UserSettings::default(),
            discovered,
            settings_directory(path),
        ),
        FileObservation::Present(contents) => {
            parse_and_resolve(contents, settings_directory(path), discovered)
        }
    }
}

fn resolve_user_settings(
    settings: UserSettings,
    discovered: Vec<LaunchProfile>,
    directory: &Path,
) -> Result<ResolvedSettings, SettingsError> {
    validate_app_settings(&settings.app)?;
    let mut terminal = resolve_terminal_settings(settings.terminal)?;
    let profiles = resolve_profiles(settings.profiles, discovered, directory)?;
    let default_profile = match settings.default_profile {
        Some(profile_id) if profiles.contains_key(&profile_id) => profile_id,
        Some(profile_id) => {
            return Err(SettingsError::new(format!(
                "default profile '{profile_id}' does not exist"
            )));
        }
        None => preferred_default_profile(&profiles),
    };
    let key_bindings = resolve_key_bindings(settings.key_bindings)?;
    terminal.action_bindings = key_bindings.clone();

    Ok(ResolvedSettings {
        app: settings.app,
        terminal,
        profiles,
        default_profile,
        key_bindings,
    })
}

fn resolve_terminal_settings(settings: TerminalSettings) -> Result<TerminalConfig, SettingsError> {
    let mut config = TerminalConfig::default();
    if let Some(rows) = settings.initial_rows {
        if rows == 0 {
            return Err(SettingsError::new("terminal.initial_rows must be positive"));
        }
        config.initial_rows = rows;
    }
    if let Some(cols) = settings.initial_cols {
        if cols == 0 {
            return Err(SettingsError::new("terminal.initial_cols must be positive"));
        }
        config.initial_cols = cols;
    }
    if let Some(scrollback) = settings.scrollback {
        if scrollback > MAX_SCROLLBACK {
            return Err(SettingsError::new(format!(
                "terminal.scrollback must not exceed {MAX_SCROLLBACK}"
            )));
        }
        config.scrollback = scrollback;
    }
    if let Some(cursor_style) = settings.cursor_style {
        config.cursor_style = cursor_style;
    }
    if let Some(cursor_blink) = settings.cursor_blink {
        config.cursor_blink = cursor_blink;
    }
    if let Some(interval) = settings.blink_interval_ms {
        if interval == 0 {
            return Err(SettingsError::new(
                "terminal.blink_interval_ms must be positive",
            ));
        }
        config.blink_interval = Duration::from_millis(interval);
    }
    if let Some(policy) = settings.terminal_clipboard_policy {
        config.terminal_clipboard_policy = policy;
    }
    if let Some(family) = settings.font_family {
        if family.trim().is_empty() {
            return Err(SettingsError::new("terminal.font_family must not be empty"));
        }
        config.font_family = family;
    }
    if let Some(size) = settings.font_size_px {
        if !size.is_finite() || !(6.0..=96.0).contains(&size) {
            return Err(SettingsError::new(
                "terminal.font_size_px must be from 6 through 96",
            ));
        }
        config.font_size_px = size;
    }
    apply_theme(settings.theme, &mut config.theme)?;
    Ok(config)
}

fn apply_theme(settings: ThemeSettings, theme: &mut TerminalTheme) -> Result<(), SettingsError> {
    if let Some(color) = settings.foreground {
        theme.foreground = parse_color("terminal.theme.foreground", &color)?;
    }
    if let Some(color) = settings.background {
        theme.background = parse_color("terminal.theme.background", &color)?;
    }
    if let Some(color) = settings.cursor {
        theme.cursor = parse_color("terminal.theme.cursor", &color)?;
    }
    if let Some(color) = settings.selection {
        theme.selection = parse_color("terminal.theme.selection", &color)?;
    }
    if let Some(palette) = settings.palette {
        if palette.len() != 16 {
            return Err(SettingsError::new(
                "terminal.theme.palette must contain 16 colors",
            ));
        }
        for (index, color) in palette.into_iter().enumerate() {
            theme.palette[index] =
                parse_color(&format!("terminal.theme.palette[{index}]"), &color)?;
        }
    }
    Ok(())
}

fn parse_color(field: &str, value: &str) -> Result<gpui::Rgba, SettingsError> {
    let Some(hex) = value.strip_prefix('#') else {
        return Err(SettingsError::new(format!(
            "{field} must use #RRGGBB format"
        )));
    };
    if hex.len() != 6 {
        return Err(SettingsError::new(format!(
            "{field} must use #RRGGBB format"
        )));
    }
    let color = u32::from_str_radix(hex, 16)
        .map_err(|_| SettingsError::new(format!("{field} must use #RRGGBB format")))?;
    Ok(gpui::rgb(color))
}

fn validate_app_settings(settings: &AppSettings) -> Result<(), SettingsError> {
    let quick = &settings.quick_terminal;
    if !(0.1..=1.0).contains(&quick.width_ratio) || !(0.1..=1.0).contains(&quick.height_ratio) {
        return Err(SettingsError::new(
            "quick-terminal size ratios must be from 0.1 through 1",
        ));
    }
    if quick.enabled {
        normalize_chord(&quick.hotkey).map_err(SettingsError::new)?;
    }
    Ok(())
}

fn resolve_profiles(
    explicit: Vec<LaunchProfile>,
    discovered: Vec<LaunchProfile>,
    directory: &Path,
) -> Result<BTreeMap<ProfileId, ResolvedLaunchProfile>, SettingsError> {
    let mut merged = discovered
        .into_iter()
        .map(|profile| (profile.id.clone(), profile))
        .collect::<BTreeMap<_, _>>();
    let mut explicit_ids = BTreeSet::new();
    for profile in explicit {
        if !explicit_ids.insert(profile.id.clone()) {
            return Err(SettingsError::new(format!(
                "profile ID '{}' is duplicated",
                profile.id
            )));
        }
        let profile = match merged.remove(&profile.id) {
            Some(discovered) => merge_profile(discovered, profile),
            None => profile,
        };
        merged.insert(profile.id.clone(), profile);
    }
    if merged.is_empty() {
        let fallback = fallback_profile();
        merged.insert(fallback.id.clone(), fallback);
    }

    merged
        .into_iter()
        .map(|(id, profile)| {
            let profile = resolve_profile(profile, directory)?;
            Ok((id, profile))
        })
        .collect()
}

fn merge_profile(mut discovered: LaunchProfile, explicit: LaunchProfile) -> LaunchProfile {
    if explicit.label.is_some() {
        discovered.label = explicit.label;
    }
    if explicit.executable.is_some() {
        discovered.executable = explicit.executable;
    }
    if explicit.arguments.is_some() {
        discovered.arguments = explicit.arguments;
    }
    if explicit.starting_directory.is_some() {
        discovered.starting_directory = explicit.starting_directory;
    }
    discovered.environment.extend(explicit.environment);
    if explicit.icon.is_some() {
        discovered.icon = explicit.icon;
    }
    discovered
}

fn resolve_profile(
    profile: LaunchProfile,
    directory: &Path,
) -> Result<ResolvedLaunchProfile, SettingsError> {
    let executable = profile.executable.ok_or_else(|| {
        SettingsError::new(format!("profile '{}' needs an executable", profile.id))
    })?;
    let executable = resolve_executable(&executable, directory).ok_or_else(|| {
        SettingsError::new(format!(
            "profile '{}' executable '{}' does not exist",
            profile.id,
            executable.to_string_lossy()
        ))
    })?;
    let working_directory = profile
        .starting_directory
        .map(|path| resolve_config_path(path, directory));
    if let Some(path) = &working_directory
        && !path.is_dir()
    {
        return Err(SettingsError::new(format!(
            "profile '{}' starting directory '{}' does not exist",
            profile.id,
            path.to_string_lossy()
        )));
    }
    let label = profile
        .label
        .unwrap_or_else(|| profile.id.as_str().to_string());
    if label.trim().is_empty() {
        return Err(SettingsError::new(format!(
            "profile '{}' label must not be empty",
            profile.id
        )));
    }
    for key in profile.environment.keys() {
        validate_environment_name(key)?;
    }
    let launch = LaunchSpec::new(executable)
        .with_arguments(profile.arguments.unwrap_or_default())
        .with_environment(profile.environment);
    let launch = match working_directory {
        Some(directory) => launch.with_working_directory(directory),
        None => launch,
    };

    Ok(ResolvedLaunchProfile {
        id: profile.id,
        label,
        launch,
        icon: profile
            .icon
            .map(|path| resolve_config_path(path, directory)),
    })
}

fn preferred_default_profile(profiles: &BTreeMap<ProfileId, ResolvedLaunchProfile>) -> ProfileId {
    for preferred in ["powershell", "command-prompt", "default-shell"] {
        if let Ok(id) = ProfileId::new(preferred)
            && profiles.contains_key(&id)
        {
            return id;
        }
    }
    profiles
        .keys()
        .next()
        .expect("profile resolution always creates one profile")
        .clone()
}

fn resolve_key_bindings(bindings: Vec<ActionBinding>) -> Result<Vec<ActionBinding>, SettingsError> {
    let mut explicit_chords = BTreeSet::new();
    let mut resolved = default_action_bindings()
        .into_iter()
        .map(|binding| (binding.chord.clone(), binding))
        .collect::<BTreeMap<_, _>>();
    for mut binding in bindings {
        binding.chord = normalize_chord(&binding.chord).map_err(SettingsError::new)?;
        if !explicit_chords.insert(binding.chord.clone()) {
            return Err(SettingsError::new(format!(
                "key chord '{}' is duplicated",
                binding.chord
            )));
        }
        resolved.insert(binding.chord.clone(), binding);
    }
    Ok(resolved.into_values().collect())
}

fn validate_environment_name(name: &str) -> Result<(), SettingsError> {
    if name.is_empty() || name.contains(['=', '\0']) {
        return Err(SettingsError::new(format!(
            "environment name '{name}' is invalid"
        )));
    }
    Ok(())
}

fn deserialize_environment<'de, D>(deserializer: D) -> Result<BTreeMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct EnvironmentVisitor;

    impl<'de> Visitor<'de> for EnvironmentVisitor {
        type Value = BTreeMap<String, String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("an object with unique environment names")
        }

        fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut values = BTreeMap::new();
            let mut names = BTreeSet::new();
            while let Some((name, value)) = map.next_entry::<String, String>()? {
                let normalized = name.to_ascii_lowercase();
                if !names.insert(normalized) {
                    return Err(serde::de::Error::custom(format!(
                        "environment name '{name}' is duplicated"
                    )));
                }
                values.insert(name, value);
            }
            Ok(values)
        }
    }

    deserializer.deserialize_map(EnvironmentVisitor)
}

fn read_observation(path: &Path) -> io::Result<FileObservation> {
    match fs::read(path) {
        Ok(contents) => Ok(FileObservation::Present(contents)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(FileObservation::Missing),
        Err(error) => Err(error),
    }
}

fn settings_directory(path: &Path) -> &Path {
    path.parent().unwrap_or_else(|| Path::new("."))
}

fn resolve_config_path(path: PathBuf, directory: &Path) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        directory.join(path)
    }
}

fn resolve_executable(path: &Path, directory: &Path) -> Option<PathBuf> {
    if path.is_absolute()
        || path
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
    {
        let path = resolve_config_path(path.to_path_buf(), directory);
        return path.is_file().then_some(path);
    }
    find_in_path(path.as_os_str())
}

fn find_in_path(executable: &OsStr) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(executable);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        if candidate.extension().is_none() {
            let candidate = candidate.with_extension("exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn discover_profiles() -> Vec<LaunchProfile> {
    platform_profiles()
}

#[cfg(windows)]
fn platform_profiles() -> Vec<LaunchProfile> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut profiles = Vec::new();
    if let Some(pwsh) = find_in_path(OsStr::new("pwsh.exe")) {
        profiles.push(LaunchProfile::complete("powershell", "PowerShell", pwsh));
    }
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        let cmd = PathBuf::from(system_root).join("System32").join("cmd.exe");
        if cmd.is_file() {
            profiles.push(LaunchProfile::complete(
                "command-prompt",
                "Command Prompt",
                cmd,
            ));
        }
    }
    for git in git_bash_candidates() {
        if git.is_file() {
            let mut profile = LaunchProfile::complete("git-bash", "Git Bash", git);
            profile.arguments = Some(vec!["--login".to_string(), "-i".to_string()]);
            profiles.push(profile);
            break;
        }
    }

    if let Some(wsl) = find_in_path(OsStr::new("wsl.exe"))
        && let Ok(output) = Command::new(&wsl)
            .args(["--list", "--quiet"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        && output.status.success()
    {
        for distribution in decode_wsl_names(&output.stdout) {
            let id = ProfileId::new(format!("wsl:{}", stable_profile_component(&distribution)))
                .expect("encoded WSL profile ID is valid");
            profiles.push(LaunchProfile {
                id,
                label: Some(format!("WSL: {distribution}")),
                executable: Some(wsl.clone()),
                arguments: Some(vec!["--distribution".to_string(), distribution]),
                starting_directory: None,
                environment: BTreeMap::new(),
                icon: None,
            });
        }
    }
    profiles
}

#[cfg(windows)]
fn git_bash_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for variable in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Some(root) = std::env::var_os(variable) {
            let root = PathBuf::from(root);
            candidates.push(if variable == "LOCALAPPDATA" {
                root.join("Programs")
                    .join("Git")
                    .join("bin")
                    .join("bash.exe")
            } else {
                root.join("Git").join("bin").join("bash.exe")
            });
        }
    }
    candidates
}

#[cfg(windows)]
fn decode_wsl_names(bytes: &[u8]) -> Vec<String> {
    let text = if bytes.starts_with(&[0xff, 0xfe])
        || bytes.chunks_exact(2).take(16).any(|pair| pair[1] == 0)
    {
        let words = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .filter(|word| *word != 0xfeff)
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&words)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    text.lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(windows)]
fn stable_profile_component(value: &str) -> String {
    let mut component = String::new();
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        if component.len() < 80 {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
                component.push(char::from(byte));
            } else {
                component.push('_');
            }
        }
    }
    format!("{component}-{hash:016x}")
}

#[cfg(unix)]
fn platform_profiles() -> Vec<LaunchProfile> {
    let shell = std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("/bin/sh"));
    vec![LaunchProfile::complete(
        "default-shell",
        "Default shell",
        shell,
    )]
}

#[cfg(not(any(windows, unix)))]
fn platform_profiles() -> Vec<LaunchProfile> {
    Vec::new()
}

#[cfg(windows)]
fn fallback_profile() -> LaunchProfile {
    let executable = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32")
        .join("cmd.exe");
    LaunchProfile::complete("command-prompt", "Command Prompt", executable)
}

#[cfg(unix)]
fn fallback_profile() -> LaunchProfile {
    LaunchProfile::complete("default-shell", "Default shell", "/bin/sh")
}

#[cfg(not(any(windows, unix)))]
fn fallback_profile() -> LaunchProfile {
    LaunchProfile::complete("default-shell", "Default shell", PathBuf::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::action::AppAction;

    static TEMP_FILE_NUMBER: AtomicUsize = AtomicUsize::new(0);

    fn discovered_profile() -> LaunchProfile {
        let executable = std::env::current_exe().unwrap();
        LaunchProfile::complete("powershell", "Discovered PowerShell", executable)
    }

    #[test]
    fn explicit_profile_fields_override_discovery() {
        let json = br#"{
            "profiles": [{
                "id": "powershell",
                "label": "Work shell",
                "arguments": ["-NoLogo"],
                "environment": {"MIGHTTY_TEST": "yes"}
            }],
            "default_profile": "powershell"
        }"#;
        let settings = parse_and_resolve(json, Path::new("."), vec![discovered_profile()]).unwrap();
        let profile = settings.profiles.get(&settings.default_profile).unwrap();

        assert_eq!(profile.label, "Work shell");
        assert_eq!(profile.launch.arguments, [OsString::from("-NoLogo")]);
        assert_eq!(
            profile.launch.environment.get(OsStr::new("MIGHTTY_TEST")),
            Some(&OsString::from("yes"))
        );
    }

    #[test]
    fn rejects_duplicate_profile_ids() {
        let executable = std::env::current_exe().unwrap();
        let json = format!(
            r#"{{
                "profiles": [
                    {{"id": "same", "executable": {path:?}}},
                    {{"id": "same", "executable": {path:?}}}
                ]
            }}"#,
            path = executable.to_string_lossy()
        );

        let error = parse_and_resolve(json.as_bytes(), Path::new("."), Vec::new()).unwrap_err();
        assert!(error.to_string().contains("duplicated"));
    }

    #[test]
    fn rejects_duplicate_environment_names_without_partial_resolution() {
        let executable = std::env::current_exe().unwrap();
        let json = format!(
            r#"{{
                "profiles": [{{
                    "id": "test",
                    "executable": {path:?},
                    "environment": {{"Path": "one", "PATH": "two"}}
                }}]
            }}"#,
            path = executable.to_string_lossy()
        );

        let error = parse_and_resolve(json.as_bytes(), Path::new("."), Vec::new()).unwrap_err();
        assert!(error.to_string().contains("duplicated"));
    }

    #[test]
    fn resolves_terminal_theme_and_typed_binding() {
        let json = br##"{
            "terminal": {
                "font_family": "Test Mono",
                "font_size_px": 18,
                "theme": {"background": "#102030"}
            },
            "key_bindings": [{
                "chord": "CTRL-SHIFT-P",
                "action": {"type": "command_palette"}
            }]
        }"##;
        let settings = parse_and_resolve(json, Path::new("."), vec![discovered_profile()]).unwrap();

        assert_eq!(settings.terminal.font_family, "Test Mono");
        assert_eq!(settings.terminal.font_size_px, 18.0);
        assert_eq!(settings.terminal.theme.background, gpui::rgb(0x102030));
        let binding = settings
            .key_bindings
            .iter()
            .find(|binding| binding.chord == "ctrl-shift-p")
            .unwrap();
        assert_eq!(binding.action, AppAction::CommandPalette);
    }

    #[test]
    fn rejects_an_invalid_profile_before_publication() {
        let json = br#"{
            "profiles": [{
                "id": "missing",
                "executable": "this-executable-does-not-exist.exe"
            }]
        }"#;
        let error = parse_and_resolve(json, Path::new("."), Vec::new()).unwrap_err();

        assert!(error.to_string().contains("does not exist"));
    }

    #[test]
    fn invalid_reload_keeps_the_previous_generation() {
        let number = TEMP_FILE_NUMBER.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "mightty-settings-test-{}-{number}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(SETTINGS_FILE_NAME);
        let executable = std::env::current_exe().unwrap();
        let valid = format!(
            r#"{{
                "terminal": {{"font_size_px": 19}},
                "profiles": [{{"id": "test", "executable": {executable:?}}}],
                "default_profile": "test"
            }}"#,
            executable = executable.to_string_lossy()
        );
        fs::write(&path, valid).unwrap();
        let mut store = SettingsStore::open(path.clone());
        let generation = store.generation();
        let font_size = store.current().terminal.font_size_px;

        fs::write(&path, b"{invalid").unwrap();
        assert!(matches!(
            store.reload_if_changed(),
            ReloadOutcome::Rejected(_)
        ));
        assert_eq!(store.generation(), generation);
        assert_eq!(store.current().terminal.font_size_px, font_size);
        assert!(store.diagnostic().is_some());

        fs::remove_dir_all(directory).unwrap();
    }
}
