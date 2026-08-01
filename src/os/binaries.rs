//! Resolution of the OpenSSH binaries we shell out to.
//!
//! On Windows, the bare name `ssh` on `PATH` often resolves to the Git/MSYS
//! build, which interprets `~/.ssh/config`, `-J` and forwards differently. We
//! therefore prefer the System32 OpenSSH binaries and only fall back to the
//! PATH name (raising a warning) when they are absent.

use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct SshTools {
    pub ssh: PathBuf,
    pub ssh_keygen: PathBuf,
    /// Resolved `ssh-add`, used to inspect and mutate the ssh-agent (#49).
    /// Preferred from System32 OpenSSH on Windows for the same reason as `ssh`:
    /// a Git/MSYS `ssh-add` may address a different agent than the `ssh` we
    /// connect with, which would make the "loaded" badge describe the wrong one.
    pub ssh_add: PathBuf,
    /// Resolved `ssh-keyscan` (retained for a future "scan host key" action).
    #[allow(dead_code)]
    pub ssh_keyscan: PathBuf,
    /// Resolved `sftp`, used to launch interactive SFTP sessions. Like `ssh` it
    /// is preferred from System32 OpenSSH on Windows so it reads `~/.ssh/config`
    /// and honours `SSH_ASKPASS` the same way.
    pub sftp: PathBuf,
    /// True when the resolved binaries are the System32 OpenSSH build (Windows)
    /// or any PATH lookup on a non-Windows host.
    pub is_system32: bool,
}

static TOOLS: OnceLock<SshTools> = OnceLock::new();

/// Resolve (once) the SSH toolchain to use for this session.
pub fn tools() -> &'static SshTools {
    TOOLS.get_or_init(resolve)
}

/// The real Windows system directory (`…\System32`) resolved via the Win32
/// `GetSystemDirectoryW` API — independent of the attacker-tamperable `%SystemRoot%`
/// env var (CWE-426 hardening of the auto-fill trust anchor; see [`resolve`]).
/// `None` if the API call fails or reports an implausibly long path.
#[cfg(windows)]
pub(crate) fn system_directory() -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

    // MAX_PATH; the system directory (e.g. `C:\Windows\System32`) is always well
    // under this, so a single sizing-safe call suffices.
    let mut buf = [0u16; 260];
    // SAFETY: GetSystemDirectoryW writes at most `buf.len()` UTF-16 code units into
    // `buf` and returns the count written (excluding the NUL), the required size if
    // the buffer were too small, or 0 on failure.
    let len = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) };
    if len == 0 || len as usize > buf.len() {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(&buf[..len as usize])))
}

#[cfg(windows)]
fn resolve() -> SshTools {
    // Resolve System32 via the Win32 API, NOT the %SystemRoot% env var. That var is
    // attacker-tamperable (HKCU\Environment or a crafted process environment), and
    // `is_system32` now gates releasing a decrypted vault password to the resolved
    // ssh/sftp through SSH_ASKPASS — so a redirected anchor would disclose the vault
    // password to a planted binary (CWE-426). Fall back to the conventional literal
    // only if the API fails; if that path has no ssh.exe, `is_system32` stays false
    // (fail-safe: PATH ssh, no auto-fill).
    let base = system_directory()
        .unwrap_or_else(|| PathBuf::from("C:\\Windows\\System32"))
        .join("OpenSSH");
    let ssh = base.join("ssh.exe");
    if ssh.is_file() {
        return SshTools {
            ssh,
            ssh_keygen: base.join("ssh-keygen.exe"),
            ssh_add: base.join("ssh-add.exe"),
            ssh_keyscan: base.join("ssh-keyscan.exe"),
            sftp: base.join("sftp.exe"),
            is_system32: true,
        };
    }
    // Accepted residual (#8 scope): when System32 OpenSSH is absent (Git-for-
    // Windows-only installs), fall back to bare names resolved by the OS executable
    // search. This is surfaced to the user as the `[PATH ssh]` warning
    // (`is_system32: false` drives `App::ssh_path_warning`); the always-on `icacls`
    // spawn was the one absolutized. A planted ssh on the CWD/PATH is the residual.
    SshTools {
        ssh: PathBuf::from("ssh"),
        ssh_keygen: PathBuf::from("ssh-keygen"),
        ssh_add: PathBuf::from("ssh-add"),
        ssh_keyscan: PathBuf::from("ssh-keyscan"),
        sftp: PathBuf::from("sftp"),
        is_system32: false,
    }
}

#[cfg(not(windows))]
fn resolve() -> SshTools {
    SshTools {
        ssh: PathBuf::from("ssh"),
        ssh_keygen: PathBuf::from("ssh-keygen"),
        ssh_add: PathBuf::from("ssh-add"),
        ssh_keyscan: PathBuf::from("ssh-keyscan"),
        sftp: PathBuf::from("sftp"),
        is_system32: true,
    }
}

/// `~/.ssh` directory, resolved from the real home directory (never `$HOME`
/// from an MSYS shell).
pub fn ssh_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".ssh"))
}

/// Locate `wt.exe` (Windows Terminal): the WindowsApps shim, else bare PATH.
#[cfg(windows)]
pub fn find_wt() -> Option<PathBuf> {
    if let Some(local) = dirs::data_local_dir() {
        let p = local.join("Microsoft").join("WindowsApps").join("wt.exe");
        if p.is_file() {
            return Some(p);
        }
    }
    Some(PathBuf::from("wt.exe"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_add_is_resolved_alongside_ssh_not_from_bare_path() {
        // #49: `ssh-add` must inherit the same System32-preference as `ssh`.
        // Resolving it from the bare PATH would let a Git/MSYS ssh-add talk to a
        // different agent than the System32 `ssh` we actually connect with, so
        // the "loaded" badge would describe an agent the connection never uses.
        let t = tools();
        assert_eq!(
            t.ssh_add.file_stem().and_then(|s| s.to_str()),
            Some("ssh-add"),
            "ssh_add should resolve to an ssh-add binary, got {:?}",
            t.ssh_add
        );
        assert_eq!(
            t.ssh_add.parent(),
            t.ssh.parent(),
            "ssh-add must come from the same directory as ssh ({:?} vs {:?})",
            t.ssh_add,
            t.ssh
        );
    }

    #[cfg(windows)]
    #[test]
    fn system_directory_ignores_a_tampered_systemroot_env() {
        // The auto-fill trust anchor (is_system32) must resolve System32 via the
        // Win32 API, NOT the %SystemRoot% env var — that var is attacker-tamperable
        // (HKCU\Environment / a crafted process environment), and is_system32 now
        // gates releasing a decrypted vault password to the resolved ssh/sftp, so a
        // redirected anchor would disclose the vault password to a planted binary
        // (CWE-426). Point SystemRoot at an attacker dir and confirm the resolved
        // system directory is UNCHANGED and real.
        let baseline = system_directory().expect("system dir");
        let orig = std::env::var_os("SystemRoot");
        // SAFETY: test-only, restored immediately below. Production resolves the
        // directory via GetSystemDirectoryW (never this var), so the brief window
        // cannot mislead a parallel test's trust decision.
        unsafe { std::env::set_var("SystemRoot", "C:\\attacker\\evil") };
        let tampered = system_directory().expect("system dir");
        match orig {
            Some(v) => unsafe { std::env::set_var("SystemRoot", v) },
            None => unsafe { std::env::remove_var("SystemRoot") },
        }
        assert_eq!(
            baseline, tampered,
            "system directory must not depend on %SystemRoot%"
        );
        assert!(tampered.exists(), "resolved system directory must exist");
        assert!(
            tampered
                .to_string_lossy()
                .to_lowercase()
                .ends_with("system32"),
            "GetSystemDirectoryW should resolve the System32 directory, got {tampered:?}"
        );
    }
}
