//! Resolving a host's effective SSH config via `ssh -G`, plus the
//! arming gates (Match-exec pre-scan, TOFU known-hosts check) that decide
//! whether connect-time secret auto-fill may run for a host.
//!
//! Zero ratatui / zero `App` dependency. Phase 2 of vault auto-fill; the values
//! resolved here are consumed by the connect wiring in Phase 3.

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

/// Parse `ssh -G` output into an ordered list of EVERY `key value` pair, in
/// emission order — keeping keys outside the typed [`ResolvedConfig`] subset and
/// repeated keys (e.g. multiple `identityfile`). Blank and keyless (whitespace-
/// less) lines are dropped. Powers the effective-config inspector (#43); pure,
/// zero ratatui / `App`. Keys are kept VERBATIM as `ssh -G` emits them (mostly
/// lowercase, but a few like `canonicalizePermittedcnames` stay camelCase).
pub fn parse_ssh_g_full(dump: &str) -> Vec<(String, String)> {
    dump.lines()
        .filter_map(|line| {
            let (key, val) = line.trim().split_once(char::is_whitespace)?;
            Some((key.to_string(), val.trim().to_string()))
        })
        .collect()
}

/// Upper bound on how long the `ssh -G` resolve may take before we kill it and
/// degrade to manual entry. Sized for the common case; a hanging `Match exec`
/// or slow DNS must not wedge the caller.
pub const SSH_G_RESOLVE_TIMEOUT: Duration = Duration::from_millis(500);

/// Run `ssh -G <alias>` (default config, matching the connect path) on a bounded
/// subprocess and parse the result. `stdin` is nulled so it can never block on a
/// prompt; on timeout the child is killed and an error returned (caller degrades
/// to manual entry / no auto-fill).
/// Resolve an alias to its effective config, optionally with ad-hoc override
/// flags applied so the resolution reflects the *effective* target of an override
/// connect (a changed user/port/identity/proxy shifts the `user@host` the vault
/// gates key off). The `options` are trusted leading flags — production builds
/// them from validated form input via `os::connect::resolve_options`; a plain
/// saved-host connect passes `&[]`. The untrusted `alias` always follows `--`.
pub fn resolve_config_with_options(options: &[String], alias: &str) -> io::Result<ResolvedConfig> {
    reject_dash_alias(alias)?;
    run_ssh_g(options, alias)
}

/// Argv flag-smuggling guard shared by both resolvers: an alias beginning with
/// '-' would be parsed by ssh as an option (e.g. `-oProxyCommand=...` → code
/// execution). No legitimate SSH host alias starts with '-' (plain `ssh <alias>`
/// could not use one either), so reject it outright rather than resolve it.
fn reject_dash_alias(alias: &str) -> io::Result<()> {
    if alias.starts_with('-') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "alias may not start with '-'",
        ));
    }
    Ok(())
}

/// Resolve an alias to its FULL effective config as an ordered `Vec<(key, value)>`
/// — every `ssh -G` line in emission order, not just the typed [`ResolvedConfig`]
/// subset. Same argv guard and bounded runner as [`resolve_config_with_options`].
/// Powers the effective-config inspector (#43); zero ratatui / `App` dependency.
pub fn resolve_full(options: &[String], alias: &str) -> io::Result<Vec<(String, String)>> {
    reject_dash_alias(alias)?;
    Ok(parse_ssh_g_full(&run_ssh_g_dump(options, alias)?))
}

/// Shared bounded runner returning the raw `ssh -G` stdout dump. `options` are
/// trusted leading flags (production passes none; tests may pass `-F <fixture>`);
/// the untrusted `alias` always follows the `--` end-of-options sentinel so it
/// can never be interpreted as a flag, even if an upstream caller skips
/// validation. `stdin` is nulled so it can never block on a prompt; on timeout
/// the child is killed and an error returned (caller degrades to manual entry).
/// The typed [`run_ssh_g`] and the full [`resolve_full`] view both read this dump.
fn run_ssh_g_dump(options: &[String], alias: &str) -> io::Result<String> {
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
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Typed resolver: run `ssh -G` and parse into the extracted [`ResolvedConfig`]
/// subset the vault gates key off.
fn run_ssh_g(options: &[String], alias: &str) -> io::Result<ResolvedConfig> {
    Ok(parse_ssh_g_output(&run_ssh_g_dump(options, alias)?))
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
        // ssh_config splices double quotes within/around a token (`"Match"`,
        // `Mat"ch"`, `"exec"`, `ex"ec"` all collapse to the bare keyword) and
        // accepts `=` (with optional spaces) as the keyword/argument separator
        // (`Match=exec`, `Match= exec`, `exec="cmd"`). Each of these makes
        // `ssh -G` run the predicate yet evades a naive whitespace split, so
        // remove `"` (splice) and turn `=` into a boundary before scanning.
        // Both transforms only ADD/erase token decoration — never merge two real
        // words — so detection can widen but never narrow. Over-detection just
        // costs a conservative skip of `ssh -G` (fail-safe); a miss would let
        // `ssh -G` execute attacker-influenced shell during connect resolution.
        let normalized = line.replace('"', "").replace('=', " ");
        let mut words = normalized.split_whitespace();
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

/// Why the effective-config inspector (#43) must NOT run `ssh -G` for this
/// config, or `None` if it is safe to. `ssh -G` executes any `Match exec`
/// predicate, so it is refused fail-safe when the config could trigger one:
/// a `Match exec` in the main file, OR ANY `Include` — whose target this scan
/// cannot see and which could itself carry a `Match exec` (Issue #43 risk #1).
/// The `Match exec` reason takes precedence when both hold. `include_count` is
/// the caller's `SshConfig::include_count()`.
pub fn inspect_block_reason(config_text: &str, include_count: usize) -> Option<&'static str> {
    if has_match_exec(config_text) {
        Some("host config uses `Match exec` — ssh -G would run it; inspector skipped")
    } else if include_count > 0 {
        Some("config uses `Include` — can't verify Match exec safety; inspector skipped")
    } else {
        None
    }
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

/// True iff `lookup_key` has a genuine TOFU pin — a marker-free, non-wildcard
/// `known_hosts` entry (plain OR HMAC-hashed) — in any of `files`. Uses
/// `ssh-keygen -F`, so hashed entries (`HashKnownHosts yes`, the Debian/Ubuntu
/// default) match too. A `@revoked` / `@cert-authority` / wildcard / negation
/// match does NOT count — auto-fill must only arm for a host the user pinned.
pub fn is_host_known(lookup_key: &str, files: &[String]) -> bool {
    // `ssh -G` prints the known-hosts file list with `~`/`%` already expanded but
    // UNQUOTED, and on Windows leaves the literal `__PROGRAMDATA__` token. Expand
    // that token, then coalesce the space-split words back into real files by
    // existence (a single path containing a space — e.g. a Windows home under
    // `C:\Users\First Last\` — is otherwise indistinguishable from two paths).
    // KNOWN (fail-safe) GAP: an *explicitly-set* `GlobalKnownHostsFile` with a
    // `~`/`%` token is dumped by `ssh -G` UNexpanded (unlike the user file), so
    // it stat-misses and that file silently never contributes — a host pinned
    // only there won't arm. Rare; never wrongly arms. The default global file
    // (the `__PROGRAMDATA__` form) and all user files are handled.
    let expanded: Vec<String> = files.iter().map(|p| expand_known_hosts_path(p)).collect();
    coalesce_existing_paths(&expanded, |p| std::path::Path::new(p).exists())
        .iter()
        .any(|file| known_in_file(lookup_key, file))
}

/// Expand OpenSSH-for-Windows's literal `__PROGRAMDATA__` prefix (which `ssh -G`
/// does NOT resolve) using `program_data`. No-op for any other path or when
/// `program_data` is unavailable. Split out as a pure fn for testability.
fn expand_program_data(path: &str, program_data: Option<&str>) -> String {
    const TOKEN: &str = "__PROGRAMDATA__";
    match (path.strip_prefix(TOKEN), program_data) {
        (Some(rest), Some(pd)) => format!("{pd}{rest}"),
        _ => path.to_string(),
    }
}

fn expand_known_hosts_path(path: &str) -> String {
    expand_program_data(path, std::env::var("ProgramData").ok().as_deref())
}

/// Coalesce `ssh -G`'s whitespace-joined, UNQUOTED known-hosts file list back
/// into real paths: greedily take the longest leading run of words that names an
/// existing file, so a space-bearing path is rejoined while genuinely separate
/// files stay split. Fail-safe — a run that matches nothing degrades to single
/// words, which `ssh-keygen -F` then stat-misses (host treated unknown). `exists`
/// is injected so the logic is unit-testable without touching the filesystem.
///
/// KNOWN (fail-safe) GAP: the upstream split collapses whitespace runs, and runs
/// are rejoined with a single space, so a path containing a *double* space or a
/// tab won't be reconstructed and that file is skipped (host treated unknown,
/// never wrongly armed). Single-space paths — the realistic Windows case — work.
fn coalesce_existing_paths(words: &[String], exists: impl Fn(&str) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        // Longest run [i..=j] (j descending) that exists on disk.
        match (i..words.len())
            .rev()
            .find(|&j| exists(&words[i..=j].join(" ")))
        {
            Some(j) => {
                out.push(words[i..=j].join(" "));
                i = j + 1;
            }
            None => {
                out.push(words[i].clone());
                i += 1;
            }
        }
    }
    out
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
    // matching line(s). Accept only a marker-free, non-wildcard entry — but a
    // HASHED match (the line printed as `|1|…`) is a legitimate single-host pin
    // and MUST count, otherwise `HashKnownHosts yes` (the Debian/Ubuntu default)
    // would silently defeat the whole gate.
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().filter_map(|l| parse_line(l, 0)).any(|e| {
        e.marker.is_none()
            && match &e.host {
                // ssh-keygen never hashes wildcards/negations or markers (those
                // survive in plaintext and are excluded above/here), so a
                // marker-free hashed hit is always a genuine per-host pin.
                HostSpec::Hashed(_) => true,
                HostSpec::Plain(h) => !h.contains(['*', '?', '!']),
            }
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
        let rc = resolve_config_with_options(&[], "sshm-test-nonexistent-alias")
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
        let err = resolve_config_with_options(&[], "-oProxyCommand=evil").unwrap_err();
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
    fn detects_match_exec_with_equals_separator() {
        // `=` is a valid keyword/argument separator in ssh_config; each of these
        // forms makes `ssh -G` execute the predicate, so the pre-scan must catch
        // them even though a plain whitespace split would not.
        assert!(has_match_exec("Match=exec \"cmd\"\n"));
        assert!(has_match_exec("Match= exec \"cmd\"\n"));
        assert!(has_match_exec("Match exec=\"cmd\"\n"));
        assert!(has_match_exec("Match=exec=\"cmd\"\n"));
    }

    #[test]
    fn detects_match_exec_through_quote_splicing() {
        // ssh_config splices double quotes, so each of these is parsed as
        // `Match exec` and runs the predicate (verified empirically against
        // `ssh -G` on both System32 and MSYS OpenSSH) — the pre-scan must catch
        // them. Quote removal must SPLICE (not split), so `Mat"ch"` -> `Match`.
        assert!(has_match_exec("\"Match\" exec \"cmd\"\n"));
        assert!(has_match_exec("Match \"exec\" \"cmd\"\n"));
        assert!(has_match_exec("\"Match\" \"exec\" \"cmd\"\n"));
        assert!(has_match_exec("Mat\"ch\" exec \"cmd\"\n")); // mid-token splice
        assert!(has_match_exec("Match ex\"ec\" \"cmd\"\n")); // mid-token splice
        assert!(has_match_exec("Match \"exec\"=\"cmd\"\n"));
        // Two separately-quoted fragments are two tokens, NOT a splice, so this
        // is `Match` with criteria `ex` `ec` — no exec criterion, no detection.
        assert!(!has_match_exec("Match \"ex\" \"ec\" \"cmd\"\n"));
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
    fn expand_program_data_handles_windows_token() {
        assert_eq!(
            expand_program_data(
                "__PROGRAMDATA__\\ssh/ssh_known_hosts",
                Some("C:\\ProgramData")
            ),
            "C:\\ProgramData\\ssh/ssh_known_hosts"
        );
        // a non-token path is left untouched
        assert_eq!(
            expand_program_data("/etc/ssh/ssh_known_hosts", Some("C:\\ProgramData")),
            "/etc/ssh/ssh_known_hosts"
        );
        // token present but no ProgramData available -> untouched (fail-safe)
        assert_eq!(
            expand_program_data("__PROGRAMDATA__\\x", None),
            "__PROGRAMDATA__\\x"
        );
    }

    #[test]
    fn coalesce_existing_paths_rejoins_spaced_and_keeps_separate_files() {
        let present: std::collections::HashSet<&str> = [
            "C:/Users/First Last/.ssh/known_hosts",
            "C:/Users/First Last/.ssh/known_hosts2",
            "/a/kh",
            "/b/kh2",
        ]
        .into_iter()
        .collect();
        let exists = |p: &str| present.contains(p);

        // Default config under a spaced home: 4 split words -> 2 real files.
        let words = vec![
            "C:/Users/First".to_string(),
            "Last/.ssh/known_hosts".to_string(),
            "C:/Users/First".to_string(),
            "Last/.ssh/known_hosts2".to_string(),
        ];
        assert_eq!(
            coalesce_existing_paths(&words, exists),
            vec![
                "C:/Users/First Last/.ssh/known_hosts".to_string(),
                "C:/Users/First Last/.ssh/known_hosts2".to_string()
            ]
        );

        // Two ordinary space-free files stay separate.
        let words2 = vec!["/a/kh".to_string(), "/b/kh2".to_string()];
        assert_eq!(
            coalesce_existing_paths(&words2, exists),
            vec!["/a/kh".to_string(), "/b/kh2".to_string()]
        );

        // Nothing exists -> degrade to single words (fail-safe), never panics.
        let words3 = vec!["/x/missing".to_string(), "tail".to_string()];
        assert_eq!(
            coalesce_existing_paths(&words3, |_| false),
            vec!["/x/missing".to_string(), "tail".to_string()]
        );
    }

    #[test]
    fn is_known_accepts_plain_rejects_marker_and_absent() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("sshm-tofu-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let kh = dir.join("known_hosts");
        let mut f = std::fs::File::create(&kh).unwrap();
        // A plain entry, a @revoked marker, a @cert-authority wildcard, and a
        // plain (markerless) wildcard. The marker/wildcard lookups below use a
        // name that actually MATCHES the pattern so `ssh-keygen -F` exits 0 and
        // the entry reaches the marker/wildcard re-parse rejection we assert on.
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
        writeln!(f, "*.wild.example ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIWWW").unwrap();
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
            !is_host_known("host.ca.example", &khs),
            "@cert-authority wildcard match must not count"
        );
        assert!(
            !is_host_known("host.wild.example", &khs),
            "a plain wildcard match must not count"
        );
        assert!(
            !is_host_known("absent.example", &khs),
            "absent host is not known"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_known_accepts_hashed_entry() {
        // The spec's headline workflow: on `HashKnownHosts yes` (the Debian/
        // Ubuntu default) the pin is stored hashed. `ssh-keygen -F` still finds
        // it and prints a `|1|…` line; the gate MUST accept that as known.
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("sshm-tofu-hashed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let kh = dir.join("known_hosts");
        let mut f = std::fs::File::create(&kh).unwrap();
        writeln!(f, "hashme.example ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHHH").unwrap();
        drop(f);
        // Hash the file in place (equivalent to HashKnownHosts yes).
        let st = Command::new(&tools().ssh_keygen)
            .arg("-H")
            .arg("-f")
            .arg(&kh)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(st.success(), "ssh-keygen -H should hash the fixture");
        let khs = vec![kh.to_string_lossy().to_string()];

        assert!(
            is_host_known("hashme.example", &khs),
            "a hashed but genuine pin must count as known"
        );
        assert!(
            !is_host_known("absent.example", &khs),
            "absent host is not known even in a hashed file"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- #43: effective-config inspector (`ssh -G` view) ---

    #[test]
    fn parse_full_preserves_order_and_all_keys() {
        // Unlike `parse_ssh_g_output` (which extracts a typed subset), the
        // inspector must surface EVERY key/value in emission order, including
        // keys outside the typed struct (`forwardagent`) and repeated keys
        // (`identityfile` twice).
        let dump = "\
host web1
hostname 10.0.0.5
user deploy
port 2222
forwardagent no
identityfile ~/.ssh/id_ed25519
identityfile ~/.ssh/id_rsa
";
        let pairs = parse_ssh_g_full(dump);
        assert_eq!(
            pairs,
            vec![
                ("host".to_string(), "web1".to_string()),
                ("hostname".to_string(), "10.0.0.5".to_string()),
                ("user".to_string(), "deploy".to_string()),
                ("port".to_string(), "2222".to_string()),
                ("forwardagent".to_string(), "no".to_string()),
                ("identityfile".to_string(), "~/.ssh/id_ed25519".to_string()),
                ("identityfile".to_string(), "~/.ssh/id_rsa".to_string()),
            ]
        );
    }

    #[test]
    fn parse_full_skips_blank_and_keyless_lines() {
        // Blank lines and keyless (whitespace-less) lines are dropped, matching
        // the typed parser's tolerance; every real `key value` line is kept.
        let pairs = parse_ssh_g_full("\nhostname h\nbogusline\n   \n");
        assert_eq!(pairs, vec![("hostname".to_string(), "h".to_string())]);
    }

    #[test]
    fn resolve_full_returns_ordered_pairs_for_any_alias() {
        // `ssh -G <alias>` always succeeds; the full view must surface many
        // keyword lines, with `hostname` defaulting to the alias itself. Keys are
        // kept VERBATIM as ssh emits them — mostly lowercase but a few stay
        // camelCase (e.g. `canonicalizePermittedcnames`), so we assert only that
        // each key is a single whitespace-free token. Requires ssh on PATH.
        let pairs = resolve_full(&[], "sshm-test-nonexistent-alias")
            .expect("ssh -G should succeed for any alias");
        assert!(pairs.len() > 5, "ssh -G emits many keyword lines");
        assert!(
            pairs
                .iter()
                .any(|(k, v)| k == "hostname" && v == "sshm-test-nonexistent-alias"),
            "hostname defaults to the alias"
        );
        assert!(
            pairs
                .iter()
                .all(|(k, _)| !k.is_empty() && !k.contains(char::is_whitespace)),
            "each key is a single whitespace-free token, preserved verbatim"
        );
    }

    #[test]
    fn resolve_full_rejects_leading_dash_alias() {
        // Same argv flag-smuggling guard as the typed resolver: a `-`-prefixed
        // alias must be refused before ever reaching `ssh`.
        let err = resolve_full(&[], "-oProxyCommand=evil").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn inspect_block_reason_gates_match_exec_and_include() {
        // The inspector must NOT run `ssh -G` when the config could make it
        // execute a `Match exec` predicate: directly in the main file, OR hidden
        // behind ANY `Include` (whose target the main-file scan cannot see).
        assert!(
            inspect_block_reason("Match exec \"cmd\"\nHost a\n", 0).is_some(),
            "a main-file Match exec must block"
        );
        assert!(
            inspect_block_reason("Host a\n  HostName 1.2.3.4\n", 1).is_some(),
            "any Include must block (Match exec could hide inside it)"
        );
        assert!(
            inspect_block_reason("Host a\n  HostName 1.2.3.4\n", 0).is_none(),
            "plain config with no Match exec and no Include is safe"
        );
        let both =
            inspect_block_reason("Match exec \"cmd\"\n", 2).expect("Match exec present must block");
        assert!(
            both.contains("Match exec"),
            "when both hold, the message names the exec risk first"
        );
    }
}
