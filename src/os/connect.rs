//! Building and launching `ssh` connections.
//!
//! Saved hosts connect via `ssh <alias>` so OpenSSH reads the very `~/.ssh/config`
//! we write — the file is the single source of truth. Flags are emitted only for
//! ad-hoc, unsaved [`ConnectOverrides`].

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::config::model::HostView;

#[cfg(windows)]
use super::binaries::find_wt;
use super::binaries::tools;

/// Ad-hoc overrides applied to a single connection without touching the file.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
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
    // End-of-options sentinel (CWE-88): a config-controlled alias beginning with
    // '-' must never be parsed by ssh as an option. Mirrors the hardened
    // `ssh -G -- <alias>` resolve path (os/resolve.rs). Applies uniformly to the
    // inline, new-tab, and "copy command" paths, which all build on this.
    a.push("--".into());
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
            // Quote on any whitespace (not just U+0020) so a tab-bearing value
            // pastes back as a single argument, matching how the forward/extras
            // parsers split on any whitespace.
            if p.chars().any(char::is_whitespace) {
                format!("\"{p}\"")
            } else {
                p.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The subset of override flags that change how `ssh -G` *resolves* an alias —
/// user, port, identity, ProxyJump and explicit `-o` options. Forwarding (`-L`/
/// `-R`/`-D`) and `-v` don't affect resolution and are omitted. Passing these to
/// `ssh -G` makes the connect-time vault gates (target identity, TOFU key) key
/// off the *effective* `user@host` of an override connect, not the saved config.
pub fn resolve_options(ov: &ConnectOverrides) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    if let Some(u) = &ov.user {
        a.push("-l".into());
        a.push(u.clone());
    }
    if let Some(p) = ov.port {
        a.push("-p".into());
        a.push(p.to_string());
    }
    if let Some(id) = &ov.identity_file {
        a.push("-i".into());
        a.push(id.display().to_string());
        a.push("-o".into());
        a.push("IdentitiesOnly=yes".into());
    }
    if let Some(j) = &ov.proxy_jump
        && !j.is_empty()
    {
        a.push("-J".into());
        a.push(j.clone());
    }
    for (k, v) in &ov.extra_options {
        a.push("-o".into());
        a.push(format!("{k}={v}"));
    }
    a
}

/// Run `ssh` with inherited stdio in the current console and wait for it. The
/// caller is responsible for suspending/restoring the TUI around this call.
pub fn run_ssh_inline(
    args: &[String],
    env: &[(OsString, OsString)],
) -> io::Result<std::process::ExitStatus> {
    Command::new(&tools().ssh)
        .args(args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
}

/// A short human-facing summary of an inline ssh exit status.
pub fn describe_exit(status: &std::process::ExitStatus) -> Option<(String, bool)> {
    describe_exit_code(status.code())
}

/// The exit-code half of [`describe_exit`], split out so the connect-time auto-fill
/// outcome toast can reuse the exact same wording from a raw code without
/// constructing a platform-specific `ExitStatus` (and so it is unit-testable on
/// every OS). `None` means "no toast" (a clean exit 0).
pub fn describe_exit_code(code: Option<i32>) -> Option<(String, bool)> {
    match code {
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
pub fn connect_new_tab(
    alias: &str,
    args: &[String],
    env: &[(OsString, OsString)],
) -> io::Result<()> {
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
    // Whether the tab's ssh inherits this env from wt.exe is the open spike
    // question; connect dispatch passes &[] for new-tab in v1 (auto-fill gated
    // off), so this is a no-op until the spike confirms inheritance.
    cmd.envs(env.iter().map(|(k, v)| (k, v)));
    cmd.spawn().map(|_| ())
}

/// Best-effort new-window connect on non-Windows hosts.
#[cfg(not(windows))]
pub fn connect_new_tab(
    _alias: &str,
    args: &[String],
    env: &[(OsString, OsString)],
) -> io::Result<()> {
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
        cmd.envs(env.iter().map(|(k, v)| (k, v)));
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
        assert_eq!(
            build_ssh_args(&h, &ConnectOverrides::default()),
            ["--", "web1"]
        );
    }

    #[test]
    fn dash_alias_is_after_end_of_options() {
        let h = host("-oProxyCommand=calc");
        let args = build_ssh_args(&h, &ConnectOverrides::default());
        // The alias can only ever appear AFTER the `--` sentinel, so ssh can
        // never parse it as an option (CWE-88).
        let sep = args.iter().position(|a| a == "--").expect("`--` present");
        assert_eq!(args[sep + 1], "-oProxyCommand=calc");
        assert_eq!(sep, args.len() - 2);
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
                "--",
                "deploy@web1",
            ]
        );
    }

    #[test]
    fn resolve_options_emits_resolution_flags_only() {
        // Forwards and verbose are irrelevant to `ssh -G` and must be omitted.
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
        assert_eq!(
            resolve_options(&ov),
            [
                "-l",
                "deploy",
                "-p",
                "2222",
                "-i",
                "/k/id",
                "-o",
                "IdentitiesOnly=yes",
                "-J",
                "bastion",
                "-o",
                "ForwardAgent=yes",
            ]
        );
        assert!(resolve_options(&ConnectOverrides::default()).is_empty());
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
