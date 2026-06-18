//! Resolving a host's effective SSH config via `ssh -G`, plus the
//! arming gates (Match-exec pre-scan, TOFU known-hosts check) that decide
//! whether connect-time secret auto-fill may run for a host.
//!
//! Zero ratatui / zero `App` dependency. Phase 2 of vault auto-fill; the values
//! resolved here are consumed by the connect wiring in Phase 3.

// Phase 2 builds this module standalone; its items are wired into the connect
// path in Phase 3. Until then the binary (non-test) build sees them as unused.
// TODO(phase3): remove this once the connect path references this module.
#![allow(dead_code)]

use std::io;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::binaries::tools;

/// The effective connection identity for an alias, parsed from `ssh -G`.
/// `ssh -G` lowercases keys and leaves IdentityFile `~`/`%`-tokens UNexpanded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedConfig {
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<String>,
    pub host_key_alias: Option<String>,
    pub identity_files: Vec<String>,
    pub proxy_jump: Option<String>,
    pub proxy_command: Option<String>,
    pub user_known_hosts_files: Vec<String>,
    pub global_known_hosts_files: Vec<String>,
}

/// Split a possibly-quoted space-separated path list (as `ssh -G` emits for
/// the known-hosts file options). Handles double-quoted tokens with spaces.
fn split_quoted_paths(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut tok = String::new();
        if c == '"' {
            chars.next(); // opening quote
            for ch in chars.by_ref() {
                if ch == '"' {
                    break;
                }
                tok.push(ch);
            }
        } else {
            while let Some(&ch) = chars.peek() {
                if ch.is_whitespace() {
                    break;
                }
                tok.push(ch);
                chars.next();
            }
        }
        if !tok.is_empty() {
            out.push(tok);
        }
    }
    out
}

fn strip_one_quote(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

/// Parse `ssh -G <alias>` output into a [`ResolvedConfig`]. Keys are lowercase;
/// each line is `key value`. Unknown keys are ignored. `proxyjump`/`proxycommand`
/// of `none` are treated as no proxy.
pub fn parse_ssh_g_output(dump: &str) -> ResolvedConfig {
    let mut rc = ResolvedConfig::default();
    for line in dump.lines() {
        let line = line.trim();
        let Some((key, val)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let val = val.trim();
        if val.is_empty() {
            continue;
        }
        match key {
            "hostname" => rc.hostname = Some(val.to_string()),
            "user" => rc.user = Some(val.to_string()),
            "port" => rc.port = Some(val.to_string()),
            "hostkeyalias" => rc.host_key_alias = Some(val.to_string()),
            "identityfile" => rc.identity_files.push(strip_one_quote(val)),
            "proxyjump" if !val.eq_ignore_ascii_case("none") => {
                rc.proxy_jump = Some(val.to_string())
            }
            "proxycommand" if !val.eq_ignore_ascii_case("none") => {
                rc.proxy_command = Some(val.to_string())
            }
            "userknownhostsfile" => rc.user_known_hosts_files = split_quoted_paths(val),
            "globalknownhostsfile" => rc.global_known_hosts_files = split_quoted_paths(val),
            _ => {}
        }
    }
    rc
}

/// Upper bound on how long the `ssh -G` resolve may take before we kill it and
/// degrade to manual entry. Sized for the common case; a hanging `Match exec`
/// or slow DNS must not wedge the caller.
pub const SSH_G_RESOLVE_TIMEOUT: Duration = Duration::from_millis(500);

/// Run `ssh -G <alias>` (default config, matching the connect path) on a bounded
/// subprocess and parse the result. `stdin` is nulled so it can never block on a
/// prompt; on timeout the child is killed and an error returned (caller degrades
/// to manual entry / no auto-fill).
pub fn resolve_config(alias: &str) -> io::Result<ResolvedConfig> {
    run_ssh_g(&[alias.to_string()])
}

/// Shared bounded runner. `extra` is the argument list AFTER `-G` (production
/// passes just the alias; tests may prepend `-F <fixture>`).
fn run_ssh_g(extra: &[String]) -> io::Result<ResolvedConfig> {
    let mut child = Command::new(&tools().ssh)
        .arg("-G")
        .args(extra)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                return Err(io::Error::other("ssh -G exited non-zero"));
            }
            break;
        }
        if start.elapsed() >= SSH_G_RESOLVE_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(io::ErrorKind::TimedOut, "ssh -G timed out"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let output = child.wait_with_output()?;
    let dump = String::from_utf8_lossy(&output.stdout);
    Ok(parse_ssh_g_output(&dump))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_config_default_is_empty() {
        let rc = ResolvedConfig::default();
        assert!(rc.hostname.is_none());
        assert!(rc.identity_files.is_empty());
    }

    #[test]
    fn parses_core_fields() {
        let dump = "\
host web1
hostname 10.0.0.5
user deploy
port 2222
hostkeyalias web1ka
identityfile ~/.ssh/id_ed25519
identityfile ~/.ssh/id_rsa
";
        let rc = parse_ssh_g_output(dump);
        assert_eq!(rc.hostname.as_deref(), Some("10.0.0.5"));
        assert_eq!(rc.user.as_deref(), Some("deploy"));
        assert_eq!(rc.port.as_deref(), Some("2222"));
        assert_eq!(rc.host_key_alias.as_deref(), Some("web1ka"));
        assert_eq!(
            rc.identity_files,
            vec!["~/.ssh/id_ed25519".to_string(), "~/.ssh/id_rsa".to_string()]
        );
    }

    #[test]
    fn proxy_none_is_no_proxy() {
        let rc = parse_ssh_g_output("proxyjump none\nproxycommand none\n");
        assert!(rc.proxy_jump.is_none());
        assert!(rc.proxy_command.is_none());
        let rc2 = parse_ssh_g_output("proxyjump bastion\nproxycommand ssh -W %h:%p jump\n");
        assert_eq!(rc2.proxy_jump.as_deref(), Some("bastion"));
        assert_eq!(rc2.proxy_command.as_deref(), Some("ssh -W %h:%p jump"));
    }

    #[test]
    fn known_hosts_files_split_and_unquote() {
        let rc = parse_ssh_g_output(
            "userknownhostsfile ~/.ssh/known_hosts ~/.ssh/known_hosts2\nglobalknownhostsfile /etc/ssh/ssh_known_hosts\n",
        );
        assert_eq!(
            rc.user_known_hosts_files,
            vec![
                "~/.ssh/known_hosts".to_string(),
                "~/.ssh/known_hosts2".to_string()
            ]
        );
        assert_eq!(
            rc.global_known_hosts_files,
            vec!["/etc/ssh/ssh_known_hosts".to_string()]
        );

        let q =
            parse_ssh_g_output("userknownhostsfile \"/path with space/kh\" ~/.ssh/known_hosts\n");
        assert_eq!(
            q.user_known_hosts_files,
            vec![
                "/path with space/kh".to_string(),
                "~/.ssh/known_hosts".to_string()
            ]
        );
    }

    #[test]
    fn ignores_blank_and_keyless_lines() {
        let rc = parse_ssh_g_output("\nhostname h\nbogusline\n   \n");
        assert_eq!(rc.hostname.as_deref(), Some("h"));
    }

    #[test]
    fn resolve_config_returns_a_hostname_for_any_alias() {
        // `ssh -G <alias>` always succeeds (an unknown alias resolves hostname to
        // itself). Requires ssh on PATH (CLAUDE.md guarantees it; CI has it).
        let rc = resolve_config("sshm-test-nonexistent-alias")
            .expect("ssh -G should succeed for any alias");
        // hostname defaults to the alias when not in config.
        assert_eq!(rc.hostname.as_deref(), Some("sshm-test-nonexistent-alias"));
        // ssh -G always emits at least one identityfile default.
        assert!(!rc.identity_files.is_empty());
        // user defaults to the OS account.
        assert!(rc.user.is_some());
    }
}
