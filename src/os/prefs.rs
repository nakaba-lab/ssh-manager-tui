//! Tiny, non-secret persisted preferences (`~/.ssh/sshm-prefs.json`).
//!
//! Holds session-spanning UI/connect preferences that are **not** secrets —
//! currently just the connect-time password auto-fill opt-in, so a user who turned
//! it on does not have to re-enable it every launch. Deliberately kept **separate
//! from the encrypted vault**: it carries no secret, so it is plain JSON written
//! owner-private and atomically, rather than going through (and risking) the AEAD
//! vault format. A missing/unreadable/corrupt file is treated as "all defaults" —
//! preferences are best-effort and must never block startup.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Persisted, non-secret preferences. New fields must be `#[serde(default)]` so an
/// older file still loads, and must never hold a secret (that belongs in the vault).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prefs {
    /// Persisted mirror of `App::password_autofill_enabled`. Defaults `false` — the
    /// documented safe default (the server-facing password can burn an auth attempt
    /// under `force`); persistence only remembers a user who explicitly opted in.
    #[serde(default)]
    pub password_autofill_enabled: bool,
}

/// `~/.ssh/sshm-prefs.json`, alongside the vault. `None` if `~/.ssh` won't resolve.
pub fn default_path() -> Option<PathBuf> {
    super::ssh_dir().map(|d| d.join("sshm-prefs.json"))
}

/// Parse prefs JSON, falling back to defaults on any error (empty, garbage, or a
/// shape from a newer build). Pure, so the lenient load contract is unit-tested
/// without touching the real `~/.ssh` (which can't be redirected on Windows).
fn parse(data: &str) -> Prefs {
    serde_json::from_str(data).unwrap_or_default()
}

impl Prefs {
    /// Load preferences, or `Prefs::default()` if the file is absent/unreadable/
    /// corrupt — best-effort, never errors (a bad prefs file must not block launch).
    pub fn load() -> Self {
        let Some(path) = default_path() else {
            return Self::default();
        };
        match fs::read_to_string(&path) {
            Ok(data) => parse(&data),
            Err(_) => Self::default(),
        }
    }

    /// Persist atomically and owner-private. Surfaced to the caller (which toasts a
    /// failure) but never panics or corrupts an existing file. The data is
    /// regenerable, so — unlike the vault — no `.bak` crash-recovery copy is kept.
    pub fn save(&self) -> anyhow::Result<()> {
        let Some(path) = default_path() else {
            anyhow::bail!("cannot resolve ~/.ssh for preferences");
        };
        let json = serde_json::to_string_pretty(self)?;
        write_atomic(&path, json.as_bytes())?;
        Ok(())
    }
}

/// Write `bytes` to `path` atomically and owner-private via the shared `secure_fs`
/// primitives: an O_EXCL, owner-only temp is fsynced then renamed in. Windows
/// `rename` fails if the destination exists, so it is removed first (the same
/// convention the config/vault writers use); prefs are regenerable, so the brief
/// "neither file exists" window a crash there could leave is harmless.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    crate::secure_fs::create_dir_private(dir)?;

    let tmp = dir.join(crate::secure_fs::temp_name(".sshm-prefs")?);
    let write_res = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut f = crate::secure_fs::create_new_private(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = write_res {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    let _ = fs::remove_file(path); // Windows rename won't clobber an existing dest.
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    crate::secure_fs::fsync_parent_dir(dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_off() {
        // The persisted default must match the documented in-app default (auto-fill
        // OFF): an absent prefs file must never silently enable a server-facing
        // secret.
        assert!(!Prefs::default().password_autofill_enabled);
    }

    #[test]
    fn parse_is_lenient_and_round_trips() {
        // Empty object -> default via #[serde(default)] (forward/backward compat).
        assert_eq!(parse("{}"), Prefs::default());
        // Garbage / empty / truncated -> default, never a panic or error.
        assert_eq!(parse(""), Prefs::default());
        assert_eq!(parse("not json"), Prefs::default());
        assert_eq!(parse("{\"password_autofill_enabled\":"), Prefs::default());
        // An explicit true round-trips through serialize -> parse.
        let on = Prefs {
            password_autofill_enabled: true,
        };
        let json = serde_json::to_string(&on).unwrap();
        assert_eq!(parse(&json), on);
        assert!(parse(&json).password_autofill_enabled);
    }
}
