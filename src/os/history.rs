//! Connection history: a small, **non-secret** record of when each host was last
//! connected to. It backs the "recent" sort on the host list and the detail
//! pane's "Last connected" line.
//!
//! Kept deliberately separate from the two files the rest of the app owns: the
//! SSH config (which must round-trip losslessly, so we never scribble metadata
//! into it) and the vault (which holds secrets). History is just timestamps —
//! losing it is harmless — so reads and writes fail soft rather than ever
//! surfacing an error to the user. Like the rest of `os/`, this module has **zero
//! ratatui dependency**.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The default history path, `~/.ssh/sshm-history.json`.
pub fn default_path() -> Option<PathBuf> {
    super::ssh_dir().map(|d| d.join("sshm-history.json"))
}

/// Current wall-clock as unix seconds (0 on a pre-epoch clock — only possible
/// with a badly misconfigured system, and harmless here).
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// On-disk shape. A wrapper (rather than a bare map) leaves room to grow the
/// format — e.g. a connection counter — without a breaking change.
#[derive(Default, Deserialize)]
struct HistoryFile {
    #[serde(default)]
    last_connected: BTreeMap<String, u64>,
}

/// Borrowing serialize view, so saving makes no second copy of the map.
#[derive(Serialize)]
struct HistoryFileRef<'a> {
    last_connected: &'a BTreeMap<String, u64>,
}

/// In-memory connection history: host alias → last-connected unix seconds.
#[derive(Default)]
pub struct History {
    entries: BTreeMap<String, u64>,
}

impl History {
    /// Load history from disk. Any problem (missing file, unreadable, corrupt
    /// JSON) yields an empty history rather than an error — it's only timestamps.
    pub fn load() -> History {
        let Some(path) = default_path() else {
            return History::default();
        };
        let Ok(data) = std::fs::read_to_string(&path) else {
            return History::default();
        };
        History::from_json(&data)
    }

    /// Parse history JSON, failing soft to an empty history on any corruption.
    /// Split out from [`History::load`] (which hardcodes the real path) so the
    /// fail-soft contract is unit-testable against fixture strings.
    fn from_json(data: &str) -> History {
        match serde_json::from_str::<HistoryFile>(data) {
            Ok(file) => History {
                entries: file.last_connected,
            },
            Err(_) => History::default(),
        }
    }

    /// Last-connected timestamp (unix seconds) for `alias`, if ever recorded.
    pub fn last(&self, alias: &str) -> Option<u64> {
        self.entries.get(alias).copied()
    }

    /// Stamp `alias` as connected-now and persist. Best-effort: a write failure
    /// is swallowed (history loss is non-fatal and must never block a connect).
    pub fn record(&mut self, alias: &str) {
        if alias.is_empty() {
            return;
        }
        self.entries.insert(alias.to_string(), now_unix());
        let _ = self.save();
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = default_path() else {
            return Ok(());
        };
        let json = serde_json::to_string_pretty(&HistoryFileRef {
            last_connected: &self.entries,
        })
        .map_err(io::Error::other)?;
        atomic_write(&path, json.as_bytes())
    }
}

/// Write `bytes` to `path` via a private temp file, then swap it in by
/// **delete-then-rename** (`rename` fails on Windows if the destination exists, so
/// the old file is removed first). This deliberately differs from the config and
/// vault writers, which use `ReplaceFileW` / a `.bak` dance to avoid any window
/// where the file is missing: here that window is acceptable, because a torn or
/// lost history file is harmless (it reloads as empty) — unlike the config or the
/// secrets, it is never worth extra machinery to protect.
fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;

    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    crate::secure_fs::create_dir_private(dir)?;

    let tmp = dir.join(crate::secure_fs::temp_name(".sshm-history")?);
    let write_res = (|| -> io::Result<()> {
        let mut f = crate::secure_fs::create_new_private(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = write_res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    crate::secure_fs::fsync_parent_dir(dir);
    Ok(())
}

/// A short, human "time since" label for a unix-seconds timestamp relative to
/// `now` — e.g. `just now`, `5m ago`, `2h ago`, `yesterday`, `3d ago`, `2w ago`,
/// `4mo ago`, `1y ago`. A `then` in the future (clock skew) reads as `just now`.
/// Pure, so it is unit-tested directly.
pub fn relative_label(then: u64, now: u64) -> String {
    const MIN: u64 = 60;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;
    const MONTH: u64 = 30 * DAY;
    const YEAR: u64 = 365 * DAY;

    if now <= then {
        return "just now".into();
    }
    let s = now - then;
    if s < MIN {
        "just now".into()
    } else if s < HOUR {
        format!("{}m ago", s / MIN)
    } else if s < DAY {
        format!("{}h ago", s / HOUR)
    } else if s < 2 * DAY {
        "yesterday".into()
    } else if s < WEEK {
        format!("{}d ago", s / DAY)
    } else if s < MONTH {
        format!("{}w ago", s / WEEK)
    } else if s < YEAR {
        format!("{}mo ago", s / MONTH)
    } else {
        format!("{}y ago", s / YEAR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;

    #[test]
    fn relative_label_buckets() {
        let now = 1_000_000_000;
        assert_eq!(relative_label(now, now), "just now");
        assert_eq!(relative_label(now - 5, now), "just now");
        assert_eq!(relative_label(now - 5 * MIN, now), "5m ago");
        assert_eq!(relative_label(now - 2 * HOUR, now), "2h ago");
        assert_eq!(relative_label(now - 25 * HOUR, now), "yesterday");
        assert_eq!(relative_label(now - 3 * DAY, now), "3d ago");
        assert_eq!(relative_label(now - 14 * DAY, now), "2w ago");
        assert_eq!(relative_label(now - 60 * DAY, now), "2mo ago");
        assert_eq!(relative_label(now - 400 * DAY, now), "1y ago");
    }

    #[test]
    fn relative_label_exact_boundaries() {
        // Guards against a flipped `<`/`<=` at each bucket edge.
        let now = 1_000_000_000;
        assert_eq!(relative_label(now - MIN, now), "1m ago"); // 60s -> minutes
        assert_eq!(relative_label(now - HOUR, now), "1h ago"); // exactly 1h
        assert_eq!(relative_label(now - DAY, now), "yesterday"); // exactly 24h
        assert_eq!(relative_label(now - 2 * DAY, now), "2d ago"); // exactly 48h -> days
        assert_eq!(relative_label(now - 7 * DAY, now), "1w ago"); // exactly 1 week
        assert_eq!(relative_label(now - 30 * DAY, now), "1mo ago"); // exactly 30d
        assert_eq!(relative_label(now - 365 * DAY, now), "1y ago"); // exactly 1 year
    }

    #[test]
    fn relative_label_future_is_just_now() {
        // Clock skew: a timestamp ahead of `now` must not underflow.
        assert_eq!(relative_label(2_000, 1_000), "just now");
    }

    #[test]
    fn record_and_last_roundtrip_in_memory() {
        let mut h = History::default();
        assert_eq!(h.last("web"), None);
        // Insert directly to avoid touching the real ~/.ssh during the test;
        // `record` additionally persists (exercised via `atomic_write` below).
        h.entries.insert("web".into(), 42);
        assert_eq!(h.last("web"), Some(42));
        assert_eq!(h.last("db"), None);
    }

    #[test]
    fn record_ignores_empty_alias() {
        // A pattern-less/empty Host block yields alias() == "" — it must not be
        // recorded (and must not trigger a write to the real ~/.ssh).
        let mut h = History::default();
        h.record("");
        assert_eq!(h.last(""), None);
        assert!(h.entries.is_empty());
    }

    #[test]
    fn from_json_fails_soft_and_parses() {
        // Corrupt / empty input -> empty history, never an error.
        assert!(History::from_json("not json at all").entries.is_empty());
        assert!(History::from_json("").entries.is_empty());
        // Well-formed JSON missing the field -> empty (serde default).
        assert!(History::from_json("{}").entries.is_empty());
        // Valid payload round-trips.
        let h = History::from_json(r#"{"last_connected":{"web":7}}"#);
        assert_eq!(h.last("web"), Some(7));
    }

    #[test]
    fn atomic_write_then_parse_roundtrips() {
        let dir = std::env::temp_dir().join(crate::secure_fs::temp_name(".sshmhist").unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sshm-history.json");

        let mut map = BTreeMap::new();
        map.insert("web".to_string(), 123u64);
        map.insert("db".to_string(), 456u64);
        let json = serde_json::to_string_pretty(&HistoryFileRef {
            last_connected: &map,
        })
        .unwrap();

        atomic_write(&path, json.as_bytes()).unwrap();
        // Write again so the delete-then-rename (destination exists) branch runs.
        atomic_write(&path, json.as_bytes()).unwrap();

        let data = std::fs::read_to_string(&path).unwrap();
        let parsed: HistoryFile = serde_json::from_str(&data).unwrap();
        assert_eq!(parsed.last_connected.get("web"), Some(&123));
        assert_eq!(parsed.last_connected.get("db"), Some(&456));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
