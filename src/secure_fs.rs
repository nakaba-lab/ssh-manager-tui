//! Shared primitives for durable, owner-private file writes, used by both the
//! SSH-config writer (`config/`) and the secret vault (`os/vault.rs`) so neither
//! layer depends on the other. Mirrors the hardening the vault writer pioneered:
//! unpredictable O_EXCL temp names, owner-only permissions (`0o600` on unix, an
//! inheritance-stripped owner-only ACL on Windows), and best-effort fsync.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

/// CSPRNG hex for unpredictable temp names; errors rather than emit a weak name.
fn random_hex(n: usize) -> io::Result<String> {
    let mut buf = vec![0u8; n];
    getrandom::getrandom(&mut buf).map_err(|e| io::Error::other(format!("rng error: {e}")))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// A unique, unpredictable temp filename: `<prefix>.<pid>.<16 hex>.tmp`. The
/// randomness defeats pre-creation / symlink squatting on the temp path.
pub fn temp_name(prefix: &str) -> io::Result<String> {
    Ok(format!(
        "{prefix}.{}.{}.tmp",
        std::process::id(),
        random_hex(8)?
    ))
}

/// Create `path` **exclusively** (O_EXCL): fails if it already exists or is a
/// symlink, so an attacker cannot pre-plant the temp as a link to another file.
/// On unix the file is created `0o600` from the start (never broader-readable for
/// an instant); on Windows the inherited ACL is tightened to owner-only
/// immediately, before any sensitive bytes are written.
pub fn create_new_private(path: &Path) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let f = opts.open(path)?;
    #[cfg(windows)]
    restrict_acl(path);
    Ok(f)
}

/// Create `dir` (and parents) if missing, locked to the owner: `0o700` on unix,
/// inheritance-stripped owner-only ACL on Windows. No-op if it already exists.
pub fn create_dir_private(dir: &Path) -> io::Result<()> {
    if dir.exists() {
        return Ok(());
    }
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    #[cfg(windows)]
    restrict_acl(dir);
    Ok(())
}

/// Restrict `path` to the current user only, stripping inheritance, on Windows
/// (where `0o600` does not exist). Best-effort: a failure must never lose the
/// caller's data, so the result is ignored. `icacls` is resolved to its absolute
/// System32 path so a planted `icacls.exe` on the CWD/PATH can't run instead.
#[cfg(windows)]
pub fn restrict_acl(path: &Path) {
    let user = std::env::var("USERNAME").unwrap_or_default();
    if user.is_empty() {
        return;
    }
    // Resolve icacls.exe to an absolute, untamperable System32 path; if that fails,
    // SKIP the tighten rather than run a bare `icacls` off PATH — a missing ACL
    // restriction is a smaller risk than executing a planted binary in the user's
    // context during a vault/prefs/history write.
    let Some(icacls) = icacls_path() else {
        return;
    };
    let _ = std::process::Command::new(icacls)
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{user}:(F)"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Absolute `…\System32\icacls.exe` resolved via the Win32 `GetSystemDirectoryW`
/// API (NOT the tamperable `%SystemRoot%` env var — the same CWE-426 hardening as
/// the ssh-client trust anchor). `None` if the system directory can't be resolved,
/// in which case the caller skips the ACL tighten rather than trusting `PATH`.
#[cfg(windows)]
fn icacls_path() -> Option<std::path::PathBuf> {
    crate::os::binaries::system_directory().map(|d| d.join("icacls.exe"))
}

/// Best-effort directory fsync so a rename into it is persisted, not just
/// buffered. On unix this is the documented durability step; on Windows `std`
/// can't open a directory handle for flush, so NTFS metadata journaling is relied
/// on. Never propagated — the rename has already succeeded by the call site.
pub fn fsync_parent_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_name_is_unique_and_shaped() {
        let a = temp_name(".cfg").unwrap();
        let b = temp_name(".cfg").unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with(".cfg."), "got {a}");
        assert!(a.ends_with(".tmp"), "got {a}");
    }

    #[test]
    fn create_new_private_rejects_existing() {
        let dir = std::env::temp_dir().join(temp_name(".sfdir").unwrap());
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x");
        let _f = create_new_private(&p).unwrap();
        let err = create_new_private(&p).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn icacls_path_resolves_under_the_real_system_dir() {
        // The icacls trust anchor must resolve via the hardened GetSystemDirectoryW
        // path (like M3 for ssh), NOT the tamperable %SystemRoot% env — so a planted
        // icacls.exe via a poisoned SystemRoot can't run during a vault/prefs write.
        let p = icacls_path().expect("icacls path resolves");
        assert!(p.ends_with("icacls.exe"), "got {p:?}");
        assert!(p.exists(), "resolved icacls.exe should exist");
        assert!(
            p.to_string_lossy().to_lowercase().contains("system32"),
            "got {p:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn create_new_private_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(temp_name(".sfperm").unwrap());
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("y");
        let _f = create_new_private(&p).unwrap();
        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = fs::remove_dir_all(&dir);
    }
}
