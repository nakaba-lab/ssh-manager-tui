# Vault auto-fill — Phase 2 (resolution & gates) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the headless `os/`-layer resolution & gating logic that decides, for a host being connected, its effective identity (`ssh -G`), whether auto-fill may arm (TOFU known-hosts gate, Match-exec pre-scan), and which secret kinds are candidates — all unit-tested in isolation, nothing wired into the connect path yet.

**Architecture:** A new `os/resolve.rs` runs `ssh -G <alias>` on a bounded subprocess and parses the dump into a `ResolvedConfig`; a TOFU gate builds the OpenSSH lookup key and probes `known_hosts` (reusing `os/known_hosts.rs::parse_line`); a pure candidacy matcher in `os/vault.rs` maps a host's patterns to the vault secret kinds it has. These are the building blocks Phase 3 calls from the connect path. Zero ratatui, zero `App` dependency in Phase 2.

**Tech Stack:** Rust (edition 2024). Shells out to the resolved `tools().ssh` / `tools().ssh_keygen` (System32-preferred). No new crates.

**Spec:** `docs/superpowers/specs/2026-06-17-vault-autofill-design.md` (rev 4) — sections "Resolving the effective config — use `ssh -G`", "BLOCKER fix — gate arming on known-host status (TOFU)", "Host ↔ secret matching".

**Builds on:** Phase 1 (`os/askpass.rs`, commits a1c941f..c0f901a). Phase 1's `ConnectSecrets`/`ResolvedIdentity` consume the values Phase 2 resolves, but Phase 2 does NOT modify `os/askpass.rs`.

---

## Scope of this plan (Phase 2 of 4)

**In scope (headless os/ logic, unit-tested):**
- `os/resolve.rs`: `ResolvedConfig`, `parse_ssh_g_output` (pure), `resolve_config` (bounded `ssh -G`), `has_match_exec` (pure config scan).
- TOFU gate (in `os/resolve.rs`): `tofu_lookup_key` (pure), `is_host_known` (`ssh-keygen -F` across the resolved known_hosts files, plain-non-marker check reusing `known_hosts::parse_line`).
- Candidacy matcher (in `os/vault.rs`): `MatchedKinds` + `match_vault_kinds` (pure).

**Deferred to later phases (do NOT build here):**
- **Phase 3:** all connect wiring — running the resolver off the UI thread (worker + mpsc), calling these gates from `connect_selected`, the `PendingConnect` state machine, `Screen::PasswordConfirm`, `App::password_autofill_enabled` opt-in, `App::vault_secret_kinds` (the App wrapper that applies opt-in masking over `match_vault_kinds`), `App::askpass_listeners` ownership + `drain_askpass`, `main.rs` env dispatch, and rejecting `\r`/`\n` secrets on the vault save path.
- **Phase 4:** UI (indicator, password-confirm modal, toasts).

Each task is TDD with complete code. **Every task must keep the existing 97 tests green and pass all gates on BOTH targets:** host `cargo fmt --all -- --check`, host `cargo clippy --all-targets -- -D warnings`, host `cargo test --all`, AND `cargo clippy --target x86_64-unknown-linux-gnu --all-targets -- -D warnings` (the cross-target gate established in Phase 1; it keeps Phase 1's `#[cfg(unix)]` arm compiling). Phase 2 code is platform-neutral, but the cross-gate must stay green.

## File structure (Phase 2)

- **Create `src/os/resolve.rs`** — the resolver + TOFU gate. Internal organization: `ResolvedConfig` struct; `parse_ssh_g_output`; `resolve_config` (+ `SSH_G_RESOLVE_TIMEOUT`); `has_match_exec`; `tofu_lookup_key`; `is_host_known`. A module-level `#![allow(dead_code)]` (with a `TODO(phase3)` comment) because nothing references these until Phase 3 — same forward-declaration mechanism Phase 1 used (a binary crate flags unused `pub` items even when tests use them).
- **Modify `src/os/mod.rs`** — add `pub mod resolve;`.
- **Modify `src/os/vault.rs`** — add `MatchedKinds` + `match_vault_kinds` (+ tests). These are NOT covered by any module allow there, but they are used by their own tests and will be referenced in Phase 3; if clippy flags them as unused in the bin(non-test) build, gate ONLY them with a scoped `#[allow(dead_code)]` + `// TODO(phase3)` (vault.rs has no module-level allow and we must not add one).

No existing behavior changes; everything is additive and unreferenced until Phase 3.

---

## Task 1: Scaffold `os/resolve.rs` + `ResolvedConfig`

**Files:**
- Create: `src/os/resolve.rs`
- Modify: `src/os/mod.rs`

- [ ] **Step 1: Create the module with the struct, the module-level allow, and a smoke test**

Create `src/os/resolve.rs`:

```rust
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_config_default_is_empty() {
        let rc = ResolvedConfig::default();
        assert!(rc.hostname.is_none());
        assert!(rc.identity_files.is_empty());
    }
}
```

- [ ] **Step 2: Register the module**

In `src/os/mod.rs`, add after `pub mod liveness;` (keep the existing grouping):

```rust
pub mod resolve;
```

- [ ] **Step 3: Run the smoke test + both gates**

Run: `cargo test --lib os::resolve::tests::resolved_config_default_is_empty` → PASS.
Then: `cargo fmt --all -- --check`; `cargo clippy --all-targets -- -D warnings`; `cargo test --all` (98 tests: prior 97 + this one); `cargo clippy --target x86_64-unknown-linux-gnu --all-targets -- -D warnings`. All clean.

- [ ] **Step 4: Commit**

```bash
git add src/os/resolve.rs src/os/mod.rs
git commit -m "feat(resolve): scaffold ssh -G resolver module + ResolvedConfig"
```

---

## Task 2: `parse_ssh_g_output` (pure)

`ssh -G` prints `key value` lines, one per line, keys lowercased. `identityfile` repeats (one per file). `userknownhostsfile`/`globalknownhostsfile` are space-separated path lists on one line (paths with spaces are double-quoted). `proxyjump`/`proxycommand` may be absent or `none` (→ treat as no proxy). Verified: `ssh -G` leaves IdentityFile `~`/`%h` etc. UNexpanded and reports `user` as the OS account when unset.

**Files:**
- Modify: `src/os/resolve.rs`

- [ ] **Step 1: Write the failing tests**

```rust
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
    assert_eq!(rc.identity_files, vec!["~/.ssh/id_ed25519".to_string(), "~/.ssh/id_rsa".to_string()]);
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
    assert_eq!(rc.user_known_hosts_files, vec!["~/.ssh/known_hosts".to_string(), "~/.ssh/known_hosts2".to_string()]);
    assert_eq!(rc.global_known_hosts_files, vec!["/etc/ssh/ssh_known_hosts".to_string()]);

    let q = parse_ssh_g_output("userknownhostsfile \"/path with space/kh\" ~/.ssh/known_hosts\n");
    assert_eq!(q.user_known_hosts_files, vec!["/path with space/kh".to_string(), "~/.ssh/known_hosts".to_string()]);
}

#[test]
fn ignores_blank_and_keyless_lines() {
    let rc = parse_ssh_g_output("\nhostname h\nbogusline\n   \n");
    assert_eq!(rc.hostname.as_deref(), Some("h"));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --lib os::resolve::tests::parses os::resolve::tests::proxy os::resolve::tests::known_hosts os::resolve::tests::ignores` → FAIL (`parse_ssh_g_output` not found).

- [ ] **Step 3: Implement `parse_ssh_g_output` + helpers**

```rust
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
            "proxyjump" if !val.eq_ignore_ascii_case("none") => rc.proxy_jump = Some(val.to_string()),
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib os::resolve::tests` → PASS (all). Then the full gate set (host fmt/clippy/test + linux cross-clippy) → clean.

- [ ] **Step 5: Commit**

```bash
git add src/os/resolve.rs
git commit -m "feat(resolve): parse ssh -G output into ResolvedConfig"
```

---

## Task 3: `resolve_config` — bounded `ssh -G` subprocess

`std::process` has no wait-with-timeout, so spawn with `stdin = Stdio::null()` and poll `try_wait()` with a `kill()` on expiry. Use `tools().ssh` (System32-preferred, matching the connect path). Production resolves against ssh's DEFAULT config (no `-F`), exactly as `os/connect.rs` connects via bare `ssh <alias>`. A test-only variant takes extra args so a fixture config can be passed via `-F`.

**Files:**
- Modify: `src/os/resolve.rs`

- [ ] **Step 1: Write the failing test**

```rust
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib os::resolve::tests::resolve_config_returns_a_hostname` → FAIL (`resolve_config` not found).

- [ ] **Step 3: Implement `resolve_config` (bounded spawn)**

```rust
use std::io;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::binaries::tools;

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
```

> Note: after `try_wait()` returns `Some(success)`, the stdout pipe still holds the output; `wait_with_output()` then drains it (the child has already exited, so this does not block). This is correct because `ssh -G` output is small (well under any pipe-buffer limit), so the child never blocks on a full stdout pipe before exit.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --lib os::resolve::tests::resolve_config_returns_a_hostname` → PASS. Then the full gate set → clean. (If CI lacks `ssh`, this test would fail — but CLAUDE.md mandates OpenSSH on PATH and CI provides it.)

- [ ] **Step 5: Commit**

```bash
git add src/os/resolve.rs
git commit -m "feat(resolve): bounded ssh -G subprocess (stdin null, timeout, kill)"
```

---

## Task 4: `has_match_exec` (pure config scan)

`ssh -G` *executes* `Match exec` predicates (verified). To avoid triggering them for a host whose config uses one, the caller (Phase 3) scans the config text first and, if a `Match exec` directive is present, degrades to manual entry WITHOUT running `ssh -G`. This is a conservative, syntax-only scan over the raw config text.

**Files:**
- Modify: `src/os/resolve.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn detects_match_exec() {
    assert!(has_match_exec("Match exec \"test -e /tmp/x\"\n  User bob\n"));
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
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --lib os::resolve::tests::detects_match_exec os::resolve::tests::ignores_non_exec_match` → FAIL (not found).

- [ ] **Step 3: Implement `has_match_exec`**

```rust
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
        if !words.next().is_some_and(|w| w.eq_ignore_ascii_case("Match")) {
            continue;
        }
        if words.any(|w| w.eq_ignore_ascii_case("exec")) {
            return true;
        }
    }
    false
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib os::resolve::tests` → PASS. Then the full gate set → clean.

- [ ] **Step 5: Commit**

```bash
git add src/os/resolve.rs
git commit -m "feat(resolve): has_match_exec config pre-scan (avoid ssh -G side effects)"
```

---

## Task 5: `tofu_lookup_key` (pure)

The TOFU gate must look the host up in `known_hosts` using the SAME key OpenSSH uses: `HostKeyAlias` verbatim if set; else the resolved hostname if `port == 22`; else `[<hostname>]:<port>`. Never the raw alias.

**Files:**
- Modify: `src/os/resolve.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn lookup_key_rules() {
    let base = ResolvedConfig { hostname: Some("10.0.0.5".into()), port: Some("22".into()), ..Default::default() };
    assert_eq!(tofu_lookup_key(&base).as_deref(), Some("10.0.0.5"));

    let p2222 = ResolvedConfig { hostname: Some("10.0.0.5".into()), port: Some("2222".into()), ..Default::default() };
    assert_eq!(tofu_lookup_key(&p2222).as_deref(), Some("[10.0.0.5]:2222"));

    let ka = ResolvedConfig { hostname: Some("10.0.0.5".into()), port: Some("2222".into()), host_key_alias: Some("web1ka".into()), ..Default::default() };
    assert_eq!(tofu_lookup_key(&ka).as_deref(), Some("web1ka")); // verbatim, ignores port

    let no_host = ResolvedConfig::default();
    assert_eq!(tofu_lookup_key(&no_host), None);

    let no_port = ResolvedConfig { hostname: Some("h".into()), port: None, ..Default::default() };
    assert_eq!(tofu_lookup_key(&no_port).as_deref(), Some("h")); // missing port = default 22
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib os::resolve::tests::lookup_key_rules` → FAIL (not found).

- [ ] **Step 3: Implement `tofu_lookup_key`**

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib os::resolve::tests::lookup_key_rules` → PASS. Then the full gate set → clean.

- [ ] **Step 5: Commit**

```bash
git add src/os/resolve.rs
git commit -m "feat(resolve): tofu_lookup_key (HostKeyAlias / host / [host]:port)"
```

---

## Task 6: `is_host_known` — TOFU gate via `ssh-keygen -F`

Probe every resolved `known_hosts` file with `ssh-keygen -F <key> -f <file>` (System32-preferred `tools().ssh_keygen`; hashed-entry aware, unlike a plain parse). Exit 0 means a match was found AND printed; re-parse the printed matching line(s) with `known_hosts::parse_line` and accept ONLY a plain entry (no `@revoked`/`@cert-authority` marker, no `*`/`?`/`!` wildcard in the host field). Any plain match in any file → known.

**Files:**
- Modify: `src/os/resolve.rs`

- [ ] **Step 1: Write the failing tests (fixture-based, hermetic)**

```rust
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
    writeln!(f, "@revoked bad.example ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBBB").unwrap();
    writeln!(f, "@cert-authority *.ca.example ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICCC").unwrap();
    drop(f);
    let khs = vec![kh.to_string_lossy().to_string()];

    assert!(is_host_known("good.example", &khs), "plain entry should be known");
    assert!(!is_host_known("bad.example", &khs), "@revoked must not count as known");
    assert!(!is_host_known("ca.example", &khs), "@cert-authority wildcard must not count");
    assert!(!is_host_known("absent.example", &khs), "absent host is not known");

    let _ = std::fs::remove_dir_all(&dir);
}
```

> The `ssh-keygen -F` requires `ssh-keygen` on PATH (CLAUDE.md mandates it). `ssh-keygen -F` does plain matching against these unhashed entries; it also matches the same fixture if it were `ssh-keygen -H`-hashed. The test deliberately uses unhashed lines so the assertion is deterministic across machines.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib os::resolve::tests::is_known_accepts_plain` → FAIL (`is_host_known` not found).

- [ ] **Step 3: Implement `is_host_known`**

```rust
use super::known_hosts::{parse_line, HostSpec};

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
        e.marker.is_none() && matches!(e.host, HostSpec::Plain(ref h) if !h.contains(['*', '?', '!']))
    })
}
```

> `parse_line` skips the `# ...` comment lines (they start with `#`), so only the real matched entries are inspected. A wildcard like `*.ca.example` is a `HostSpec::Plain` whose text contains `*`, so the `contains(['*','?','!'])` check correctly rejects it.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --lib os::resolve::tests::is_known_accepts_plain` → PASS. Then the full gate set → clean.

- [ ] **Step 5: Commit**

```bash
git add src/os/resolve.rs
git commit -m "feat(resolve): TOFU is_host_known via ssh-keygen -F (plain entries only)"
```

---

## Task 7: `MatchedKinds` + `match_vault_kinds` (pure candidacy matcher)

The candidacy predicate: a host is an auto-fill candidate if a vault entry's `host` equals ANY of the host's patterns (the patterns are the `Host` line; `alias()` is the first), excluding any pattern containing a glob (`*`/`?`) or negation (`!`). Returns which kinds (Password / Passphrase) are present. This pure matcher is shared in Phase 3/4 by `App::vault_secret_kinds` (which adds opt-in masking) and the indicator.

**Files:**
- Modify: `src/os/vault.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` in `src/os/vault.rs`:

```rust
#[test]
fn match_vault_kinds_basics() {
    use super::{match_vault_kinds, MatchedKinds, VaultEntry, SecretKind};
    let entries = vec![
        VaultEntry { host: "web1".into(), kind: SecretKind::Password, secret: "p".into(), note: String::new() },
        VaultEntry { host: "web1".into(), kind: SecretKind::Passphrase, secret: "k".into(), note: String::new() },
        VaultEntry { host: "db".into(), kind: SecretKind::Password, secret: "p".into(), note: String::new() },
    ];
    // exact alias match, both kinds.
    assert_eq!(match_vault_kinds(&["web1".to_string()], &entries),
        Some(MatchedKinds { password: true, passphrase: true }));
    // a second pattern on the Host line matches too.
    assert_eq!(match_vault_kinds(&["nope".to_string(), "db".to_string()], &entries),
        Some(MatchedKinds { password: true, passphrase: false }));
    // no match -> None.
    assert_eq!(match_vault_kinds(&["other".to_string()], &entries), None);
    // glob / negation patterns are never candidates.
    assert_eq!(match_vault_kinds(&["web*".to_string()], &entries), None);
    assert_eq!(match_vault_kinds(&["!web1".to_string()], &entries), None);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib os::vault::tests::match_vault_kinds_basics` → FAIL (`match_vault_kinds`/`MatchedKinds` not found).

- [ ] **Step 3: Implement `MatchedKinds` + `match_vault_kinds`**

Add to `src/os/vault.rs` (module body, near `VaultEntry`):

```rust
/// Which secret kinds a host has stored, for the auto-fill candidacy predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MatchedKinds {
    pub password: bool,
    pub passphrase: bool,
}

impl MatchedKinds {
    pub fn any(self) -> bool {
        self.password || self.passphrase
    }
}

/// The pure auto-fill candidacy match: a host (given its `Host`-line `patterns`)
/// is a candidate if a vault entry's `host` equals ANY non-glob, non-negation
/// pattern. Returns the kinds present, or `None` if nothing matches. This is the
/// host<->entry match only — it is NOT the listener's prompt/identity-binding
/// release logic (see `os/askpass.rs`).
pub fn match_vault_kinds(patterns: &[String], entries: &[VaultEntry]) -> Option<MatchedKinds> {
    let mut m = MatchedKinds::default();
    for pat in patterns {
        if pat.contains(['*', '?', '!']) {
            continue;
        }
        for e in entries {
            if e.host == *pat {
                match e.kind {
                    SecretKind::Password => m.password = true,
                    SecretKind::Passphrase => m.passphrase = true,
                }
            }
        }
    }
    m.any().then_some(m)
}
```

> If `cargo clippy --all-targets -- -D warnings` flags `MatchedKinds`/`match_vault_kinds` as unused in the bin(non-test) build (likely — vault.rs has no module-level allow and nothing references them until Phase 3), add a scoped `#[allow(dead_code)]` with a `// TODO(phase3): consumed by App::vault_secret_kinds + the indicator` comment on EACH of the two items. Do NOT add a module-level allow to vault.rs (it has none today and the rest of the module is live). Verify which is needed by running clippy; add the minimum.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --lib os::vault::tests::match_vault_kinds_basics` → PASS. Then the full gate set (host fmt/clippy/test + linux cross-clippy) → clean. Confirm the existing vault tests still pass.

- [ ] **Step 5: Commit**

```bash
git add src/os/vault.rs
git commit -m "feat(vault): MatchedKinds + pure match_vault_kinds candidacy predicate"
```

---

## Self-review checklist (run before handoff)

- [ ] **Spec coverage (Phase 2 scope):** `ssh -G` resolve + parse ✓ (T2, T3); bounded/timeout/stdin-null ✓ (T3); Match-exec pre-scan ✓ (T4); TOFU lookup key ✓ (T5); TOFU known-host gate with marker/wildcard exclusion across all known_hosts files ✓ (T6); candidacy matcher (any-pattern, glob/negation excluded, kinds) ✓ (T7). Deferred items (off-thread resolver, `App::vault_secret_kinds` masking, connect wiring, UI, save-path `\r\n` rejection) are documented in "Scope" as Phase 3/4.
- [ ] **Placeholder scan:** every code step has complete code; no TBD/"add error handling"/"similar to". The only conditional is T7's scoped-allow note (add iff clippy requires), which is explicit about what to add and when.
- [ ] **Type consistency:** `ResolvedConfig` fields are used identically across `parse_ssh_g_output` (T2), `resolve_config` (T3), `tofu_lookup_key` (T5). `tofu_lookup_key` returns `Option<String>`; `is_host_known(lookup_key, files)` takes the unwrapped key + the `user_known_hosts_files`/`global_known_hosts_files` lists. `MatchedKinds { password, passphrase }` matches the Phase-1-plan/spec shape Phase 3 expects. `parse_line`/`HostSpec` reused from `os/known_hosts.rs` with their real signatures. `tools().ssh`/`tools().ssh_keygen` are the real `SshTools` fields. `VaultEntry { host, kind, secret, note }` and `SecretKind::{Password,Passphrase}` match `os/vault.rs`.
- [ ] **No regressions:** only additive files/items; existing 97 tests stay green; all four gates (incl. linux cross-clippy) must pass each task.
- [ ] **Cross-task note for the executor:** Phase 2 has NO platform-specific code, so the linux cross-clippy is a consistency gate (it keeps Phase 1's `#[cfg(unix)]` arm compiling); it should stay green without special effort.
