//! Shell-reported metadata policy and integration resources.

use std::path::PathBuf;

use url::Url;

use crate::workspace::trusted_working_directory;

const MAX_METADATA_BYTES: usize = 4096;
const MAX_TITLE_CHARACTERS: usize = 128;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
