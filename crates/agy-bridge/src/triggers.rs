//! Trigger configuration types for the Python Antigravity SDK.
//!
//! Provides [`TriggerConfig`], [`TriggerEntry`], and `TriggerSet` — the
//! Rust-side configuration types that are serialized and passed to the
//! Python SDK's trigger system (`@every`, `@on_file_change`). The actual
//! file watching and scheduling is handled entirely by the Python SDK;
//! this module contains no native Rust trigger implementations.

use std::{path::PathBuf, time::Duration};

use serde::{Deserialize, Serialize};

use crate::error::Error;

/// Configuration for a trigger that the SDK will run.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerConfig {
    /// Periodic trigger: fires every `interval`.
    Every {
        /// Interval between firings.
        #[serde(with = "duration_secs")]
        interval: Duration,
    },
    /// File-change trigger: fires when files change under `path`.
    OnFileChange {
        /// Directory to watch for changes.
        path: PathBuf,
    },
}

impl TriggerConfig {
    /// Minimum allowed interval for an `Every` trigger.
    const MIN_INTERVAL: Duration = Duration::from_secs(1);

    /// Create an `Every` trigger from a number of seconds.
    ///
    /// # Panics
    ///
    /// Panics if `secs` is 0.
    #[must_use]
    pub const fn every_secs(secs: u64) -> Self {
        assert!(secs >= 1, "trigger interval must be at least 1 second");
        Self::Every {
            interval: Duration::from_secs(secs),
        }
    }

    /// Create an `Every` trigger with a specific Duration.
    ///
    /// # Panics
    ///
    /// Panics if the duration is less than 1 second.
    #[must_use]
    pub fn every(duration: Duration) -> Self {
        assert!(
            duration >= Self::MIN_INTERVAL,
            "trigger interval must be at least 1 second, got {duration:?}"
        );
        Self::Every { interval: duration }
    }

    /// Create an `OnFileChange` trigger watching the given directory.
    ///
    /// # Panics
    ///
    /// Panics if `path` is empty, relative, or contains `..`.
    #[must_use]
    pub fn on_file_change(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        assert!(
            !path.as_os_str().is_empty(),
            "on_file_change path must not be empty"
        );
        assert!(
            path.is_absolute(),
            "on_file_change path must be absolute, got: {}",
            path.display()
        );
        assert!(
            !path
                .components()
                .any(|c| c == std::path::Component::ParentDir),
            "on_file_change path must not contain '..', got: {}",
            path.display()
        );
        Self::OnFileChange { path }
    }

    /// Fallible version of [`on_file_change`](Self::on_file_change).
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] if `path` is empty, relative,
    /// or contains `..`.
    pub fn try_on_file_change(path: impl Into<PathBuf>) -> Result<Self, Error> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(Error::InvalidConfig {
                message: "on_file_change path must not be empty".to_owned(),
            });
        }
        if !path.is_absolute() {
            return Err(Error::InvalidConfig {
                message: format!(
                    "on_file_change path must be absolute, got: {}",
                    path.display()
                ),
            });
        }
        if path
            .components()
            .any(|c| c == std::path::Component::ParentDir)
        {
            return Err(Error::InvalidConfig {
                message: format!(
                    "on_file_change path must not contain '..', got: {}",
                    path.display()
                ),
            });
        }
        Ok(Self::OnFileChange { path })
    }

    /// Fallible version of [`every`](Self::every).
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] if the duration is less than 1 second.
    pub fn try_every(duration: Duration) -> Result<Self, Error> {
        if duration < Self::MIN_INTERVAL {
            return Err(Error::InvalidConfig {
                message: format!("trigger interval must be at least 1 second, got {duration:?}"),
            });
        }
        Ok(Self::Every { interval: duration })
    }

    /// Human-readable description for logging.
    #[must_use]
    pub fn description(&self) -> String {
        match self {
            Self::Every { interval } => format!("every({}s)", interval.as_secs()),
            Self::OnFileChange { path } => format!("on_file_change({})", path.display()),
        }
    }
}

/// A named trigger attached to an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerEntry {
    /// Descriptive name for the trigger (e.g. `"poll_threads"`).
    pub name: String,
    /// The trigger configuration.
    pub config: TriggerConfig,
    /// Message template sent to the agent when the trigger fires.
    /// For `OnFileChange`, the placeholder `{changes}` is replaced with
    /// the list of changed files.
    pub message_template: String,
}

impl TriggerEntry {
    /// Validate that the entry has non-empty name and `message_template`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] if the name or
    /// `message_template` is empty.
    pub fn validate(&self) -> Result<(), Error> {
        if self.name.trim().is_empty() {
            return Err(Error::InvalidConfig {
                message: "TriggerEntry name must not be empty".to_owned(),
            });
        }
        if self.message_template.trim().is_empty() {
            return Err(Error::InvalidConfig {
                message: format!("TriggerEntry '{}' has an empty message_template", self.name),
            });
        }
        Ok(())
    }
}

/// An ordered list of triggers to attach to an agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TriggerSet {
    entries: Vec<TriggerEntry>,
}

impl TriggerSet {
    /// Create an empty trigger set.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Add a trigger entry.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] if the entry fails validation
    /// (empty name or `message_template`).
    pub fn push(&mut self, entry: TriggerEntry) -> Result<(), Error> {
        entry.validate()?;
        self.entries.push(entry);
        Ok(())
    }

    /// Iterate over trigger entries.
    pub fn iter(&self) -> impl Iterator<Item = &TriggerEntry> {
        self.entries.iter()
    }

    /// Number of triggers.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the set is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl From<TriggerSet> for Vec<TriggerEntry> {
    fn from(set: TriggerSet) -> Self {
        set.entries
    }
}

impl From<&TriggerSet> for Vec<TriggerEntry> {
    fn from(set: &TriggerSet) -> Self {
        set.entries.clone()
    }
}

impl FromIterator<TriggerEntry> for TriggerSet {
    fn from_iter<T: IntoIterator<Item = TriggerEntry>>(iter: T) -> Self {
        let mut set = Self::new();
        for entry in iter {
            set.push(entry)
                .expect("TriggerSet::from_iter: invalid trigger entry");
        }
        set
    }
}

impl From<Vec<TriggerEntry>> for TriggerSet {
    fn from(entries: Vec<TriggerEntry>) -> Self {
        Self::from_iter(entries)
    }
}

impl<const N: usize> From<[TriggerEntry; N]> for TriggerSet {
    fn from(entries: [TriggerEntry; N]) -> Self {
        Self::from_iter(entries)
    }
}

impl IntoIterator for TriggerSet {
    type Item = TriggerEntry;
    type IntoIter = std::vec::IntoIter<TriggerEntry>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl<'a> IntoIterator for &'a TriggerSet {
    type Item = &'a TriggerEntry;
    type IntoIter = std::slice::Iter<'a, TriggerEntry>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

/// Serde helper: serialize/deserialize [`Duration`] as fractional seconds (`f64`).
///
/// This preserves sub-second precision (e.g. `Duration::from_millis(1500)` →
/// `1.5`) while remaining human-readable.
mod duration_secs {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &Duration, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_f64(d.as_secs_f64())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Duration, D::Error> {
        let secs = f64::deserialize(de)?;
        if secs < 0.0 {
            return Err(serde::de::Error::custom("duration must not be negative"));
        }
        Ok(Duration::from_secs_f64(secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_trigger_description() {
        let t = TriggerConfig::every_secs(30);
        assert_eq!(t.description(), "every(30s)");
    }

    #[test]
    fn on_file_change_trigger_description() {
        let t = TriggerConfig::on_file_change("/workspace/threads");
        assert_eq!(t.description(), "on_file_change(/workspace/threads)");
    }

    #[test]
    fn every_fires_at_expected_interval() {
        let t = TriggerConfig::every_secs(60);
        match t {
            TriggerConfig::Every { interval } => {
                assert_eq!(interval, Duration::from_mins(1));
            }
            TriggerConfig::OnFileChange { .. } => {
                panic!("Expected Every trigger");
            }
        }
    }

    #[test]
    fn on_file_change_detects_path() {
        let t = TriggerConfig::on_file_change("/workspace/sessions/bug123/threads");
        match t {
            TriggerConfig::OnFileChange { path } => {
                assert_eq!(path, PathBuf::from("/workspace/sessions/bug123/threads"));
            }
            TriggerConfig::Every { .. } => {
                panic!("Expected OnFileChange trigger");
            }
        }
    }

    #[test]
    fn trigger_config_serde_roundtrip() {
        let configs = vec![
            TriggerConfig::every_secs(10),
            TriggerConfig::on_file_change("/tmp/watch"),
        ];
        for config in &configs {
            let json = serde_json::to_string(config).expect("serialize");
            let parsed: TriggerConfig = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&parsed, config);
        }
    }

    #[test]
    fn trigger_set_operations() {
        let mut set = TriggerSet::new();
        assert!(set.is_empty());

        set.push(TriggerEntry {
            name: "poll_threads".to_owned(),
            config: TriggerConfig::every_secs(30),
            message_template: "Check threads for updates".to_owned(),
        })
        .unwrap();
        set.push(TriggerEntry {
            name: "watch_threads".to_owned(),
            config: TriggerConfig::on_file_change("/workspace/threads"),
            message_template: "New files in threads: {changes}".to_owned(),
        })
        .unwrap();

        assert_eq!(set.len(), 2);
        let names: Vec<&str> = set.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["poll_threads", "watch_threads"]);
    }

    #[test]
    fn trigger_entry_serde_roundtrip() {
        let entry = TriggerEntry {
            name: "poll".to_owned(),
            config: TriggerConfig::every_secs(15),
            message_template: "time to poll".to_owned(),
        };
        let json = serde_json::to_string(&entry).expect("serialize");
        let parsed: TriggerEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.name, entry.name);
        assert_eq!(parsed.config, entry.config);
        assert_eq!(parsed.message_template, entry.message_template);
    }

    #[test]
    fn trigger_set_serde_roundtrip() {
        let mut set = TriggerSet::new();
        set.push(TriggerEntry {
            name: "poll".to_owned(),
            config: TriggerConfig::every_secs(60),
            message_template: "poll now".to_owned(),
        })
        .unwrap();
        set.push(TriggerEntry {
            name: "watch".to_owned(),
            config: TriggerConfig::on_file_change("/tmp"),
            message_template: "files changed: {changes}".to_owned(),
        })
        .unwrap();
        let json = serde_json::to_string(&set).expect("serialize");
        let parsed: TriggerSet = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.len(), 2);
        let names: Vec<&str> = parsed.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["poll", "watch"]);
    }

    #[test]
    fn trigger_set_from_conversions() {
        let mut set = TriggerSet::new();
        set.push(TriggerEntry {
            name: "poll".to_owned(),
            config: TriggerConfig::every_secs(60),
            message_template: "poll now".to_owned(),
        })
        .unwrap();

        let vec_from_owned: Vec<TriggerEntry> = Vec::from(set.clone());
        assert_eq!(vec_from_owned.len(), 1);
        assert_eq!(vec_from_owned[0].name, "poll");

        let vec_from_ref: Vec<TriggerEntry> = Vec::from(&set);
        assert_eq!(vec_from_ref.len(), 1);
        assert_eq!(vec_from_ref[0].name, "poll");

        let entry = TriggerEntry {
            name: "poll".to_owned(),
            config: TriggerConfig::every_secs(60),
            message_template: "poll now".to_owned(),
        };
        let set_from_arr = TriggerSet::from([entry.clone()]);
        assert_eq!(set_from_arr.len(), 1);

        let set_from_vec = TriggerSet::from(vec![entry]);
        assert_eq!(set_from_vec.len(), 1);
    }

    #[test]
    fn trigger_set_default_is_empty() {
        let set = TriggerSet::default();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
    }

    #[test]
    #[should_panic(expected = "trigger interval must be at least 1 second")]
    fn every_trigger_zero_seconds_panics() {
        eprintln!("{:?}", TriggerConfig::every_secs(0));
    }

    #[test]
    #[should_panic(expected = "trigger interval must be at least 1 second")]
    fn every_trigger_sub_second_panics() {
        eprintln!("{:?}", TriggerConfig::every(Duration::from_millis(500)));
    }

    #[test]
    fn duration_secs_serializes_as_number() {
        let config = TriggerConfig::every_secs(120);
        let json = serde_json::to_string(&config).expect("serialize");
        // The interval should serialize as a number
        assert!(json.contains("120"), "Expected '120' in {json}");
    }

    #[test]
    fn duration_secs_preserves_subsecond_via_serde() {
        // Construct via serde to bypass the constructor validation
        let json = r#"{"Every":{"interval":1.5}}"#;
        let parsed: TriggerConfig = serde_json::from_str(json).expect("deserialize");
        match &parsed {
            TriggerConfig::Every { interval } => {
                assert_eq!(*interval, Duration::from_millis(1500));
            }
            TriggerConfig::OnFileChange { .. } => panic!("Expected Every, got OnFileChange"),
        }
        // Re-serialize should preserve 1.5
        let reserialized = serde_json::to_string(&parsed).expect("serialize");
        assert!(
            reserialized.contains("1.5"),
            "Sub-second duration should round-trip, got {reserialized}"
        );
    }

    #[test]
    #[should_panic(expected = "on_file_change path must not be empty")]
    fn on_file_change_empty_path_panics() {
        eprintln!("{:?}", TriggerConfig::on_file_change(""));
    }

    #[test]
    #[should_panic(expected = "on_file_change path must be absolute")]
    fn on_file_change_relative_path_panics() {
        eprintln!("{:?}", TriggerConfig::on_file_change("relative/path"));
    }

    #[test]
    #[should_panic(expected = "on_file_change path must not contain '..'")]
    fn on_file_change_parent_traversal_panics() {
        eprintln!(
            "{:?}",
            TriggerConfig::on_file_change("/workspace/../etc/passwd")
        );
    }

    #[test]
    fn trigger_entry_validate_empty_name() {
        let entry = TriggerEntry {
            name: "  ".to_owned(),
            config: TriggerConfig::every_secs(10),
            message_template: "msg".to_owned(),
        };
        assert!(entry.validate().is_err());
    }

    #[test]
    fn trigger_entry_validate_empty_template() {
        let entry = TriggerEntry {
            name: "poll".to_owned(),
            config: TriggerConfig::every_secs(10),
            message_template: "  ".to_owned(),
        };
        assert!(entry.validate().is_err());
    }

    #[test]
    fn trigger_entry_validate_ok() {
        let entry = TriggerEntry {
            name: "poll".to_owned(),
            config: TriggerConfig::every_secs(10),
            message_template: "poll now".to_owned(),
        };
        assert!(entry.validate().is_ok());
    }

    #[test]
    fn trigger_config_equality() {
        assert_eq!(TriggerConfig::every_secs(30), TriggerConfig::every_secs(30));
        assert_ne!(TriggerConfig::every_secs(30), TriggerConfig::every_secs(60));
        assert_ne!(
            TriggerConfig::every_secs(30),
            TriggerConfig::on_file_change("/tmp")
        );
        assert_eq!(
            TriggerConfig::on_file_change("/a"),
            TriggerConfig::on_file_change("/a")
        );
        assert_ne!(
            TriggerConfig::on_file_change("/a"),
            TriggerConfig::on_file_change("/b")
        );
    }

    #[test]
    fn trigger_large_interval() {
        let t = TriggerConfig::every_secs(86400); // 24h
        assert_eq!(t.description(), "every(86400s)");
    }

    // ── Fallible constructor tests ───────────────────────────────────────

    #[test]
    fn try_on_file_change_ok() {
        let t = TriggerConfig::try_on_file_change("/workspace/threads").unwrap();
        match t {
            TriggerConfig::OnFileChange { path } => {
                assert_eq!(path, PathBuf::from("/workspace/threads"));
            }
            TriggerConfig::Every { .. } => panic!("Expected OnFileChange"),
        }
    }

    #[test]
    fn try_on_file_change_empty_is_err() {
        assert!(TriggerConfig::try_on_file_change("").is_err());
    }

    #[test]
    fn try_on_file_change_relative_is_err() {
        assert!(TriggerConfig::try_on_file_change("relative/path").is_err());
    }

    #[test]
    fn try_on_file_change_parent_dir_is_err() {
        assert!(TriggerConfig::try_on_file_change("/workspace/../etc/passwd").is_err());
    }

    #[test]
    fn try_every_ok() {
        let t = TriggerConfig::try_every(Duration::from_secs(5)).unwrap();
        match t {
            TriggerConfig::Every { interval } => {
                assert_eq!(interval, Duration::from_secs(5));
            }
            TriggerConfig::OnFileChange { .. } => panic!("Expected Every"),
        }
    }

    #[test]
    fn try_every_sub_second_is_err() {
        assert!(TriggerConfig::try_every(Duration::from_millis(500)).is_err());
    }
}
