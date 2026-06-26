# SFTP Browser Password Auto-fill Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the dual-pane SFTP browser (`b`) auto-fill a stored vault password on password-only hosts, and close a pre-existing arming-gate gap on the SFTP transfer path.

**Architecture:** A shared pure gate decision (`sftp_arm_kinds`) brings the SFTP transfer arming to parity with `connect_plan` (adds not-proxied + TOFU gates). The browser, when the browsed host is a full auto-fill candidate, opens its `SftpSession` in an *armed* mode: each background `sftp -b` op passes an explicit `-o BatchMode=no` (overriding the `-b`-implied batchmode) and mints its **own** fresh `arm_connect` listener, so the per-connect single-shot is consumed once per op with no change to `os/askpass.rs`. A first-auth-failure circuit-breaker reverts to `BatchMode=yes` to bound Windows lockout exposure; an in-browser `F` key + advisory steer is the fallback for non-armable cases.

**Tech Stack:** Rust 2024, ratatui, OpenSSH `sftp`/`ssh` (System32 build on Windows), the existing `os/askpass.rs` askpass listener + `os/vault.rs` encrypted vault.

## Global Constraints

- MSRV `rust-version = "1.94"`; Rust edition 2024.
- CI gates run on Linux **and** Windows — all three must pass on both: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all`.
- **No changes to `os/askpass.rs`** or its single-shot model (`password_served`, per-path passphrase shot). Each browse op gets a *fresh* `arm_connect`.
- Secrets must zeroize on drop (use the existing `Secret`/`ConnectSecrets` types; never `to_string()` a secret onto the heap un-zeroized).
- Identity binding is unchanged: secrets bind to the `ssh -G`-resolved `user@host` + IdentityFile paths via `resolved_identity` + `arm_connect`.
- Armed mode is gated on `os::binaries::tools().is_system32` — never arm against the Git/MSYS `[PATH ssh]` fallback build.
- Cross-platform clippy: any `#[cfg(windows)]`-only symbol must stay referenced on Linux too (the `find_wt`/`escape_wt_arg` dead_code trap). Keep the armed/un-armed arg split symmetric across `cfg(unix)`/`cfg(not(unix))`.
- `config/` and `os/` keep zero ratatui dependency; UI never mutates domain state.

---

## File Structure

- `src/update.rs` — `arm_sftp_secrets` gate fix + new pure `sftp_arm_kinds`; new `compute_sftp_arm`; `open_sftp_browser` arms the session; `handle_sftp_browser` gains `F`.
- `src/os/sftp.rs` — `SftpArm` type; `SftpSession` armed mode (`arm` field, `set_arm`/`disable_arm`); armed `run_op` (`-o BatchMode=no` + per-op `arm_connect`); pure `is_auth_failure` + `batchmode_args`.
- `src/app.rs` — `apply_sftp_event` trips the circuit-breaker; `friendly_sftp_error` gains the auth-failure steer.
- `src/ui/mod.rs` — `SFTP_BROWSER_FOOTER` adds `F sftp`.
- `src/ui/help.rs` — `SftpBrowser` help block adds the `F` line.

---

## Task 1: STEP 0 — close GAP 1/2 (SFTP transfer arming gate parity)

Independently shippable security fix: `arm_sftp_secrets` (used by the guided transfer **and** the in-browser transfer) must refuse to arm — for **both** password and passphrase — a proxied or not-yet-trusted host, exactly as `connect_plan` does.

**Files:**
- Modify: `src/update.rs` (`arm_sftp_secrets` ~1705-1749; add `sftp_arm_kinds`)
- Test: `src/update.rs` `#[cfg(test)]` module (inline)

**Interfaces:**
- Consumes: `MatchedKinds` (`os::vault`), `should_arm_sftp_password` (existing, `update.rs:1756`), `tofu_lookup_key`/`is_host_known` (`os::resolve`), `known_hosts_files` (`update.rs:818`), `resolved_target` (`update.rs:807`).
- Produces: `fn sftp_arm_kinds(candidacy: MatchedKinds, autofill_enabled: bool, target_consented: bool, kbdint_supported: bool, is_proxied: bool, is_known: bool) -> Option<MatchedKinds>` — the SFTP-path gate decision, reused by Task 4.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `src/update.rs`:

```rust
#[test]
fn sftp_arm_kinds_matches_connect_plan_blanket_gates() {
    use crate::os::vault::MatchedKinds;
    let both = MatchedKinds { password: true, passphrase: true };
    let pp_only = MatchedKinds { password: false, passphrase: true };

    // Known, non-proxied, consented, autofill on, kbdint ok -> arm both.
    assert_eq!(
        sftp_arm_kinds(both, true, true, true, false, true),
        Some(MatchedKinds { password: true, passphrase: true })
    );
    // PROXIED -> arm NOTHING, even the local passphrase (connect_plan parity).
    assert_eq!(sftp_arm_kinds(both, true, true, true, true, true), None);
    assert_eq!(sftp_arm_kinds(pp_only, true, true, true, true, true), None);
    // NOT KNOWN (TOFU) -> arm NOTHING, even the passphrase.
    assert_eq!(sftp_arm_kinds(both, true, true, true, false, false), None);
    assert_eq!(sftp_arm_kinds(pp_only, true, true, true, false, false), None);
    // Known + non-proxied but password un-consented -> passphrase only.
    assert_eq!(
        sftp_arm_kinds(both, true, false, true, false, true),
        Some(MatchedKinds { password: false, passphrase: true })
    );
    // Autofill off -> password masked, passphrase still arms.
    assert_eq!(
        sftp_arm_kinds(both, false, true, true, false, true),
        Some(MatchedKinds { password: false, passphrase: true })
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib sftp_arm_kinds_matches_connect_plan_blanket_gates`
Expected: FAIL — `cannot find function sftp_arm_kinds`.

- [ ] **Step 3: Add the pure gate function**

Insert directly above `should_arm_sftp_password` in `src/update.rs`:

```rust
/// The SFTP-transfer/browser arming decision, at full parity with `connect_plan`.
/// Returns the kinds to arm, or `None` to arm nothing. The two blanket gates are
/// load-bearing for BOTH kinds: a proxied or not-yet-trusted host arms neither the
/// server password NOR the local passphrase (arming sets `SSH_ASKPASS_REQUIRE=force`,
/// which would intercept the host-key prompt). Pure, so it is unit-tested against the
/// same truth table as `connect_plan`.
fn sftp_arm_kinds(
    candidacy: MatchedKinds,
    autofill_enabled: bool,
    target_consented: bool,
    kbdint_supported: bool,
    is_proxied: bool,
    is_known: bool,
) -> Option<MatchedKinds> {
    if is_proxied || !is_known {
        return None;
    }
    let password = should_arm_sftp_password(
        candidacy.password,
        autofill_enabled,
        target_consented,
        kbdint_supported,
    );
    let kinds = MatchedKinds {
        password,
        passphrase: candidacy.passphrase,
    };
    kinds.any().then_some(kinds)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib sftp_arm_kinds_matches_connect_plan_blanket_gates`
Expected: PASS.

- [ ] **Step 5: Wire the gate into `arm_sftp_secrets`**

Replace the body of `arm_sftp_secrets` between the `resolve_config_with_options` line and the `gather_secrets` line (`src/update.rs` ~1721-1739) so the kinds come from `sftp_arm_kinds` with the two new gate inputs:

```rust
    let Ok(rc) = resolve_config_with_options(&[], alias) else {
        return (None, Vec::new());
    };
    // Blanket gates at connect_plan parity (GAP 1/2): never arm EITHER kind for a
    // proxied or not-yet-trusted host.
    let is_proxied =
        host.is_proxied() || rc.proxy_jump.is_some() || rc.proxy_command.is_some();
    let is_known = tofu_lookup_key(&rc)
        .is_some_and(|k| is_host_known(&k, &known_hosts_files(&rc)));
    let Some(kinds) = sftp_arm_kinds(
        candidacy,
        app.password_autofill_enabled,
        app.confirmed_password_targets
            .contains(&resolved_target(&rc)),
        crate::os::askpass::ssh_kbdint_prefix_supported(),
        is_proxied,
        is_known,
    ) else {
        return (None, Vec::new());
    };
    let (password, passphrase) = gather_secrets(app, host, kinds);
```

Ensure `tofu_lookup_key` and `is_host_known` are imported (they live in `crate::os::resolve`; check the existing `use` block — the connect path already imports them, e.g. `use crate::os::resolve::{... is_host_known, tofu_lookup_key ...}`). The old `arm_password`/`kinds`/`should_arm_sftp_password` call is now subsumed; delete the superseded lines.

- [ ] **Step 6: Run the full gates**

Run: `cargo test --all`
Expected: PASS (existing `should_arm_sftp_password`/connect_plan tests untouched and green).
Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean (watch for an unused import if `arm_password` was the only user of something — it was not).
Run: `cargo fmt --all -- --check`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/update.rs
git commit -m "fix(sftp): arm transfers only for known, non-proxied hosts (GAP 1/2)

arm_sftp_secrets diverged from connect_plan: it armed a passphrase (and a
consented password) for unknown/proxied hosts on both the guided- and
in-browser-transfer paths. Add the not-proxied + TOFU blanket gates via a new
pure sftp_arm_kinds, mirroring connect_plan's truth table.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01NwsPJhrKscjLXQCTUN3QRY"
```

---

## Task 2: Pure `is_auth_failure` classifier

**Files:**
- Modify: `src/os/sftp.rs` (add `is_auth_failure`)
- Test: `src/os/sftp.rs` `#[cfg(test)]` module (inline)

**Interfaces:**
- Produces: `pub fn is_auth_failure(stderr: &str) -> bool` — true iff the sftp/ssh stderr shows an *authentication* failure (not a directory-ACL denial, not a host-key/KEX failure). Used by the circuit-breaker (Task 4) and the steer (Task 5).

- [ ] **Step 1: Write the failing test**

Add to `#[cfg(test)] mod tests` in `src/os/sftp.rs`:

```rust
#[test]
fn is_auth_failure_classifies_auth_only() {
    // Positive: the parenthesized method-list form, and the explicit phrases.
    assert!(is_auth_failure(
        "user@host: Permission denied (publickey,password)."
    ));
    assert!(is_auth_failure("Authentication failed."));
    assert!(is_auth_failure("Received disconnect: Too many authentication failures"));
    // Positive even when a banner precedes it (scan ALL lines).
    assert!(is_auth_failure(
        "WARNING: unauthorized use prohibited\nuser@host: Permission denied (password)."
    ));
    // Negative: a directory-ACL denial with NO (method-list).
    assert!(!is_auth_failure("remote open(\"/root/x\"): Permission denied"));
    // Negative: host-key / KEX phase (must route to the accept-key steer, not here).
    assert!(!is_auth_failure("Host key verification failed."));
    assert!(!is_auth_failure("Connection closed by 10.0.0.1 port 22"));
    // Negative: a server banner that merely contains the substring before success.
    assert!(!is_auth_failure("Notice: 'Permission denied (' is logged for audits"));
}
```

Note the last case: the banner contains `Permission denied (` but is not at a line that *is* the denial. The classifier requires the line to **start** (after trim) with a recognised auth-failure shape, so an embedded substring in prose does not match.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib is_auth_failure_classifies_auth_only`
Expected: FAIL — `cannot find function is_auth_failure`.

- [ ] **Step 3: Implement the classifier**

Add near `first_error_line` in `src/os/sftp.rs`:

```rust
/// True iff `stderr` reports an OpenSSH **authentication** failure (vs a
/// directory-ACL denial, a host-key/KEX failure, or an arbitrary banner). Scans
/// every line so a leading server banner can neither mask the result nor forge a
/// false positive. A line must, after trimming, BE an auth-failure form: the
/// `<user>@<host>: Permission denied (<method-list>).` exhaustion message (the
/// `": Permission denied ("` infix demands the `user@host:` prefix AND the
/// parenthesized method list), `Authentication failed…`, or `Too many
/// authentication failures`. A bare directory `Permission denied` (no `(method)`)
/// and a host-key failure are deliberately negatives.
pub fn is_auth_failure(stderr: &str) -> bool {
    stderr.lines().map(str::trim).any(|line| {
        let denied_with_methods =
            line.contains(": Permission denied (") && line.ends_with(").");
        denied_with_methods
            || line.starts_with("Authentication failed")
            || line.contains("Too many authentication failures")
    })
}
```

Why the cases hold: the directory-ACL form `remote open("..."): Permission denied`
has no `(` after `denied` → false; the prose banner `Notice: 'Permission denied
('…` has `: '…` not `: Permission denied (` → false; `…: Permission denied
(publickey,password).` matches both halves → true.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib is_auth_failure_classifies_auth_only`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/os/sftp.rs
git commit -m "feat(sftp): pure is_auth_failure stderr classifier

Distinguishes an OpenSSH authentication failure from a directory-ACL denial or a
host-key/KEX failure, scanning all stderr lines so a banner can't mask or forge
the result. Used by the browser circuit-breaker and the F steer.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01NwsPJhrKscjLXQCTUN3QRY"
```

---

## Task 3: Armed `SftpSession` mode (`-o BatchMode=no` + per-op `arm_connect`)

**Files:**
- Modify: `src/os/sftp.rs` (`SftpArm`, `SftpSession.arm`, `set_arm`/`disable_arm`/`is_armed`, `request`, `run_op`, `batchmode_args`)
- Test: `src/os/sftp.rs` `#[cfg(test)]` module (inline)

**Interfaces:**
- Consumes: `arm_connect`, `ResolvedIdentity`, `AskpassListener`, `Outcome` (`os::askpass`); `Secret` (`os::vault`).
- Produces:
  - `pub struct SftpArm { pub identity: ResolvedIdentity, pub password: Option<Secret>, pub passphrase: Option<Secret> }`
  - `SftpSession::set_arm(&mut self, arm: SftpArm)`, `SftpSession::disable_arm(&mut self)`, `SftpSession::is_armed(&self) -> bool`
  - `fn batchmode_args(armed: bool) -> [&'static str; 2]` — `["-o","BatchMode=no"]` when armed, else `["-o","BatchMode=yes"]`.

- [ ] **Step 1: Write the failing test (arg selection is pure & testable)**

Add to `#[cfg(test)] mod tests` in `src/os/sftp.rs`:

```rust
#[test]
fn batchmode_args_flip_on_arm() {
    assert_eq!(batchmode_args(false), ["-o", "BatchMode=yes"]);
    assert_eq!(batchmode_args(true), ["-o", "BatchMode=no"]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib batchmode_args_flip_on_arm`
Expected: FAIL — `cannot find function batchmode_args`.

- [ ] **Step 3: Implement `batchmode_args` and switch `common_args` to use it**

In `src/os/sftp.rs`, add:

```rust
/// The BatchMode option pair for an op. Armed ops pass `BatchMode=no` to override
/// the `-b`-implied `batchmode yes` (sftp injects it into the spawned ssh), which
/// is what re-enables the armed `SSH_ASKPASS` helper for the password prompt.
fn batchmode_args(armed: bool) -> [&'static str; 2] {
    if armed {
        ["-o", "BatchMode=no"]
    } else {
        ["-o", "BatchMode=yes"]
    }
}
```

Change `with_control` so `common_args` no longer hardcodes `BatchMode=yes` (it will be supplied per-op by `run_op` from `batchmode_args`). The `unix` arm keeps only the control options; the non-unix arm becomes empty:

```rust
        #[cfg(unix)]
        let common_args = {
            let mut a: Vec<String> = Vec::new();
            if let Some(path) = &control {
                a.extend(control_options(path));
            }
            a
        };
        #[cfg(not(unix))]
        let common_args: Vec<String> = Vec::new();
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib batchmode_args_flip_on_arm`
Expected: PASS.

- [ ] **Step 5: Add the `SftpArm` type + the session arming fields/methods**

In `src/os/sftp.rs` add the imports and type (only `arm_connect` + `ResolvedIdentity`
are named; `AskpassListener`/`Outcome` are reached through `arm_connect`'s return
and `.stop_and_join()`, so importing them would be an unused-import clippy error):

```rust
use crate::os::askpass::{ResolvedIdentity, arm_connect};
use crate::os::vault::Secret;

/// The per-session arming recipe: the resolved identity to bind to, plus the
/// vault secrets to release. Each browse op mints its OWN listener from a clone of
/// this, so the per-connect single-shot is consumed exactly once per op. Dropped
/// (zeroizing the secrets) when the browser closes or the circuit-breaker trips.
/// `Secret` and `ResolvedIdentity` both derive `Clone`, so the derive suffices.
#[derive(Clone)]
pub struct SftpArm {
    pub identity: ResolvedIdentity,
    pub password: Option<Secret>,
    pub passphrase: Option<Secret>,
}
```

Add an `arm: Option<SftpArm>` field to `SftpSession` (init `None` in `with_control`), and the methods:

```rust
    /// Arm this session: subsequent ops run with `-o BatchMode=no` + a fresh
    /// per-op askpass listener built from `arm`.
    pub fn set_arm(&mut self, arm: SftpArm) {
        self.arm = Some(arm);
    }

    /// Disable arming (circuit-breaker / teardown): drops + zeroizes the secrets
    /// and reverts subsequent ops to `BatchMode=yes`.
    pub fn disable_arm(&mut self) {
        self.arm = None;
    }

    pub fn is_armed(&self) -> bool {
        self.arm.is_some()
    }
```

- [ ] **Step 6: Thread the arm into `request` + `run_op`**

In `request`, clone the arm into the worker:

```rust
    pub fn request(&mut self, op: SftpOp) {
        let alias = self.alias.clone();
        let common = self.common_args.clone();
        let arm = self.arm.clone();
        let tx = self.tx.clone();
        let handle = std::thread::spawn(move || {
            let event = run_op(&alias, &common, arm, op);
            let _ = tx.send(event);
        });
        self.handles.push(handle);
    }
```

Change `run_op` to take `arm: Option<SftpArm>` and build the command from `batchmode_args` + (when armed) the askpass env. Replace the args assembly + spawn in `run_op`:

```rust
fn run_op(alias: &str, common_args: &[String], arm: Option<SftpArm>, op: SftpOp) -> SftpEvent {
    let SftpOp::List(path) = op;
    let Some(batch) = list_batch(&path) else {
        return SftpEvent::Failed {
            path: Some(path),
            msg: "unsafe remote path (contains a quote or control character)".to_string(),
        };
    };
    let script = match stage_batch(&batch) {
        Ok(p) => p,
        Err(e) => {
            return SftpEvent::Failed {
                path: Some(path),
                msg: format!("could not stage command: {e}"),
            };
        }
    };
    let _cleanup = BatchCleanup(script.clone());

    let armed = arm.is_some();
    let mut args: Vec<String> = common_args.to_vec();
    args.extend(batchmode_args(armed).iter().map(|s| s.to_string()));
    args.push("-b".to_string());
    args.push(script.display().to_string());
    args.push("--".to_string());
    args.push(alias.to_string());

    // Arm a fresh per-op listener so the password single-shot is consumed once per
    // op (one op == one ssh connect). The listener zeroizes on stop_and_join.
    let listener_env = arm.and_then(|a| {
        arm_connect(a.identity, a.password, a.passphrase).ok()
    });
    let (listener, env) = match listener_env {
        Some((l, env)) => (Some(l), env),
        None => (None, Vec::new()),
    };

    let output = std::process::Command::new(tools().sftp.as_path())
        .args(&args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .output();

    // Tear the per-op listener down (zeroize) regardless of outcome.
    if let Some(l) = listener {
        let _ = l.stop_and_join();
    }

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let cwd = if path == "." { parse_pwd(&stdout) } else { None };
            SftpEvent::Listing { path, cwd, entries: parse_ls_l(&stdout) }
        }
        Ok(o) => SftpEvent::Failed {
            path: Some(path),
            msg: first_error_line(&o.stderr, &o.stdout),
        },
        Err(e) => SftpEvent::Failed { path: Some(path), msg: e.to_string() },
    }
}
```

(The `test`-only `open_no_master` constructor and any other `run_op` caller compile unchanged because `request` is the only caller; the new `arm` param is internal.)

- [ ] **Step 7: Run the gates**

Run: `cargo test --all`
Expected: PASS (existing sftp parse tests untouched).
Run: `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all -- --check`
Expected: clean. Watch: `SftpArm`/`set_arm`/`disable_arm`/`is_armed` are not yet *called* (Task 4 calls them) — if clippy flags dead_code, add `#[allow(dead_code)]` to the methods **only if** the lint fires, and remove it once Task 4 lands (or order Task 4 in the same commit). Prefer: do not commit Task 3 alone if the dead_code lint blocks; fold its commit into Task 4. (Decision rule: commit Task 3 only if `cargo clippy --all-targets -- -D warnings` is clean.)

- [ ] **Step 8: Commit (if clippy-clean; else fold into Task 4)**

```bash
git add src/os/sftp.rs
git commit -m "feat(sftp): armed SftpSession mode (-o BatchMode=no + per-op arm_connect)

Add SftpArm + an optional armed mode to SftpSession: armed ops pass
-o BatchMode=no (overriding the -b-implied batchmode) and mint a fresh per-op
askpass listener, so the password single-shot is consumed once per op with no
change to os/askpass.rs. BatchMode now comes per-op from batchmode_args.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01NwsPJhrKscjLXQCTUN3QRY"
```

---

## Task 4: Arm at browser-open + first-failure circuit-breaker

**Files:**
- Modify: `src/update.rs` (`compute_sftp_arm`; `open_sftp_browser` arms the session)
- Modify: `src/app.rs` (`apply_sftp_event` trips the circuit-breaker)
- Test: `src/app.rs` `#[cfg(test)]` (circuit-breaker on `apply_sftp_event`)

**Interfaces:**
- Consumes: `sftp_arm_kinds` (Task 1), `SftpArm`/`SftpSession::set_arm`/`disable_arm`/`is_armed` (Task 3), `is_auth_failure` (Task 2), `gather_secrets`, `resolved_identity`, `os_tokens`, `resolve_config_with_options`, `has_match_exec`, `tofu_lookup_key`, `is_host_known`, `known_hosts_files`, `resolved_target`, `tools().is_system32`.
- Produces: `fn compute_sftp_arm(app: &App, host: &HostView, alias: &str) -> Option<crate::os::sftp::SftpArm>`.

- [ ] **Step 1: Write the failing test (circuit-breaker)**

Add to `#[cfg(test)] mod tests` in `src/app.rs` (the module already constructs `SftpBrowser` via `SftpSession::open_no_master`):

```rust
#[test]
fn apply_sftp_event_trips_circuit_breaker_on_auth_failure() {
    use crate::os::sftp::{SftpArm, SftpEvent};
    use crate::os::askpass::ResolvedIdentity;
    let mut b = SftpBrowser {
        host: 0,
        focus: SftpPane::Remote,
        local_cwd: std::path::PathBuf::from("/"),
        local_entries: Vec::new(),
        local_sel: 0,
        remote_cwd: String::new(),
        remote_entries: Vec::new(),
        remote_sel: 0,
        remote_loading: true,
        status: String::new(),
        session: crate::os::sftp::SftpSession::open_no_master("h"),
    };
    b.session.set_arm(SftpArm {
        identity: ResolvedIdentity {
            user: "u".into(), host: "h".into(),
            host_key_alias: None, identity_paths: Vec::new(),
        },
        password: None, passphrase: None,
    });
    assert!(b.session.is_armed());
    apply_sftp_event(&mut b, SftpEvent::Failed {
        path: Some(".".into()),  // initial listing
        msg: "u@h: Permission denied (publickey,password).".into(),
    });
    // First auth failure on an armed session disarms it (no second bad attempt).
    assert!(!b.session.is_armed());
    assert!(b.status.contains("password"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib apply_sftp_event_trips_circuit_breaker_on_auth_failure`
Expected: FAIL (the breaker logic does not exist yet).

- [ ] **Step 3: Add the circuit-breaker to `apply_sftp_event`**

In `src/app.rs`, in the `SftpEvent::Failed { path, msg }` arm of `apply_sftp_event`, place this **after** the existing stale-failure early-return (`if let Some(p) = &path && !b.remote_cwd.is_empty() && *p != b.remote_cwd { return; }`) and **before** the `b.remote_loading = false; b.status = friendly_sftp_error(&msg);` lines, so a stale failure can never trip the breaker:

```rust
            // Circuit-breaker: the first auth failure on an armed session disarms
            // it, so the (apparently wrong/stale) stored password is sent at most
            // once — subsequent ops fall back to BatchMode=yes (no password sent),
            // bounding fail2ban/lockout exposure on Windows (no ControlMaster).
            if b.session.is_armed() && crate::os::sftp::is_auth_failure(&msg) {
                b.session.disable_arm();
                b.remote_loading = false;
                b.status =
                    "stored password rejected — re-check the vault, or press F".to_string();
                if b.remote_entries.is_empty() {
                    b.remote_entries = vec![RemoteEntry::parent()];
                    b.remote_sel = 0;
                }
                return;
            }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib apply_sftp_event_trips_circuit_breaker_on_auth_failure`
Expected: PASS.

- [ ] **Step 5: Add `compute_sftp_arm` and arm the session at open**

Add to `src/update.rs` (near `arm_sftp_secrets`):

```rust
/// Build the per-session SFTP browse arming recipe for `host`, or `None` when the
/// host is not a full auto-fill candidate. Same gate truth as the connect path
/// (via `sftp_arm_kinds`), plus the System32-ssh requirement (never arm against
/// the Git/MSYS `[PATH ssh]` build, whose force/console handling differs).
fn compute_sftp_arm(app: &App, host: &HostView, alias: &str) -> Option<crate::os::sftp::SftpArm> {
    if !crate::os::binaries::tools().is_system32 {
        return None;
    }
    let candidacy = app.vault_secret_kinds(host)?;
    if has_match_exec(&app.config.render()) {
        return None;
    }
    let rc = resolve_config_with_options(&[], alias).ok()?;
    let is_proxied =
        host.is_proxied() || rc.proxy_jump.is_some() || rc.proxy_command.is_some();
    let is_known = tofu_lookup_key(&rc)
        .is_some_and(|k| is_host_known(&k, &known_hosts_files(&rc)));
    let kinds = sftp_arm_kinds(
        candidacy,
        app.password_autofill_enabled,
        app.confirmed_password_targets
            .contains(&resolved_target(&rc)),
        crate::os::askpass::ssh_kbdint_prefix_supported(),
        is_proxied,
        is_known,
    )?;
    let (password, passphrase) = gather_secrets(app, host, kinds);
    if password.is_none() && passphrase.is_none() {
        return None;
    }
    let identity = resolved_identity(&rc, alias, &os_tokens());
    Some(crate::os::sftp::SftpArm { identity, password, passphrase })
}
```

In `open_sftp_browser` (`src/update.rs:1812`), after `let mut session = SftpSession::open(&alias);` and **before** the first `session.request(...)`, arm it when eligible. Note `host` there is the `usize` index, so fetch the `HostView`:

```rust
    let mut session = SftpSession::open(&alias);
    if let Some(h) = app.hosts.get(host)
        && let Some(arm) = compute_sftp_arm(app, h, &alias)
    {
        session.set_arm(arm);
    }
    session.request(SftpOp::List(".".to_string()));
```

- [ ] **Step 6: Run the gates**

Run: `cargo test --all`
Expected: PASS.
Run: `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all -- --check`
Expected: clean (the Task 3 `SftpArm`/`set_arm`/`is_armed`/`disable_arm` symbols are now all referenced).

- [ ] **Step 7: Commit**

```bash
git add src/update.rs src/app.rs
git commit -m "feat(sftp): arm the browse session for password hosts + circuit-breaker

open_sftp_browser arms the SftpSession when the browsed host passes the full
connect-parity gate set (compute_sftp_arm), so listing ops auto-fill the stored
password. apply_sftp_event trips a first-auth-failure circuit-breaker that
disarms the session, bounding bad-password attempts to one on Windows.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01NwsPJhrKscjLXQCTUN3QRY"
```

---

## Task 5: In-browser `F` fallback + steer + footer/help

**Files:**
- Modify: `src/update.rs` (`handle_sftp_browser` `F` arm)
- Modify: `src/app.rs` (`friendly_sftp_error` steer)
- Modify: `src/ui/mod.rs` (`SFTP_BROWSER_FOOTER`)
- Modify: `src/ui/help.rs` (SftpBrowser help block)
- Test: `src/ui/mod.rs` (`footers_fit_80_cols`, already present); `src/app.rs` (steer)

**Interfaces:**
- Consumes: `connect_by_alias`, `ConnectMode::Inline`, `Protocol::Sftp`, `PasswordChoice::Ask`, `ConnectOverrides::default()`, `is_auth_failure` (Task 2).

- [ ] **Step 1: Write the failing test (steer message)**

Add to `#[cfg(test)] mod tests` in `src/app.rs`:

```rust
#[test]
fn friendly_sftp_error_steers_on_auth_failure() {
    // Auth failure -> steer to F. Host-key failure keeps the accept-key steer.
    let s = friendly_sftp_error("u@h: Permission denied (publickey,password).");
    assert!(s.contains('F') && s.to_lowercase().contains("password"));
    let hk = friendly_sftp_error("Host key verification failed.");
    assert!(hk.contains("host key") && !hk.to_lowercase().contains("stored password"));
    // A directory-ACL denial passes through unchanged (not an auth failure).
    let acl = friendly_sftp_error("remote open(\"/root/x\"): Permission denied");
    assert_eq!(acl, "remote open(\"/root/x\"): Permission denied");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib friendly_sftp_error_steers_on_auth_failure`
Expected: FAIL.

- [ ] **Step 3: Extend `friendly_sftp_error`**

Replace `friendly_sftp_error` in `src/app.rs` (keep the host-key arm FIRST so an unknown host routes to the accept-key steer, never the password steer):

```rust
fn friendly_sftp_error(msg: &str) -> String {
    if msg.contains("Host key verification failed") {
        "host key not trusted — connect once (Enter/F) to accept it, then browse".to_string()
    } else if crate::os::sftp::is_auth_failure(msg) {
        "auth failed — press F to open an SFTP session (stored password auto-fills)".to_string()
    } else {
        msg.to_string()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib friendly_sftp_error_steers_on_auth_failure`
Expected: PASS.

- [ ] **Step 5: Add the in-browser `F` key**

In `handle_sftp_browser` (`src/update.rs:1855`), replace the `_ => {}` arm with an `F` handler that resolves the alias from **`b.host`** (not the list cursor) and connects inline via the fully-gated path:

```rust
        KeyCode::Char('F') => {
            // Resolve the alias from the BROWSED host (b.host), never the list
            // cursor, so the stored secret is served for the host on screen.
            let alias = app
                .sftp_browser
                .as_ref()
                .and_then(|b| app.hosts.get(b.host))
                .map(|h| h.alias().to_string());
            match alias {
                Some(alias) => {
                    app.sftp_browser = None;
                    app.screen = Screen::List;
                    return connect_by_alias(
                        app,
                        terminal,
                        &alias,
                        ConnectMode::Inline,
                        Protocol::Sftp,
                        PasswordChoice::Ask,
                        None,
                        ConnectOverrides::default(),
                    );
                }
                None => app.toast("host is no longer available", true),
            }
        }
        _ => {}
```

- [ ] **Step 6: Update the footer (stay within the 80-col budget)**

Replace `SFTP_BROWSER_FOOTER` in `src/ui/mod.rs` (drop `r refresh` — still in the help block — to make room for `F sftp`; width = 76 ≤ 80):

```rust
const SFTP_BROWSER_FOOTER: &[(&str, &str)] = &[
    ("Tab", "pane"),
    ("j/k", "move"),
    ("Enter", "open/xfer"),
    ("F", "sftp"),
    ("Bksp", "up"),
    ("?", "help"),
    ("Esc", "back"),
];
```

- [ ] **Step 7: Update the help block**

In `src/ui/help.rs`, in the `Screen::SftpBrowser` arm (after the `r` refresh line ~line 100), add the `F` line and keep `r` discoverable here:

```rust
            lines.push(key("F", "open an inline SFTP session (auto-fills password)"));
```

- [ ] **Step 8: Run the gates (footer width is asserted on both OSes)**

Run: `cargo test --all`
Expected: PASS, including `footers_fit_80_cols` (the new footer is 76 cols).
Run: `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all -- --check`
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add src/update.rs src/app.rs src/ui/mod.rs src/ui/help.rs
git commit -m "feat(sftp): in-browser F fallback + auth-failure steer

Add an in-browser F key that drops to the fully-gated inline SFTP session for the
BROWSED host (b.host, not the list cursor), an auth-failure steer in
friendly_sftp_error, and F in the footer/help. Covers locked-vault / unconsented
/ <8.5 / non-System32 / circuit-broken cases where in-browser arming can't apply.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01NwsPJhrKscjLXQCTUN3QRY"
```

---

## Manual verification (after Task 5, with a real password host)

Use the WSL Debian password host (`WSL-Debian` → `user@localhost:2222`, host key trusted) and a **scratch** vault (back up `~/.ssh/sshm-vault.json` first; `dirs` ignores `USERPROFILE` on Windows so the vault path is fixed — restore byte-exact after):

1. Unlock vault, `p` to enable pw-autofill, store the password under host `WSL-Debian`.
2. Select `WSL-Debian`, press `b` → the remote pane **lists** (auto-filled), not "Permission denied".
3. Navigate a directory or two → each lists (Windows: per-op re-auth, slight latency).
4. With a **wrong** stored password: `b` → one failure, status "stored password rejected — … press F", and no repeated attempts (circuit-breaker).
5. A key/agent host: `b` browses unchanged (still `BatchMode=yes`), no steer.
6. Vault locked: `b` on a password host → steer to `F`; `F` opens the inline session and auto-fills after consent.

## Self-review notes (coverage vs spec)

- STEP 0 / GAP 1/2 → Task 1. `is_auth_failure` → Task 2. `-o BatchMode=no` + per-op arm → Task 3. Open-time arming + circuit-breaker → Task 4. `F` + steer + footer/help → Task 5.
- `os/askpass.rs` untouched (Task 3 mints a fresh `arm_connect` per op). Identity binding unchanged. Secrets zeroize (SftpArm drop on close/disarm; per-op `ConnectSecrets` drop on `stop_and_join`).
- Open risk carried into review: overlapping in-flight listing ops each arm independently (more concurrent attempts before the breaker trips). The browser serializes most navigation; if review wants a hard guard, add "skip new listing dispatch while `remote_loading`" — left out to avoid changing the existing supersede-on-navigate behavior.
