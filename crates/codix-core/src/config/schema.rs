//! The settings schema: every key's type, default, allowed values, description,
//! owning file and effect. Validation, `codix config --describe` and the docs
//! all read this table.

use std::time::Duration;

use serde::Serialize;

/// Which file holds a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFile {
    /// `codix.toml`, committed: facts about the code.
    Shared,
    /// `.codix/config.toml`, not committed: personal or machine preferences.
    Local,
}

impl ConfigFile {
    /// The file's path relative to the project root.
    pub fn path(self) -> &'static str {
        match self {
            Self::Shared => "codix.toml",
            Self::Local => ".codix/config.toml",
        }
    }
}

/// A key's value type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Bytes, such as `2MB`.
    Size,
    /// Seconds to days, such as `10m`.
    Duration,
    /// A language name.
    Language,
    /// Root-relative directories.
    PathList,
}

impl Kind {
    /// The type's name and the values it accepts.
    pub fn describe(self) -> String {
        match self {
            Self::Size => "size, such as 2MB (units B, KB, MB, GB; 1 KB = 1024 B)".to_owned(),
            Self::Duration => "duration, such as 10m (units s, m, h, d)".to_owned(),
            Self::Language => format!(
                "language, one of: {}",
                codix_provider::known::LANGUAGES.join(", ")
            ),
            Self::PathList => {
                "comma-separated directories, relative to the project root".to_owned()
            }
        }
    }
}

/// One key of the schema.
#[derive(Debug)]
pub struct KeyDef {
    /// Dotted path. For a map, the path of the table holding its entries.
    pub path: &'static str,
    /// Whether the key is a table of entries, each keyed by a glob.
    pub map: bool,
    /// Value type.
    pub kind: Kind,
    /// Default value, if any.
    pub default: Option<&'static str>,
    /// What the key means.
    pub description: &'static str,
    /// Owning file.
    pub file: ConfigFile,
    /// What a change implies.
    pub effect: &'static str,
}

impl KeyDef {
    /// The key as shown in docs: maps get a `"<glob>"` placeholder entry.
    pub fn display(&self) -> String {
        if self.map {
            format!("{}.\"<glob>\"", self.path)
        } else {
            self.path.to_owned()
        }
    }
}

/// Every key, sorted by path.
pub const KEYS: &[KeyDef] = &[
    KeyDef {
        path: "index.max_file_size",
        map: false,
        kind: Kind::Size,
        default: Some("2MB"),
        description: "Files larger than this are skipped.",
        file: ConfigFile::Shared,
        effect: "files crossing the new limit will be reindexed on the next scan",
    },
    KeyDef {
        path: "index.scan_interval",
        map: false,
        kind: Kind::Duration,
        default: Some("10m"),
        description: "How often a running daemon rescans the whole project, catching changes the watcher missed.",
        file: ConfigFile::Local,
        effect: "takes effect the next time a daemon starts",
    },
    KeyDef {
        path: "languages.overrides",
        map: true,
        kind: Kind::Language,
        default: None,
        description: "Language of the files matching each glob, overriding detection.",
        file: ConfigFile::Shared,
        effect: "matching files will be reindexed on the next scan",
    },
    KeyDef {
        path: "sources.roots",
        map: false,
        kind: Kind::PathList,
        default: None,
        description: "Source directories, overriding those found from build files.",
        file: ConfigFile::Shared,
        effect: "files under the old and new roots will be reindexed on the next scan",
    },
];

/// The valid keys, for error hints.
pub fn valid_keys() -> String {
    let keys: Vec<String> = KEYS.iter().map(KeyDef::display).collect();
    format!("valid keys: {}", keys.join(", "))
}

/// Parses a size such as `2MB` or `512` (bytes) into bytes.
pub fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: u64 = number.parse().ok()?;
    let multiplier: u64 = match unit.trim().to_ascii_uppercase().as_str() {
        "" | "B" => 1,
        "KB" => 1 << 10,
        "MB" => 1 << 20,
        "GB" => 1 << 30,
        _ => return None,
    };
    number.checked_mul(multiplier)
}

/// Parses a duration such as `10m` into a [`Duration`].
pub fn parse_duration(text: &str) -> Option<Duration> {
    let text = text.trim();
    let split = text.find(|c: char| !c.is_ascii_digit())?;
    let (number, unit) = text.split_at(split);
    let number: u64 = number.parse().ok()?;
    let seconds: u64 = match unit.trim() {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => return None,
    };
    Some(Duration::from_secs(number.checked_mul(seconds)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("2MB"), Some(2 << 20));
        assert_eq!(parse_size("512"), Some(512));
        assert_eq!(parse_size("3 kb"), Some(3 << 10));
        assert_eq!(parse_size("2TB"), None);
        assert_eq!(parse_size("MB"), None);
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("10m"), Some(Duration::from_mins(10)));
        assert_eq!(parse_duration("1d"), Some(Duration::from_hours(24)));
        assert_eq!(parse_duration("10"), None);
        assert_eq!(parse_duration("5w"), None);
    }

    #[test]
    fn keys_are_sorted_and_defaults_valid() {
        let paths: Vec<&str> = KEYS.iter().map(|k| k.path).collect();
        let mut sorted = paths.clone();
        sorted.sort_unstable();
        assert_eq!(paths, sorted);
        for key in KEYS {
            if let Some(default) = key.default {
                let valid = match key.kind {
                    Kind::Size => parse_size(default).is_some(),
                    Kind::Duration => parse_duration(default).is_some(),
                    Kind::Language | Kind::PathList => true,
                };
                assert!(valid, "{}", key.path);
            }
        }
    }
}
