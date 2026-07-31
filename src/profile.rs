use std::fmt;
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
}
