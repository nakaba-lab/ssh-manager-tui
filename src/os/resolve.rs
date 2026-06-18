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
use super::known_hosts::{HostSpec, parse_line};

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
    // Defense against argv flag-smuggling: an alias beginning with '-' would be
    // parsed by ssh as an option (e.g. `-oProxyCommand=...` → code execution).
    // No legitimate SSH host alias starts with '-' (plain `ssh <alias>` could
    // not use one either), so reject it outright rather than resolve it.
    if alias.starts_with('-') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "alias may not start with '-'",
        ));
    }
    run_ssh_g(&[], alias)
}

/// Shared bounded runner: `ssh -G <options> -- <alias>`. `options` are trusted
/// leading flags (production passes none; tests may pass `-F <fixture>`); the
/// untrusted `alias` always follows the `--` end-of-options sentinel so it can
/// never be interpreted as a flag, even if an upstream caller skips validation.
fn run_ssh_g(options: &[String], alias: &str) -> io::Result<ResolvedConfig> {
    let mut child = Command::new(&tools().ssh)
        .arg("-G")
        .args(options)
        .arg("--")
        .arg(alias)
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

/// True if the raw SSH config text contains a `Match` line with an `exec`
/// criterion. Conservative + syntax-only: it never runs anything. Used to skip
/// `ssh -G` entirely for such hosts (which would otherwise execute the predicate).
pub fn has_match_exec(config_text: &str) -> bool {
    for line in config_text.lines() {
        let line = line.trim_start();
        if line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        if !words
            .next()
            .is_some_and(|w| w.eq_ignore_ascii_case("Match"))
        {
            continue;
        }
        if words.any(|w| w.eq_ignore_ascii_case("exec")) {
            return true;
        }
    }
    false
}

/// The `known_hosts` lookup key for a resolved host, matching OpenSSH:
/// `HostKeyAlias` verbatim if set; else `hostname` when the port is 22 (or
/// unset); else `[hostname]:port`. `None` if no hostname resolved.
pub fn tofu_lookup_key(rc: &ResolvedConfig) -> Option<String> {
    if let Some(ka) = &rc.host_key_alias {
        return Some(ka.clone());
    }
    let host = rc.hostname.as_deref()?;
    let port = rc.port.as_deref().unwrap_or("22");
    if port == "22" {
        Some(host.to_string())
    } else {
        Some(format!("[{host}]:{port}"))
    }
}

/// True iff `lookup_key` has a PLAIN (no marker, no wildcard/negation)
/// `known_hosts` entry in any of `files`. Uses `ssh-keygen -F` (hashed-aware).
/// A `@revoked` / `@cert-authority` / wildcard match does NOT count — auto-fill
/// must only arm for a host that is genuinely TOFU-pinned.
pub fn is_host_known(lookup_key: &str, files: &[String]) -> bool {
    files.iter().any(|file| known_in_file(lookup_key, file))
}

fn known_in_file(lookup_key: &str, file: &str) -> bool {
    let output = match Command::new(&tools().ssh_keygen)
        .arg("-F")
        .arg(lookup_key)
        .arg("-f")
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        Ok(o) => o,
        Err(_) => return false,
    };
    if !output.status.success() {
        return false; // exit 1 = not found / file missing
    }
    // ssh-keygen -F prints `# Host <key> found: line N` comments plus the
    // matching line(s). Accept only a plain, marker-free, non-wildcard entry.
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().filter_map(|l| parse_line(l, 0)).any(|e| {
        e.marker.is_none()
            && matches!(e.host, HostSpec::Plain(ref h) if !h.contains(['*', '?', '!']))
    })
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

    #[test]
    fn resolve_config_rejects_leading_dash_alias() {
        // argv flag-smuggling guard: a `-`-prefixed alias must be refused, not
        // handed to `ssh`, where it would be parsed as an option.
        let err = resolve_config("-oProxyCommand=evil").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn detects_match_exec() {
        assert!(has_match_exec(
            "Match exec \"test -e /tmp/x\"\n  User bob\n"
        ));
        assert!(has_match_exec("match   Exec  uptime\n")); // case/space insensitive
        assert!(has_match_exec("Host a\nMatch host b exec \"cmd\"\n")); // exec as a later criterion
    }

    #[test]
    fn ignores_non_exec_match_and_comments() {
        assert!(!has_match_exec("Match host web1\n  User bob\n"));
        assert!(!has_match_exec("# Match exec \"cmd\"\n")); // commented out
        assert!(!has_match_exec("Host exec-server\n  HostName x\n")); // 'exec' in a value, not a Match
        assert!(!has_match_exec(""));
    }

    #[test]
    fn lookup_key_rules() {
        let base = ResolvedConfig {
            hostname: Some("10.0.0.5".into()),
            port: Some("22".into()),
            ..Default::default()
        };
        assert_eq!(tofu_lookup_key(&base).as_deref(), Some("10.0.0.5"));

        let p2222 = ResolvedConfig {
            hostname: Some("10.0.0.5".into()),
            port: Some("2222".into()),
            ..Default::default()
        };
        assert_eq!(tofu_lookup_key(&p2222).as_deref(), Some("[10.0.0.5]:2222"));

        let ka = ResolvedConfig {
            hostname: Some("10.0.0.5".into()),
            port: Some("2222".into()),
            host_key_alias: Some("web1ka".into()),
            ..Default::default()
        };
        assert_eq!(tofu_lookup_key(&ka).as_deref(), Some("web1ka")); // verbatim, ignores port

        let no_host = ResolvedConfig::default();
        assert_eq!(tofu_lookup_key(&no_host), None);

        let no_port = ResolvedConfig {
            hostname: Some("h".into()),
            port: None,
            ..Default::default()
        };
        assert_eq!(tofu_lookup_key(&no_port).as_deref(), Some("h")); // missing port = default 22
    }

    #[test]
    fn is_known_accepts_plain_rejects_marker_and_absent() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("sshm-tofu-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let kh = dir.join("known_hosts");
        let mut f = std::fs::File::create(&kh).unwrap();
        // plain entry for good.example, a @revoked entry for bad.example,
        // a @cert-authority wildcard for *.ca.example.
        writeln!(f, "good.example ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAA").unwrap();
        writeln!(
            f,
            "@revoked bad.example ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBBB"
        )
        .unwrap();
        writeln!(
            f,
            "@cert-authority *.ca.example ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICCC"
        )
        .unwrap();
        drop(f);
        let khs = vec![kh.to_string_lossy().to_string()];

        assert!(
            is_host_known("good.example", &khs),
            "plain entry should be known"
        );
        assert!(
            !is_host_known("bad.example", &khs),
            "@revoked must not count as known"
        );
        assert!(
            !is_host_known("ca.example", &khs),
            "@cert-authority wildcard must not count"
        );
        assert!(
            !is_host_known("absent.example", &khs),
            "absent host is not known"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
