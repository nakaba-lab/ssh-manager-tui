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
use super::known_hosts::{KnownHostEntry, parse_line};

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
    /// True when the host's trust decisions come from a source this code cannot
    /// read: `KnownHostsCommand` (OpenSSH ≥ 8.5 — supplies known-hosts lines
    /// from a program, honoured for host-key verification) or
    /// `VerifyHostKeyDNS yes` (a DNSSEC-signed SSHFP authenticates the host
    /// without any known_hosts entry).
    ///
    /// `ssh-keygen -F` sees neither, so the keyscan writer would believe an
    /// established host is unpinned and append beside that trust path — proven
    /// end-to-end for `KnownHostsCommand` (#46 round 8). It refuses instead.
    pub has_external_trust_source: bool,
    /// True when a known-hosts file list could NOT be split back into paths
    /// without losing information: the raw `ssh -G` value contained a run of
    /// two-or-more whitespace characters, or a tab.
    ///
    /// `ssh -G` prints the list separated by exactly one space and does NOT
    /// collapse whitespace inside a path (verified against OpenSSH 9.6p1), so a
    /// longer run is content — but [`split_quoted_paths`] splits on any
    /// whitespace and drops empties, which discards where the boundaries were.
    /// Readers merely lose entries (fail-safe); the keyscan writer would pick a
    /// wrong file while believing it had the whole picture, so it refuses to
    /// scan when this is set (#46 round 6).
    pub known_hosts_list_lossy: bool,
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

/// True when [`split_quoted_paths`] would discard information about where the
/// path boundaries were.
///
/// `ssh -G` separates entries with exactly ONE space and preserves whitespace
/// inside a path, so the split is lossless exactly when re-joining the pieces
/// with single spaces reproduces the value byte for byte. Asking the splitter
/// itself is the point: a hand-written scanner is a SECOND model of the split,
/// and the two disagreed — the previous one exempted quoted segments (which
/// `split_quoted_paths` only honours at the start of a token) and ran on a
/// trimmed value, so a stray `"` or a trailing run slipped past it while the
/// splitter still mangled the path (#46 round 7).
///
/// Must be given the RAW value: leading and trailing runs are as unrecoverable
/// as interior ones.
fn splitting_loses_information(raw: &str) -> bool {
    split_quoted_paths(raw).join(" ") != raw
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
        // Trailing whitespace is NOT trimmed here: a known-hosts path may end
        // in a space or tab, and that byte is exactly what decides whether the
        // list can be split back into paths (#46 round 7). Only a CRLF's `\r`
        // is dropped, since it is never part of a value.
        let line = line.trim_start().trim_end_matches('\r');
        let Some((key, val)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        // `raw` is the value with NOTHING trimmed — `split_once` consumed
        // exactly the one separator space `ssh -G` emits, so any whitespace
        // still attached belongs to a path. `val` is the trimmed form every
        // other option wants.
        let raw = val;
        let val = raw.trim();
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
            // `ssh -G` omits `knownhostscommand` entirely when unset, and
            // prints `none` when explicitly disabled.
            "knownhostscommand" if !val.eq_ignore_ascii_case("none") => {
                rc.has_external_trust_source = true;
            }
            // `ssh -G` renders this through OpenSSH's multistate table, which
            // prints `true`/`false`/`ask` — never the `yes`/`no` the config
            // uses. Matching only `yes` made this arm dead code (#46 round 9).
            // `ask` is deliberately NOT flagged: with it, trust does not exist
            // without user interaction and the accepted key lands in
            // known_hosts, so "no matching entry" really does mean "not trusted".
            "verifyhostkeydns" if matches!(val, "true" | "yes") => {
                rc.has_external_trust_source = true;
            }
            // Kerberos-authenticated key exchange (the Debian/Ubuntu GSSAPI
            // patch): the server is authenticated without any host key, so a
            // host in daily use legitimately has no known_hosts entry and a pin
            // would create a host-key fallback that did not exist before.
            "gssapikeyexchange" if matches!(val, "true" | "yes") => {
                rc.has_external_trust_source = true;
            }
            "userknownhostsfile" => {
                rc.known_hosts_list_lossy |= splitting_loses_information(raw);
                rc.user_known_hosts_files = split_quoted_paths(raw);
            }
            "globalknownhostsfile" => {
                rc.known_hosts_list_lossy |= splitting_loses_information(raw);
                rc.global_known_hosts_files = split_quoted_paths(raw);
            }
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
/// Resolve an alias to its effective config, optionally with ad-hoc override
/// flags applied so the resolution reflects the *effective* target of an override
/// connect (a changed user/port/identity/proxy shifts the `user@host` the vault
/// gates key off). The `options` are trusted leading flags — production builds
/// them from validated form input via `os::connect::resolve_options`; a plain
/// saved-host connect passes `&[]`. The untrusted `alias` always follows `--`.
pub fn resolve_config_with_options(options: &[String], alias: &str) -> io::Result<ResolvedConfig> {
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
    run_ssh_g(options, alias)
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
        // A leading `!` NEGATES a criterion but still evaluates it — OpenSSH
        // runs `Match !exec "cmd"` just as it runs `Match exec "cmd"` (verified).
        // Comparing the undecorated word missed it entirely (#46 round 10).
        if words.any(|w| w.trim_start_matches('!').eq_ignore_ascii_case("exec")) {
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

/// True iff `lookup_key` has a genuine TOFU pin — a marker-free, non-wildcard
/// `known_hosts` entry (plain OR HMAC-hashed) — in any of `files`. Uses
/// `ssh-keygen -F`, so hashed entries (`HashKnownHosts yes`, the Debian/Ubuntu
/// default) match too. A `@revoked` / `@cert-authority` / wildcard / negation
/// match does NOT count — auto-fill must only arm for a host the user pinned.
/// Every `known_hosts` entry OpenSSH itself matches for `lookup_key` across
/// `files`, markers included, via `ssh-keygen -F`. Delegating the match to
/// OpenSSH is the point: hashed lines (`HashKnownHosts yes`), wildcard
/// patterns and case-insensitive hostname comparison are all handled by the
/// same matcher the connection will use, which hand-rolled token comparison
/// gets wrong (#46 review). Callers classify on the returned `key_type` /
/// `key_b64` / `marker`, never on the host field — the returned entries are
/// UNFILTERED, so a caller that needs "is this a genuine pin" must apply the
/// marker / [`KnownHostEntry::is_pattern`] checks itself (as [`is_host_known`]
/// does).
pub fn matching_known_entries(lookup_key: &str, files: &[String]) -> Vec<KnownHostEntry> {
    resolve_known_hosts_files(files)
        .iter()
        .flat_map(|file| entries_in_file(lookup_key, file))
        .collect()
}

/// True when our reading of the reported words would DROP a real file — i.e.
/// the words can be read as EITHER separate paths OR one space-bearing path,
/// and the greedy reading loses a file that exists on disk.
///
/// `coalesce_existing_paths` resolves that ambiguity greedily, joining the
/// longest run that exists — so a file literally named `"<pathA> <pathB>"`
/// swallows `pathB` and every later word in the run, hiding the pins in them
/// while the write still lands in a file ssh reads (#46 round 11, reproduced
/// end-to-end). The reader can afford the greedy guess (it only loses entries);
/// the writer cannot, so it refuses when the input is ambiguous at all.
///
/// The question is asked DIRECTLY — "does either partition drop a file?" —
/// rather than by reasoning about one run at a time. Every run-local rule tried
/// here was wrong: keying on the first word's existence missed a swallow whose
/// run starts with an absent word (#46 round 12); a word-level swallow test
/// missed a genuine path that itself contains a space (#46 round 13); and
/// testing only the sub-runs INSIDE a joined run missed a genuine file
/// straddling its right edge (#46 round 14). All three are the same failure —
/// a real file our reading does not produce — so that is what is tested.
///
/// Only REGULAR files count: a directory holds no entries, so a sibling
/// account directory must not make the space-bearing Windows home ambiguous.
pub fn known_hosts_paths_are_ambiguous(files: &[String]) -> bool {
    let expanded: Vec<String> = files.iter().map(|p| expand_known_hosts_path(p)).collect();
    // Both coalescing predicates in use must be checked: the readers join when
    // the joined path exists; the writer also joins when its parent directory
    // exists (a first pin names a file that does not exist yet). Checking only
    // one lets an attacker pick whichever partition the checker ignores
    // (#46 round 12).
    let reader = coalesce_existing_paths(&expanded, |p| std::path::Path::new(p).exists());
    let writer = coalesce_existing_paths(&expanded, |p| {
        let path = std::path::Path::new(p);
        path.exists()
            || path
                .parent()
                .is_some_and(|d| !d.as_os_str().is_empty() && d.is_dir())
    });
    let is_file = |p: &str| std::fs::metadata(p).is_ok_and(|m| m.is_file());
    (0..expanded.len()).any(|k| {
        (k..expanded.len()).any(|l| {
            let candidate = expanded[k..=l].join(" ");
            is_file(&candidate) && (!reader.contains(&candidate) || !writer.contains(&candidate))
        })
    })
}

pub fn has_unresolvable_known_hosts_file(files: &[String]) -> bool {
    files.iter().any(|p| {
        p.starts_with('~')
            || has_percent_token(p)
            || expand_known_hosts_path(p).contains("__PROGRAMDATA__")
    })
}

/// True for an UNEXPANDED `%`-token (`%d`, `%u`, …), not for a literal `%` in a
/// filename: `ssh -G` expands user-file tokens, so `/tmp/50%off/known_hosts` is
/// a perfectly readable path and refusing it would be a false negative
/// (#46 round 5). The token letters are ssh_config's TOKENS for these options.
fn has_percent_token(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.iter().enumerate().any(|(i, &c)| {
        c == b'%'
            && bytes
                .get(i + 1)
                .is_some_and(|n| b"CdhiklLnpruj%".contains(n))
    })
}

/// The first known-hosts file `ssh -G` reported, as a real path — the file a
/// new entry should be written to.
///
/// This cannot just take `files[0]`: `ssh -G` prints the list space-separated
/// and UNQUOTED, so a path containing a space (the Windows default under
/// `C:\Users\First Last\`) arrives pre-split and `files[0]` is a truncated
/// prefix. Writing there creates a stray file OpenSSH never reads (#46
/// re-review). Coalescing keys off the parent directory rather than the file
/// itself, because the first-pin case is exactly when the file does not exist.
///
/// Returns `None` when the list resolves to no writable file — including the
/// `none` sentinel, which OpenSSH documents as "read no user file" and `ssh -G`
/// emits verbatim. Treating `none` as a filename would create a junk file in
/// the process's CWD and report success for a pin OpenSSH never reads.
pub fn primary_known_hosts_file(files: &[String]) -> Option<std::path::PathBuf> {
    if is_none_file_list(files) {
        return None;
    }
    let expanded: Vec<String> = files.iter().map(|p| expand_known_hosts_path(p)).collect();
    let writable = |p: &str| {
        let path = std::path::Path::new(p);
        path.exists()
            || path
                .parent()
                .is_some_and(|d| !d.as_os_str().is_empty() && d.is_dir())
    };
    let coalesced = coalesce_existing_paths(&expanded, writable);
    // EVERY member must look writable, not just the one chosen. Coalescing
    // degrades to single words when a run matches nothing, and the degraded
    // members are exactly the truncated prefixes of an unrecoverable path — the
    // `…/a  b/known_hosts` (double-space) case, where `ssh -G` collapsed the run
    // and no reconstruction is possible. Writing to such a prefix creates a
    // stray file OpenSSH never reads, and the post-write check cannot catch it
    // because the prefix is itself a member of the read set (#46 round 5).
    if coalesced.is_empty() || !coalesced.iter().all(|p| writable(p)) {
        return None;
    }
    coalesced.into_iter().next().map(std::path::PathBuf::from)
}

/// True when a known-hosts file list is OpenSSH's `none` sentinel ("use no
/// file").
///
/// Tested against a WHOLE list from ONE option, never per element and never on
/// a merged list. OpenSSH's "argument must appear alone" rule is per option, so
/// `UserKnownHostsFile none` yields a merged (user + global) list of length 3 —
/// checking the merged list misses the sentinel, while filtering per element
/// would drop a path component of a space-bearing directory name (e.g.
/// `/home/me/my none dir/known_hosts`, which `ssh -G` emits unquoted and split)
/// and hide a genuine pin. Both mistakes have been shipped once each (#46
/// rounds 4 and 5), so callers must pass `rc.user_known_hosts_files` and
/// `rc.global_known_hosts_files` separately.
pub fn is_none_file_list(files: &[String]) -> bool {
    files.len() == 1 && files[0] == "none"
}

/// Shared file-list normalization for the known-hosts readers: expand the
/// Windows `__PROGRAMDATA__` token, then coalesce `ssh -G`'s unquoted,
/// space-split list back into paths that exist.
fn resolve_known_hosts_files(files: &[String]) -> Vec<String> {
    // No `none` handling here: this receives the MERGED user+global list, where
    // a bare `none` element is a path component, not the sentinel. Dropping the
    // sentinel is the caller's job, per option (see [`is_none_file_list`]).
    let expanded: Vec<String> = files.iter().map(|p| expand_known_hosts_path(p)).collect();
    // Readers coalesce on the file itself (a file that does not exist holds no
    // entries anyway); the writer's variant keys off the parent directory
    // instead, because it has to name a file that does not exist yet.
    coalesce_existing_paths(&expanded, |p| std::path::Path::new(p).exists())
}

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
    resolve_known_hosts_files(files)
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

/// The entries `ssh-keygen -F` reports for `lookup_key` in one file, parsed but
/// unfiltered (markers and wildcard hosts included — callers decide). Empty on
/// any failure: exit 1 = not found / file missing.
fn entries_in_file(lookup_key: &str, file: &str) -> Vec<KnownHostEntry> {
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
        Err(_) => return Vec::new(),
    };
    if !output.status.success() {
        return Vec::new();
    }
    // ssh-keygen -F prints `# Host <key> found: line N` comments plus the
    // matching line(s); `parse_line` drops the comments.
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|l| parse_line(l, 0))
        .collect()
}

fn known_in_file(lookup_key: &str, file: &str) -> bool {
    // Accept only a marker-free, non-wildcard entry — but a HASHED match (the
    // line printed as `|1|…`) is a legitimate single-host pin and MUST count,
    // otherwise `HashKnownHosts yes` (the Debian/Ubuntu default) would silently
    // defeat the whole gate.
    // ssh-keygen never hashes wildcards/negations or markers (those survive in
    // plaintext), so a marker-free, non-pattern hit — hashed or plain — is
    // always a genuine per-host pin.
    entries_in_file(lookup_key, file)
        .iter()
        .any(|e| e.marker.is_none() && !e.is_pattern())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_sentinel_is_judged_per_option_never_per_element() {
        // given — the sentinel, exactly as OpenSSH allows it (alone in ONE
        // option's list)
        let sentinel = vec!["none".to_string()];
        // when / then — recognised, and nothing to write
        assert!(is_none_file_list(&sentinel));
        assert_eq!(primary_known_hosts_file(&sentinel), None);

        // given — a MERGED user+global list carrying the sentinel. OpenSSH's
        // "argument must appear alone" rule is per option, so after merging the
        // sentinel is just one element of a longer list and must NOT be
        // recognised here — the caller drops it per option instead.
        let merged = vec![
            "none".to_string(),
            "/etc/ssh/ssh_known_hosts".to_string(),
            "/etc/ssh/ssh_known_hosts2".to_string(),
        ];
        assert!(!is_none_file_list(&merged));

        // given — a directory literally named `none` inside a space-bearing
        // path, which `ssh -G` emits UNQUOTED and therefore pre-split. Dropping
        // the `none` element would break the path apart and hide a real pin.
        let dir = std::env::temp_dir().join(format!("sshm-none-{}", std::process::id()));
        let nested = dir.join("my none dir");
        std::fs::create_dir_all(&nested).unwrap();
        let kh = nested.join("known_hosts");
        std::fs::write(&kh, "db.example ssh-ed25519 AAAA\n").unwrap();
        let split: Vec<String> = kh
            .to_str()
            .unwrap()
            .split(' ')
            .map(str::to_string)
            .collect();
        assert!(split.contains(&"none".to_string()), "fixture must split");
        // when / then — the path is rejoined intact, not filtered apart
        assert!(!is_none_file_list(&split));
        assert_eq!(primary_known_hosts_file(&split), Some(kh.clone()));
        assert_eq!(
            resolve_known_hosts_files(&split),
            vec![kh.to_str().unwrap().to_string()]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_swallowing_file_name_makes_the_list_ambiguous() {
        // given — three reported files, the pin in the LAST one, plus a file
        // literally named "<f2> <f3>". Greedy coalescing joins the longest run
        // that exists, so it swallows f3 and the pin becomes invisible while
        // the write still lands in f1 — a file ssh really reads (#46 round 11).
        let dir = std::env::temp_dir().join(format!("sshm-swallow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = |n: &str| dir.join(n).to_str().unwrap().to_string();
        for n in ["f1", "f2", "f3"] {
            std::fs::write(dir.join(n), "").unwrap();
        }
        let list = vec![f("f1"), f("f2"), f("f3")];
        // without the swallow file the list reads one way only
        assert!(!known_hosts_paths_are_ambiguous(&list));

        // when — a file literally NAMED "<f2> <f3>" exists (its own path
        // contains a space, so its parent directory is `<f2> <dir>`)
        let swallow = std::path::PathBuf::from(format!("{} {}", f("f2"), f("f3")));
        std::fs::create_dir_all(swallow.parent().unwrap()).unwrap();
        std::fs::write(&swallow, "").unwrap();
        // then — ambiguous, and the writer must refuse rather than guess
        assert!(known_hosts_paths_are_ambiguous(&list));

        // and the space-bearing Windows home stays UNambiguous: neither word is
        // a real FILE on its own, so the join is the only reading
        let win = vec![f("First"), "Last/.ssh/known_hosts".to_string()];
        assert!(!known_hosts_paths_are_ambiguous(&win));
        // even when a sibling account DIRECTORY exists — a directory holds no
        // entries, so it must not disable the feature (#46 round 12)
        std::fs::create_dir_all(dir.join("First")).unwrap();
        assert!(!known_hosts_paths_are_ambiguous(&win));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ambiguity_covers_a_swallow_whose_first_word_is_absent() {
        // given — the run the reader would join starts with a word that does
        // NOT exist. Coalescing joins on whether the JOIN exists, so requiring
        // the first word to exist missed this entirely, and the swallowed
        // file's pins went invisible while the write target stayed a real file
        // (#46 round 12, reproduced end-to-end).
        let dir = std::env::temp_dir().join(format!("sshm-swallow2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = |n: &str| dir.join(n).to_str().unwrap().to_string();
        std::fs::write(dir.join("f1"), "").unwrap();
        std::fs::write(dir.join("f3"), "h ssh-ed25519 GENUINE\n").unwrap();
        // `g` is absent on purpose
        let list = vec![f("f1"), f("g"), f("f3")];
        assert!(!known_hosts_paths_are_ambiguous(&list));

        // when — a file named "<g> <f3>" exists
        let swallow = std::path::PathBuf::from(format!("{} {}", f("g"), f("f3")));
        std::fs::create_dir_all(swallow.parent().unwrap()).unwrap();
        std::fs::write(&swallow, "").unwrap();
        // then — ambiguous: f3 is a real file the join would swallow
        assert!(known_hosts_paths_are_ambiguous(&list));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ambiguity_covers_a_swallow_of_a_multi_word_path() {
        // given — the genuine pin lives in a path that ITSELF contains a space,
        // so `ssh -G` reports it as two words. Asking whether any single WORD is
        // a file missed this: none of the words is a file on its own, yet the
        // join swallows the real (multi-word) file and its pins (#46 round 13,
        // reproduced end-to-end against real OpenSSH).
        let dir = std::env::temp_dir().join(format!("sshm-swallow4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = |n: &str| dir.join(n).to_str().unwrap().to_string();
        std::fs::write(dir.join("kh"), "").unwrap();
        // the genuine, space-bearing pin file; `team` is absent on purpose
        std::fs::write(dir.join("my hosts"), "h ssh-ed25519 GENUINE\n").unwrap();
        let list = vec![f("kh"), f("team"), f("my"), "hosts".to_string()];
        assert!(!known_hosts_paths_are_ambiguous(&list));

        // when — a file named "<team> <my> hosts" exists, so the reader joins
        // words 1..=3 and the genuine "<dir>/my hosts" disappears
        let swallow = std::path::PathBuf::from(format!("{} {} hosts", f("team"), f("my")));
        std::fs::create_dir_all(swallow.parent().unwrap()).unwrap();
        std::fs::write(&swallow, "").unwrap();
        // then — ambiguous: a CONTIGUOUS SUB-RUN of the join is a real file
        assert!(known_hosts_paths_are_ambiguous(&list));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ambiguity_covers_a_swallow_that_overlaps_the_genuine_path() {
        // given — the same shape as round 13, except the attacker's file
        // OVERLAPS the genuine multi-word path instead of containing it. Asking
        // "is some sub-run of the joined run a file" only looked INSIDE the run,
        // so a genuine file straddling the run's right edge stayed invisible
        // while coalescing destroyed it just the same (#46 round 14, reproduced
        // end-to-end against real OpenSSH).
        let dir = std::env::temp_dir().join(format!("sshm-swallow5-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = |n: &str| dir.join(n).to_str().unwrap().to_string();
        std::fs::write(dir.join("kh"), "").unwrap();
        // the genuine, space-bearing pin file spans words 2..=3; `team` is absent
        let genuine = std::path::PathBuf::from(format!("{} {}", f("my"), f("hosts")));
        std::fs::create_dir_all(genuine.parent().unwrap()).unwrap();
        std::fs::write(&genuine, "h ssh-ed25519 GENUINE\n").unwrap();
        let list = vec![f("kh"), f("team"), f("my"), f("hosts")];
        assert!(!known_hosts_paths_are_ambiguous(&list));

        // when — the attacker's file spans words 1..=2, overlapping (not
        // containing) the genuine 2..=3
        let swallow = std::path::PathBuf::from(format!("{} {}", f("team"), f("my")));
        std::fs::create_dir_all(swallow.parent().unwrap()).unwrap();
        std::fs::write(&swallow, "").unwrap();
        // then — ambiguous: our reading no longer yields the genuine file at all
        assert!(known_hosts_paths_are_ambiguous(&list));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ambiguity_covers_the_writers_parent_dir_join() {
        // given — two real files, and only the DIRECTORY that the joined name
        // would live in. The readers do not join (the join does not exist) but
        // the writer does (its parent is a dir), so the write went to a stray
        // path while the post-write check re-read it and reported success
        // (#46 round 12).
        let dir = std::env::temp_dir().join(format!("sshm-swallow3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = |n: &str| dir.join(n).to_str().unwrap().to_string();
        std::fs::write(dir.join("f1"), "").unwrap();
        std::fs::write(dir.join("f2"), "").unwrap();
        let list = vec![f("f1"), f("f2")];
        assert!(!known_hosts_paths_are_ambiguous(&list));

        // when — the attacker creates the directory the join would sit in
        let join = std::path::PathBuf::from(format!("{} {}", f("f1"), f("f2")));
        std::fs::create_dir_all(join.parent().unwrap()).unwrap();
        // then — ambiguous under the writer's predicate too
        assert!(known_hosts_paths_are_ambiguous(&list));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn primary_known_hosts_file_refuses_an_unrecoverable_split() {
        // given — a directory whose name contains TWO spaces. `ssh -G` collapses
        // whitespace runs, so the path can never be reconstructed; coalescing
        // degrades to single words and the first is a truncated prefix.
        let dir = std::env::temp_dir().join(format!("sshm-dbl-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("a  b")).unwrap();
        let collapsed = vec![
            dir.join("a").to_str().unwrap().to_string(),
            "b/known_hosts".to_string(),
        ];
        // when / then — refuse rather than write to `<dir>/a`, which OpenSSH
        // never reads and which the post-write check cannot flag (the stray
        // prefix is itself a member of the read set)
        assert_eq!(primary_known_hosts_file(&collapsed), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn percent_token_detection_ignores_a_literal_percent_in_a_filename() {
        // given / when / then — unexpanded tokens must be refused
        assert!(has_unresolvable_known_hosts_file(&[
            "%d/.ssh/known_hosts".into()
        ]));
        assert!(has_unresolvable_known_hosts_file(&["~/gkh".into()]));
        // but a literal `%` in a real filename resolves fine — `ssh -G` expands
        // user-file tokens, so refusing it would be a false negative
        assert!(!has_unresolvable_known_hosts_file(&[
            "/tmp/50%off/known_hosts".into()
        ]));
        assert!(!has_unresolvable_known_hosts_file(&[
            "/home/me/.ssh/known_hosts".into()
        ]));
    }

    #[test]
    fn unresolvable_known_hosts_files_are_reported() {
        // given / when / then — `ssh -G` leaves an explicitly-set global file's
        // `~`/`%` tokens unexpanded; deciding pinnability on a partial view of
        // the pins would be fail-open, so callers must be able to detect it
        assert!(has_unresolvable_known_hosts_file(&[
            "/home/me/.ssh/known_hosts".into(),
            "~/gkh".into()
        ]));
        assert!(has_unresolvable_known_hosts_file(&[
            "%d/.ssh/known_hosts".into()
        ]));
        assert!(!has_unresolvable_known_hosts_file(&[
            "/home/me/.ssh/known_hosts".into(),
            "/etc/ssh/ssh_known_hosts".into()
        ]));
    }

    #[test]
    fn known_hosts_list_lossy_is_flagged_for_unsplittable_paths() {
        // given / when / then — `ssh -G` separates entries with ONE space and
        // keeps whitespace inside a path, so a longer run (or a tab) is content
        // our splitter cannot put back
        for lossy in [
            "userknownhostsfile /d/known_hosts /d/kh  2",
            "userknownhostsfile /d/known  hosts",
            "userknownhostsfile /d/a\tb/known_hosts",
            "globalknownhostsfile /etc/ssh/a  b",
        ] {
            assert!(
                parse_ssh_g_output(lossy).known_hosts_list_lossy,
                "should be lossy: {lossy}"
            );
        }
        // and ordinary configurations must NOT be flagged
        for ok in [
            "userknownhostsfile /root/.ssh/known_hosts /root/.ssh/known_hosts2",
            "userknownhostsfile /c/Users/First Last/.ssh/known_hosts",
            "userknownhostsfile none",
            "userknownhostsfile /home/me/my none dir/known_hosts",
            // a literal quote is not a separator: `ssh -G` splices quotes away
            // and only ever emits single-space separators
            "userknownhostsfile /d/kh1 /d/q\"p/kh",
            // CRLF output (Windows) must not flag every line
            "userknownhostsfile /d/kh1\r",
        ] {
            assert!(
                !parse_ssh_g_output(ok).known_hosts_list_lossy,
                "should not be lossy: {ok}"
            );
        }
    }

    #[test]
    fn external_trust_sources_are_detected() {
        // given / when / then — trust that `ssh-keygen -F` cannot see. Treating
        // "no matching entry" as "unpinned" here would let a scan append beside
        // an established trust path (#46 round 8, reproduced for
        // KnownHostsCommand end-to-end).
        // NOTE: the values here are the ones REAL `ssh -G` prints. An earlier
        // version of this test used `verifyhostkeydns yes`, which `ssh -G`
        // never emits (it renders the multistate as `true`/`false`/`ask`), so
        // the test passed while the guard was dead code (#46 round 9).
        // NOTE: `ssh -G` renders `verifyhostkeydns` as true/false/ask but
        // `gssapikeyexchange` as yes/no — the values below are the ones each
        // option really prints. A test using a value ssh -G cannot produce
        // passes while the guard is dead code (#46 rounds 9-10).
        for dump in [
            "knownhostscommand /usr/local/bin/kh.sh",
            "verifyhostkeydns true",
            "gssapikeyexchange yes",
        ] {
            assert!(
                parse_ssh_g_output(dump).has_external_trust_source,
                "should flag: {dump}"
            );
        }
        // and the disabled / default forms must NOT flag (`ssh -G` omits
        // knownhostscommand entirely when unset, and prints `none` when off)
        for dump in [
            "knownhostscommand none",
            "verifyhostkeydns false",
            // `ask` prompts the user and still records the key in known_hosts,
            // so "no matching entry" genuinely means "not yet trusted"
            "verifyhostkeydns ask",
            "gssapikeyexchange no",
            "hostname db.example",
        ] {
            assert!(
                !parse_ssh_g_output(dump).has_external_trust_source,
                "should not flag: {dump}"
            );
        }
    }

    #[test]
    fn match_exec_detects_the_negated_form() {
        // given / when / then — OpenSSH RUNS the predicate for `!exec` too
        assert!(has_match_exec("Match !exec \"/bin/true\""));
        assert!(has_match_exec("Match all !exec=/bin/true"));
        assert!(has_match_exec("Match exec \"/bin/true\""));
        assert!(!has_match_exec("Match host db.example"));
    }

    #[test]
    fn known_hosts_list_lossy_catches_runs_the_trim_used_to_eat() {
        // given — a path ENDING in whitespace. Trimming the value before
        // testing it deleted the very bytes that decide splittability, so the
        // list silently lost its last path's tail (#46 round 7).
        for lossy in [
            "userknownhostsfile /d/kh1 /d/kh2  ",
            "userknownhostsfile /d/kh1 /d/kh2\t",
            // ...and a leading run, for the same reason
            "userknownhostsfile  /d/kh1",
        ] {
            assert!(
                parse_ssh_g_output(lossy).known_hosts_list_lossy,
                "should be lossy: {lossy:?}"
            );
        }

        // given — a stray literal quote followed by a real run. The previous
        // scanner toggled an in-quotes state on any `"` and stopped noticing
        // runs, while the splitter (which only honours a quote at the START of
        // a token) still mangled the path.
        let masked = "userknownhostsfile /d/kh1 /d/q\"p  /d/s/kh";
        assert!(
            parse_ssh_g_output(masked).known_hosts_list_lossy,
            "a quote must not mask a later run"
        );
    }

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
}
