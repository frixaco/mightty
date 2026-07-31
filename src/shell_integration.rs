//! Shell-reported metadata policy and integration resources.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use url::Url;

use crate::profile::LaunchSpec;
use crate::settings::settings_path;
use crate::workspace::trusted_working_directory;

const MAX_METADATA_BYTES: usize = 4096;
const MAX_TITLE_CHARACTERS: usize = 128;
const POWERSHELL_RESOURCE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/shell-integration/mightty.ps1"
));
const BASH_RESOURCE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/shell-integration/mightty.bash"
));

/// Add terminal identity and automatic PowerShell integration to one launch.
pub fn prepare_launch(launch: &LaunchSpec) -> io::Result<LaunchSpec> {
    let directory = settings_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("shell-integration");
    prepare_launch_in(launch, &directory)
}

/// Convert an OSC 7 report into a trusted local working directory.
pub fn local_working_directory(report: &str) -> Option<PathBuf> {
    parse_local_working_directory(report, local_hostname().as_deref())
}

/// Remove terminal control data and bound the tab title.
pub fn display_title(report: Option<&str>) -> Option<String> {
    let title = report?
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_TITLE_CHARACTERS)
        .collect::<String>();
    let title = title.trim();
    (!title.is_empty()).then(|| title.to_string())
}

fn parse_local_working_directory(report: &str, local_hostname: Option<&str>) -> Option<PathBuf> {
    if report.len() > MAX_METADATA_BYTES {
        return None;
    }
    let mut url = Url::parse(report).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    let host = uri_authority(report).or_else(|| url.host_str());
    if let Some(host) = host
        && !host.is_empty()
        && !host.eq_ignore_ascii_case("localhost")
        && !local_hostname.is_some_and(|local| host.eq_ignore_ascii_case(local))
    {
        return None;
    }
    url.set_host(None).ok()?;
    let path = url.to_file_path().ok()?;
    trusted_working_directory(&path).then_some(path)
}

fn uri_authority(uri: &str) -> Option<&str> {
    let (_, remainder) = uri.split_once("://")?;
    Some(
        remainder
            .split_once(['/', '\\'])
            .map_or(remainder, |(authority, _)| authority),
    )
}

fn local_hostname() -> Option<String> {
    #[cfg(windows)]
    let hostname = std::env::var("COMPUTERNAME").ok();
    #[cfg(not(windows))]
    let hostname = std::env::var("HOSTNAME").ok();

    hostname.filter(|hostname| !hostname.is_empty())
}

fn prepare_launch_in(launch: &LaunchSpec, directory: &Path) -> io::Result<LaunchSpec> {
    let mut launch = launch.clone();
    launch
        .environment
        .insert(OsString::from("TERM_PROGRAM"), OsString::from("mightty"));
    launch.environment.insert(
        OsString::from("TERM_PROGRAM_VERSION"),
        OsString::from(env!("CARGO_PKG_VERSION")),
    );

    fs::create_dir_all(directory)?;
    let powershell_path = directory.join("mightty.ps1");
    write_resource(&powershell_path, POWERSHELL_RESOURCE)?;
    write_resource(&directory.join("mightty.bash"), BASH_RESOURCE)?;

    if is_powershell(launch.executable()) && launch.arguments.is_empty() {
        let escaped_path = powershell_path.to_string_lossy().replace('\'', "''");
        launch.arguments.extend([
            OsString::from("-NoExit"),
            OsString::from("-Command"),
            OsString::from(format!(". '{escaped_path}'")),
        ]);
        launch.environment.insert(
            OsString::from("MIGHTTY_SHELL_INTEGRATION"),
            OsString::from("1"),
        );
    }
    Ok(launch)
}

fn is_powershell(executable: &OsStr) -> bool {
    Path::new(executable)
        .file_stem()
        .and_then(OsStr::to_str)
        .is_some_and(|name| {
            name.eq_ignore_ascii_case("pwsh") || name.eq_ignore_ascii_case("powershell")
        })
}

fn write_resource(path: &Path, contents: &[u8]) -> io::Result<()> {
    match fs::read(path) {
        Ok(current) if current == contents => Ok(()),
        Ok(_) | Err(_) => fs::write(path, contents),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_DIRECTORY_NUMBER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn accepts_only_local_file_working_directories() {
        #[cfg(windows)]
        let (local, expected) = (
            "file://workstation/C:/src/mightty",
            PathBuf::from(r"C:\src\mightty"),
        );
        #[cfg(not(windows))]
        let (local, expected) = (
            "file://workstation/src/mightty",
            PathBuf::from("/src/mightty"),
        );

        assert_eq!(
            parse_local_working_directory(local, Some("WORKSTATION")),
            Some(expected)
        );
        assert!(parse_local_working_directory(local, Some("other-host")).is_none());
        assert!(
            parse_local_working_directory("https://localhost/src", Some("localhost")).is_none()
        );
        assert!(parse_local_working_directory("file:relative", Some("localhost")).is_none());
    }

    #[test]
    fn cleans_and_bounds_terminal_titles() {
        let long = format!("\u{1b}]0;{}\u{7}", "a".repeat(200));
        let title = display_title(Some(&long)).unwrap();

        assert_eq!(title.chars().count(), MAX_TITLE_CHARACTERS);
        assert!(!title.chars().any(char::is_control));
        assert_eq!(display_title(Some("\r\n")), None);
    }

    #[test]
    fn injects_powershell_without_replacing_explicit_arguments() {
        let number = TEMP_DIRECTORY_NUMBER.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "mightty-integration-{}-{number}",
            std::process::id()
        ));

        let prepared = prepare_launch_in(&LaunchSpec::new("pwsh.exe"), &directory).unwrap();
        assert_eq!(
            prepared.arguments,
            [
                OsString::from("-NoExit"),
                OsString::from("-Command"),
                OsString::from(format!(
                    ". '{}'",
                    directory.join("mightty.ps1").to_string_lossy()
                )),
            ]
        );
        assert_eq!(
            prepared.environment.get(OsStr::new("TERM_PROGRAM")),
            Some(&OsString::from("mightty"))
        );
        assert_eq!(
            fs::read(directory.join("mightty.ps1")).unwrap(),
            POWERSHELL_RESOURCE
        );
        assert_eq!(
            fs::read(directory.join("mightty.bash")).unwrap(),
            BASH_RESOURCE
        );

        let explicit = LaunchSpec::new("pwsh.exe").with_arguments(["-NoProfile"]);
        assert_eq!(
            prepare_launch_in(&explicit, &directory).unwrap().arguments,
            explicit.arguments
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
