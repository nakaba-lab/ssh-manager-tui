//! Building and launching `ssh` connections.
//!
//! Saved hosts connect via `ssh <alias>` so OpenSSH reads the very `~/.ssh/config`
//! we write — the file is the single source of truth. Flags are emitted only for
//! ad-hoc, unsaved [`ConnectOverrides`].

use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::config::model::HostView;

#[cfg(windows)]
use super::binaries::find_wt;
use super::binaries::tools;

/// Ad-hoc overrides applied to a single connection without touching the file.
#[derive(Default, Clone)]
pub struct ConnectOverrides {
    pub port: Option<u16>,
    pub user: Option<String>,
    pub identity_file: Option<PathBuf>,
    pub proxy_jump: Option<String>,
    pub local_forwards: Vec<String>,
    pub remote_forwards: Vec<String>,
    pub dynamic_forwards: Vec<String>,
    pub extra_options: Vec<(String, String)>,
    pub verbose: bool,
}

/// Build the `ssh` argument vector for a host. With default overrides this is
/// just `[alias]`; flags are appended only for non-default override fields.
pub fn build_ssh_args(host: &HostView, ov: &ConnectOverrides) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();

    if let Some(id) = &ov.identity_file {
        a.push("-i".into());
        a.push(id.display().to_string());
        a.push("-o".into());
        a.push("IdentitiesOnly=yes".into());
    }
    if let Some(p) = ov.port {
        a.push("-p".into());
        a.push(p.to_string());
    }
    if let Some(j) = &ov.proxy_jump
        && !j.is_empty()
    {
        a.push("-J".into());
        a.push(j.clone());
    }
    for f in &ov.local_forwards {
        a.push("-L".into());
        a.push(f.clone());
    }
    for f in &ov.remote_forwards {
        a.push("-R".into());
        a.push(f.clone());
    }
    for f in &ov.dynamic_forwards {
        a.push("-D".into());
        a.push(f.clone());
    }
    for (k, v) in &ov.extra_options {
        a.push("-o".into());
        a.push(format!("{k}={v}"));
    }
    if ov.verbose {
        a.push("-v".into());
    }

    let dest = match &ov.user {
        Some(u) => format!("{u}@{}", host.alias()),
        None => host.alias().to_string(),
    };
    a.push(dest);
    a
}

/// Human-readable `ssh ...` command line (for the "copy command" action).
pub fn command_line(host: &HostView, ov: &ConnectOverrides) -> String {
    let mut parts = vec!["ssh".to_string()];
    parts.extend(build_ssh_args(host, ov));
    parts
        .iter()
        .map(|p| {
            if p.contains(' ') {
                format!("\"{p}\"")
            } else {
                p.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Run `ssh` with inherited stdio in the current console and wait for it. The
/// caller is responsible for suspending/restoring the TUI around this call.
pub fn run_ssh_inline(args: &[String]) -> io::Result<std::process::ExitStatus> {
    Command::new(&tools().ssh)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
}

/// A short human-facing summary of an inline ssh exit status.
pub fn describe_exit(status: &std::process::ExitStatus) -> Option<(String, bool)> {
    match status.code() {
        Some(0) => None,
        Some(255) => Some(("ssh: connection or authentication failed".into(), true)),
        Some(code) => Some((format!("ssh exited with code {code}"), false)),
        None => Some(("ssh terminated by signal".into(), true)),
    }
}

/// Escape one argument for `wt.exe`'s own re-parsing of its trailing command
/// line: quote on whitespace, escape `;` (a wt command delimiter), and double
/// embedded quotes. Backslashes are left intact so Windows paths survive.
#[cfg(any(windows, test))]
pub fn escape_wt_arg(s: &str) -> String {
    let needs_quote = s.is_empty() || s.chars().any(|c| c.is_whitespace());
    let mut t = String::new();
    for c in s.chars() {
        match c {
            ';' => {
                t.push('\\');
                t.push(';');
            }
            '"' => {
                t.push('"');
                t.push('"');
            }
            _ => t.push(c),
        }
    }
    if needs_quote { format!("\"{t}\"") } else { t }
}

/// Open the connection in a new Windows Terminal tab (Windows). Fire-and-forget;
/// the TUI keeps running. Returns an error to surface as a toast if `wt.exe` is
/// unavailable.
#[cfg(windows)]
pub fn connect_new_tab(alias: &str, args: &[String]) -> io::Result<()> {
    let wt =
        find_wt().ok_or_else(|| io::Error::other("wt.exe not found — use Enter for inline"))?;
    let ssh = tools().ssh.to_string_lossy().to_string();

    let mut cmd = Command::new(wt);
    cmd.arg("-w")
        .arg("0")
        .arg("new-tab")
        .arg("--title")
        .arg(escape_wt_arg(&format!("ssh: {alias}")));
    cmd.arg(escape_wt_arg(&ssh));
    for a in args {
        cmd.arg(escape_wt_arg(a));
    }
    cmd.spawn().map(|_| ())
}

/// Best-effort new-window connect on non-Windows hosts.
#[cfg(not(windows))]
pub fn connect_new_tab(_alias: &str, args: &[String]) -> io::Result<()> {
    let terminals = [
        "x-terminal-emulator",
        "wezterm",
        "alacritty",
        "kitty",
        "gnome-terminal",
    ];
    for term in terminals {
        let mut cmd = Command::new(term);
        if term == "gnome-terminal" {
            cmd.arg("--");
        } else {
            cmd.arg("-e");
        }
        cmd.arg("ssh");
        cmd.args(args);
        if cmd.spawn().is_ok() {
            return Ok(());
        }
    }
    Err(io::Error::other(
        "no terminal emulator found for new-tab connect",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(alias: &str) -> HostView {
        HostView {
            patterns: vec![alias.to_string()],
            ..Default::default()
        }
    }

    #[test]
    fn default_args_is_just_alias() {
        let h = host("web1");
        assert_eq!(build_ssh_args(&h, &ConnectOverrides::default()), ["web1"]);
    }

    #[test]
    fn overrides_emit_flags() {
        let h = host("web1");
        let ov = ConnectOverrides {
            port: Some(2222),
            user: Some("deploy".into()),
            identity_file: Some(PathBuf::from("/k/id")),
            proxy_jump: Some("bastion".into()),
            local_forwards: vec!["8080 localhost:80".into()],
            remote_forwards: vec!["9090 localhost:90".into()],
            dynamic_forwards: vec!["1080".into()],
            extra_options: vec![("ForwardAgent".into(), "yes".into())],
            verbose: true,
        };
        let args = build_ssh_args(&h, &ov);
        assert_eq!(
            args,
            [
                "-i",
                "/k/id",
                "-o",
                "IdentitiesOnly=yes",
                "-p",
                "2222",
                "-J",
                "bastion",
                "-L",
                "8080 localhost:80",
                "-R",
                "9090 localhost:90",
                "-D",
                "1080",
                "-o",
                "ForwardAgent=yes",
                "-v",
                "deploy@web1",
            ]
        );
    }

    #[test]
    fn wt_escaping() {
        assert_eq!(escape_wt_arg("web1"), "web1");
        assert_eq!(escape_wt_arg("a b"), "\"a b\"");
        assert_eq!(escape_wt_arg("a;b"), "a\\;b");
        assert_eq!(
            escape_wt_arg("C:\\OpenSSH\\ssh.exe"),
            "C:\\OpenSSH\\ssh.exe"
        );
    }
}
