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
    /// Resolved `ssh-keyscan` (retained for a future "scan host key" action).
    #[allow(dead_code)]
    pub ssh_keyscan: PathBuf,
    /// True when the resolved binaries are the System32 OpenSSH build (Windows)
    /// or any PATH lookup on a non-Windows host.
    pub is_system32: bool,
}

static TOOLS: OnceLock<SshTools> = OnceLock::new();

/// Resolve (once) the SSH toolchain to use for this session.
pub fn tools() -> &'static SshTools {
    TOOLS.get_or_init(resolve)
}

#[cfg(windows)]
fn resolve() -> SshTools {
    let sysroot = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    let base = PathBuf::from(sysroot).join("System32").join("OpenSSH");
    let ssh = base.join("ssh.exe");
    if ssh.is_file() {
        return SshTools {
            ssh,
            ssh_keygen: base.join("ssh-keygen.exe"),
            ssh_keyscan: base.join("ssh-keyscan.exe"),
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
        ssh_keyscan: PathBuf::from("ssh-keyscan"),
        is_system32: false,
    }
}

#[cfg(not(windows))]
fn resolve() -> SshTools {
    SshTools {
        ssh: PathBuf::from("ssh"),
        ssh_keygen: PathBuf::from("ssh-keygen"),
        ssh_keyscan: PathBuf::from("ssh-keyscan"),
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
