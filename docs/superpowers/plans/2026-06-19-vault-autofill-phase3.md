# Vault auto-fill — Phase 3 (connect wiring) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans (or
> subagent-driven-development) to implement task-by-task. Steps use checkbox
> (`- [ ]`) syntax. Every task keeps the existing **114 tests** green and passes
> all four gates: host `cargo fmt --all -- --check`, host `cargo clippy
> --all-targets -- -D warnings`, host `cargo test --all`, AND `cargo clippy
> --target x86_64-unknown-linux-gnu --all-targets -- -D warnings`.
> NB: this is a **binary crate** (no lib target) — run targeted tests with
> `cargo test <filter>`, not `--lib`.

**Goal:** Wire the headless Phase 1 (`os/askpass.rs`) and Phase 2 (`os/resolve.rs`,
`os/vault::match_vault_kinds`) building blocks into the live connect path: resolve
→ gate (proxy/TOFU/Match-exec) → unlock → password-confirm → arm the askpass
listener → spawn `ssh` with the force env, drain the outcome, and surface the
pre-connect indicator, confirm modal, and outcome toasts.

**Spec:** `docs/superpowers/specs/2026-06-17-vault-autofill-design.md` (rev 4) —
sections "Connect-time flow", "Deferred unlock → confirm → connect state machine",
"App ownership of listeners", "Session-scoped password-confirm suppression",
"Password auto-fill enablement", "connect.rs signatures", "Discoverability".

**Builds on:** Phase 1 commits a1c941f..c0f901a (`os/askpass.rs`: `Listener::bind`/
`serve_one`, `ConnectSecrets`, `ResolvedIdentity`, `IdentityTokens` +
`expand_identity_path`, `classify`, wire protocol, `run_helper`, `Outcome`/
`DeclineReason`, `CHANNEL_ENV`/`TOKEN_ENV`/`Token`/`TOKEN_LEN`/`ct_eq`,
`secret_is_one_line`); Phase 2 commits 9edf8c0..97b6b09 (`os/resolve.rs`:
`resolve_config`, `parse_ssh_g_output`, `has_match_exec`, `tofu_lookup_key`,
`is_host_known`; `os/vault.rs`: `MatchedKinds` + `match_vault_kinds`).

---

## Step 0 — manual spike (GATING; not a code task)

The spec gates ship/degrade on a manual spike that this plan does **not** automate.
Until it is run, build to **conservative defaults** (which are already the design):

- **Password auto-fill defaults OFF** (`password_autofill_enabled=false`); only the
  dedicated `password` method is ever served; passphrase auto-fill is on.
- **New-tab (`t`) ships WITHOUT auto-fill** unless the env-inheritance spike passes;
  i.e. `connect_new_tab` is wired to *accept* the env param but the connect-dispatch
  only arms for `ConnectMode::Inline` in v1 (new-tab path passes `&[]`). A one-line
  constant `NEW_TAB_AUTOFILL: bool = false` gates this so it flips when the spike
  passes.
- **Timeouts:** `SSH_G_RESOLVE_TIMEOUT` already 500 ms (Phase 2). Add
  `NEW_TAB_ASKPASS_TIMEOUT = 60s` only when new-tab auto-fill is enabled.
- The implementation **degrades safely** (connect normally, no env) on any gate miss,
  so an unrun/partial spike never releases a secret wrongly — it only withholds.

### Spike RESULTS (run 2026-06-19, Windows 11 + System32 OpenSSH_for_Windows 9.5p2)

Validated empirically against the **System32 ssh `tools()` resolves** (a throwaway
sentinel-writing askpass exe captured the prompt; no login — the helper returned
empty so auth failed after capture; a temp `known_hosts` kept the real one clean):

- **force invokes the helper on Windows** ✅ — `SSH_ASKPASS_REQUIRE=force` +
  `SSH_ASKPASS=<exe>` routed every prompt to the helper (9.5p2). The core mechanism
  works here. (Helper-side dispatch already verified in T6.)
- **Host-key prompt IS hijacked** ✅ — an unknown host's "Are you sure you want to
  continue connecting (yes/no/[fingerprint])? " went to the helper → empty →
  "Host key verification failed". This **confirms the TOFU blocker** (Phase 2's
  `is_host_known` gate is load-bearing: never arm an unknown host).
- **Password prompt = `<user>@<host>'s password: `** ✅ — captured verbatim
  `demo@test.rebex.net's password: ` → `classify()` → `Password{user,host}`. Matches.
- **Keyboard-interactive prompt = `(<user>@<host>) Password: `** ✅ — the `(user@host)`
  instance prefix IS emitted by 9.5p2 (OpenSSH ≥8.5), so `classify()` → `Other` →
  password withheld. The load-bearing discriminator works against real server output;
  a stock server offering `keyboard-interactive` correctly burns-but-withholds (→
  `Declined{KeyboardInteractive}`). Confirms **password auto-fill must default OFF**.
- **`ssh -G` spawn latency ≈ 35–37 ms** ✅ — `SSH_G_RESOLVE_TIMEOUT = 500 ms` has >10×
  headroom. Confirmed.
- **Passphrase prompt — NOT server-captured** (no publickey-offering server was
  available; `test.rebex.net` is password/kbd-interactive only). `ssh-keygen -y` under
  force used `Enter passphrase: ` (no path) — that is ssh-keygen's form, NOT ssh's
  connect-time `Enter passphrase for key '<path>': ` that `classify()` targets; do
  **not** change classify to ssh-keygen's form. The ssh-connect passphrase format
  remains the documented openssh-portable source string (still owed a live capture
  against a publickey server before claiming the passphrase path end-to-end verified).
- **Still un-spiked:** new-tab (`wt.exe -w 0`) env inheritance → keep new-tab auto-fill
  gated OFF; the ssh-connect passphrase prompt live-capture (above).

Net: the password + host-key + force-mechanism + timeout are validated; T8 can wire the
inline connect path with confidence. New-tab + the passphrase live-capture stay deferred.

Tasks below are ordered **headless-first** (T1–T4: testable, spike-independent),
then **wiring** (T5–T9), then **UI** (T10–T12, best verified manually).

---

## Task 1: vault `\r`/`\n` rejection at entry + `secrets_for_host` lookup

The helper writes the secret + one `\n`; OpenSSH truncates at the first `\r`/`\n`.
A secret containing either cannot survive, so reject it at **save time** (`secret_is_one_line`
already exists in askpass). Also add the candidacy lookup the connect path needs.

**Files:** `src/os/vault.rs`, `src/update.rs`.

- [ ] **Step 1 (vault): `secrets_for_host` + reject `\r`/`\n` on save — failing tests**

Add to the `#[cfg(test)] mod tests` in `src/os/vault.rs`:

```rust
#[test]
fn secrets_for_host_returns_matching_kinds() {
    let mut v = Vault::create("pw").unwrap();
    v.upsert(None, entry("web1", SecretKind::Password, "p"));
    v.upsert(None, entry("web1", SecretKind::Passphrase, "k"));
    v.upsert(None, entry("db", SecretKind::Password, "d"));
    let got = v.secrets_for_host("web1");
    assert_eq!(got.len(), 2);
    assert!(got.iter().any(|e| e.kind == SecretKind::Password && e.secret.as_str() == "p"));
    assert!(got.iter().any(|e| e.kind == SecretKind::Passphrase));
    assert!(v.secrets_for_host("nope").is_empty());
}

#[test]
fn secret_validation_rejects_newlines() {
    assert!(reject_unservable_secret("ok").is_ok());
    assert!(reject_unservable_secret("two\nlines").is_err());
    assert!(reject_unservable_secret("cr\rret").is_err());
    assert!(reject_unservable_secret(&"x".repeat(1024)).is_err()); // > OpenSSH 1023 cap
}
```

- [ ] **Step 2: implement**

Add to `impl Vault` (near the other public methods):

```rust
/// All stored secrets whose `host` field equals `host` (case-sensitive, the
/// verbatim ssh destination). Candidacy only — release is decided by the
/// listener's identity binding (see `os/askpass.rs`).
pub fn secrets_for_host(&self, host: &str) -> Vec<&VaultEntry> {
    self.entries.iter().filter(|e| e.host == host).collect()
}
```

Add a free fn in `src/os/vault.rs` (module level, near `Secret`):

```rust
/// Reject a secret that cannot be delivered over OpenSSH's line-oriented askpass
/// channel: it must not contain `\r`/`\n` (OpenSSH truncates at the first) and
/// must fit the 1023-byte read cap. Enforced at vault-entry save time so an
/// unservable secret never reaches the channel.
pub fn reject_unservable_secret(secret: &str) -> Result<()> {
    if secret.contains(['\r', '\n']) {
        return Err(anyhow!("secret must not contain a newline or carriage return"));
    }
    if secret.len() > 1023 {
        return Err(anyhow!("secret is too long (max 1023 bytes)"));
    }
    Ok(())
}
```

Wire it into `save_vault_entry` (`src/update.rs` ~line 1260, after the empty check,
before constructing `VaultEntry`):

```rust
if let Err(e) = crate::os::vault::reject_unservable_secret(&app.vault_entry.secret) {
    app.toast(format!("{e}"), true);
    return;
}
```

- [ ] **Step 3: gates + commit** (`feat(vault): secrets_for_host + reject unservable secrets at save`).

---

## Task 2: `SSH_ASKPASS` path normalizer (verbatim-prefix stripper)

Win32-OpenSSH's `CreateProcessW` rejects a `\\?\`-prefixed argv0, and
`current_exe()` emits that prefix for installs at a path > 260 chars. Strip it.
Pure + table-testable; lives in `os/askpass.rs`.

**Files:** `src/os/askpass.rs`.

- [ ] **Step 1: failing tests**

```rust
#[test]
fn strips_verbatim_disk_and_unc_prefixes() {
    assert_eq!(strip_verbatim_prefix(r"\\?\C:\Users\me\sshm.exe"), r"C:\Users\me\sshm.exe");
    assert_eq!(strip_verbatim_prefix(r"\\?\UNC\server\share\sshm.exe"), r"\\server\share\sshm.exe");
    // ordinary paths pass through unchanged
    assert_eq!(strip_verbatim_prefix(r"C:\Users\me\sshm.exe"), r"C:\Users\me\sshm.exe");
    assert_eq!(strip_verbatim_prefix("/usr/local/bin/sshm"), "/usr/local/bin/sshm");
}
```

- [ ] **Step 2: implement** (string-level so it is testable on every OS — do NOT
gate behind `#[cfg(windows)]`; the synthetic inputs must run on the Linux gate too):

```rust
/// Strip a Windows verbatim path prefix so the result is a plain path
/// Win32-OpenSSH's `CreateProcessW` accepts as argv0: `\\?\C:\…` → `C:\…`,
/// `\\?\UNC\server\share\…` → `\\server\share\…`. Any other string is returned
/// unchanged. Operates on the string form (not `Path::components`) so it is
/// unit-testable with synthetic inputs on all platforms.
pub fn strip_verbatim_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        return rest.to_string();
    }
    path.to_string()
}

/// The absolute, prefix-normalized path to the running sshm exe, for SSH_ASKPASS.
pub fn askpass_exe_path() -> io::Result<String> {
    let exe = std::env::current_exe()?;
    Ok(strip_verbatim_prefix(&exe.to_string_lossy()))
}
```

- [ ] **Step 3: gates + commit** (`feat(askpass): SSH_ASKPASS verbatim-prefix stripper`).

---

## Task 3: `ResolvedConfig` → `IdentityTokens`/`ResolvedIdentity` bridge

Phase 2 resolves the config; Phase 1's listener binds to a `ResolvedIdentity` and
expands IdentityFile via `IdentityTokens`. This pure bridge connects them. OS-sourced
tokens (`%d` home, `%u` OS user, `%i` uid, `%l` localhost) are passed in so it stays
testable; a thin wrapper fills them from the environment.

**Files:** `src/os/askpass.rs` (consumes `crate::os::resolve::ResolvedConfig`).

- [ ] **Step 1: failing test** — build tokens from a `ResolvedConfig` + injected OS
values, expand the identity paths, and assemble a `ResolvedIdentity`:

```rust
#[test]
fn identity_from_resolved_config_expands_paths() {
    use crate::os::resolve::ResolvedConfig;
    let rc = ResolvedConfig {
        hostname: Some("web1".into()),
        user: Some("deploy".into()),
        port: Some("22".into()),
        host_key_alias: None,
        identity_files: vec!["~/.ssh/id_ed25519".into(), "~/.ssh/id_%C".into()],
        ..Default::default()
    };
    let os = OsTokens { home: "/home/u".into(), local_user: "u".into(), uid: "1000".into(), localhost: "box".into() };
    let id = resolved_identity(&rc, "web1", &os);
    assert_eq!(id.user, "deploy");
    assert_eq!(id.host, "web1");
    // the expandable path is kept; the %C path is dropped (fail-safe).
    assert_eq!(id.identity_paths, vec!["/home/u/.ssh/id_ed25519".to_string()]);
}
```

- [ ] **Step 2: implement**

```rust
/// OS-sourced expansion inputs that `ssh -G` does not provide. Injected so the
/// bridge is unit-testable; `os_tokens()` fills them from the environment.
#[derive(Debug, Clone)]
pub struct OsTokens {
    pub home: String,
    pub local_user: String,
    pub uid: String,
    pub localhost: String,
}

/// Build the `ssh -G`-bound `ResolvedIdentity`: resolve the prompt host
/// (HostKeyAlias verbatim, else hostname), the remote user, and the set of
/// IdentityFile paths that expand cleanly (unexpandable ones are dropped —
/// fail-safe, the listener simply won't release for them).
pub fn resolved_identity(rc: &crate::os::resolve::ResolvedConfig, host_arg: &str, os: &OsTokens) -> ResolvedIdentity {
    let toks = IdentityTokens {
        home: os.home.clone(),
        hostname: rc.hostname.clone().unwrap_or_else(|| host_arg.to_string()),
        local_user: os.local_user.clone(),
        remote_user: rc.user.clone().unwrap_or_default(),
        host_key_alias: rc.host_key_alias.clone(),
        host_arg: host_arg.to_string(),
        port: rc.port.clone().unwrap_or_else(|| "22".into()),
        proxy_jump_host: rc.proxy_jump.clone(),
        uid: os.uid.clone(),
        localhost: os.localhost.clone(),
    };
    let identity_paths = rc
        .identity_files
        .iter()
        .filter_map(|raw| expand_identity_path(raw, &toks))
        .collect();
    ResolvedIdentity {
        user: rc.user.clone().unwrap_or_default(),
        host: rc.hostname.clone().unwrap_or_else(|| host_arg.to_string()),
        host_key_alias: rc.host_key_alias.clone(),
        identity_paths,
    }
}

/// Fill `OsTokens` from the environment (HOME/USERPROFILE, USER/USERNAME, uid,
/// hostname). Best-effort: a missing value becomes empty (its tokens then fail
/// to match, which is fail-safe).
pub fn os_tokens() -> OsTokens { /* env::var HOME/USERPROFILE, USER/USERNAME, etc. */ }
```

> Note: `ResolvedIdentity.host` must be the value OpenSSH puts in the prompt. `ssh -G`
> already ASCII-lowercased `hostname`; do **not** re-fold. `host_key_alias` is compared
> verbatim by `ResolvedIdentity::prompt_host()`.

- [ ] **Step 3: gates + commit** (`feat(askpass): ResolvedConfig -> ResolvedIdentity bridge with token expansion`).

---

## Task 4: arm orchestration — token, listener thread, env bundle, outcome

The integration core. Generates the token, binds the platform `Listener`, spawns the
serve-loop on a thread, returns the env bundle to pass to `ssh`, and exposes an
`Outcome`. Inline owns the handle in a scope-guard; new-tab (when enabled) registers
in `App::askpass_listeners`.

**Files:** `src/os/askpass.rs`.

- [ ] **Step 1: design the public surface** (tests cover the env bundle + the
serve-loop outcome via the existing `Listener`):

```rust
/// Hex-encode the token for the env var.
fn token_hex(t: &Token) -> String { t.iter().map(|b| format!("{b:02x}")).collect() }

/// The env bundle (name, value) pairs to apply to the connect `Command`.
pub fn arm_env(channel_addr: &str, token: &Token) -> io::Result<Vec<(std::ffi::OsString, std::ffi::OsString)>> {
    use std::ffi::OsString;
    Ok(vec![
        (OsString::from("SSH_ASKPASS"), OsString::from(askpass_exe_path()?)),
        (OsString::from("SSH_ASKPASS_REQUIRE"), OsString::from("force")),
        (OsString::from(CHANNEL_ENV), OsString::from(channel_addr)),
        (OsString::from(TOKEN_ENV), OsString::from(token_hex(token))),
    ])
}

/// A live listener: a join handle + a stop flag + the terminal Outcome.
pub struct AskpassListener {
    handle: Option<std::thread::JoinHandle<Outcome>>,
    /* stop signal; the secrets bundle lives inside the worker (zeroized there) */
}
```

- [ ] **Step 2: the serve loop** — call `Listener::serve_one(&mut ConnectSecrets)`
until **both armed kinds are served-or-rejected** or a stop/timeout fires; compute the
`Outcome` (`Served{kind}` / `Declined{reason}` / `TimedOut` / `NotAttempted`). Tests
drive it through a real `Listener` + `connect_client` (mirror the Phase 1 channel
tests) and assert the `Outcome`.

- [ ] **Step 3: `arm_connect(identity, password, passphrase) -> io::Result<(AskpassListener, env)>`**
binds the listener, confirms it is accepting before returning (TOCTOU: caller spawns
`ssh` only after this returns Ok), spawns the serve thread, returns the env bundle.

- [ ] **Step 4: teardown** — `AskpassListener::stop_and_join() -> Outcome` (stop flag,
join, the worker has dropped its `ConnectSecrets` so secrets are zeroized). Inline uses
this as a scope-guard; new-tab's `drain_askpass` joins on the outcome signal.

> Cross-platform: keep `AskpassListener` + the serve loop platform-neutral; the
> `Listener` impl is already cfg-gated. Per the spec's clippy discipline, do not add an
> always-present field read by only one OS arm. `#[cfg(any(windows, test))]` for helpers
> a Linux test exercises.

- [ ] **Step 5: gates + commit** (`feat(askpass): arm orchestration — token, listener thread, env bundle, outcome`).

---

## Task 5: `connect.rs` env parameter (both cfg arms) + call sites

**Files:** `src/os/connect.rs`, `src/update.rs`.

- [ ] `run_ssh_inline(args: &[String], env: &[(OsString, OsString)])` applies
`Command::envs(env)` before spawn.
- [ ] `connect_new_tab(alias, args, env)` — **both** the `#[cfg(windows)]` and
`#[cfg(not(windows))]` arms gain `env`; the non-windows arm binds it `_env` to satisfy
the Linux unused-param clippy gate (the env is consumed only by the windows arm in v1,
since new-tab auto-fill is spike-gated). Apply `.envs(env)` to the spawned `Command`.
- [ ] Update the two call sites in `update.rs:307,318` to pass `&[]` initially (Task 8
threads the real bundle in). Keep the existing 114 tests green (connect.rs has
`build_ssh_args` tests; the env param is additive).
- [ ] gates + commit (`refactor(connect): thread an env bundle through inline + new-tab spawn`).

---

## Task 6: `main.rs` env-presence askpass dispatch

**Files:** `src/main.rs`.

- [ ] Before the arg loop (and before `ratatui::init()` / config load), call
`os::askpass::run_helper(std::env::args().nth(1), |k| std::env::var(k).ok())`:
  - `None` → not askpass mode → continue normal parsing.
  - `Some(bytes)` non-empty → write bytes to stdout, `exit(0)`.
  - `Some(empty)` → `exit(1)` (no stdout).
This runs before the `other => exit(2)` arm, so a prompt starting with `-` is never
misparsed (the spec's CLI test: `SSHM_ASKPASS_CHANNEL` set + `argv[1]=-x` routes to the
helper; absent → `--config`/`--list`/help/version parse normally).
- [ ] gates + commit (`feat(main): dispatch SSH_ASKPASS helper by env presence before arg parse`).

---

## Task 7: App state + `vault_secret_kinds` + drain + clearing

**Files:** `src/app.rs`, `src/update.rs`, `src/event_loop.rs`.

- [ ] App fields: `pending_connect: Option<PendingConnect>`, `askpass_listeners:
Vec<AskpassListener>`, `confirmed_password_targets: HashSet<String>`,
`password_autofill_enabled: bool` (default false). `PendingConnect { alias: String,
mode: ConnectMode, awaiting: Awaiting }`, `enum Awaiting { Unlock, PasswordConfirm,
Ready }`.
- [ ] `App::vault_secret_kinds(&self, host: &HostView) -> Option<MatchedKinds>`: the
**shared** candidacy predicate — calls `match_vault_kinds(host.patterns, &vault.entries)`
when unlocked, then **masks the Password kind when `!password_autofill_enabled`** (so a
password-only host downgrades to `None`/passphrase-only). Reads `self.vault` live (no
cached field). Returns `None` when locked.
- [ ] `App::drain_askpass(&mut self)`: reap finished `askpass_listeners` (join on the
outcome signal), emit the outcome toast, drop (zeroize) the bundle.
- [ ] Clearing: add `confirmed_password_targets.clear()` + stop/join/zeroize listeners
to `rebuild_hosts()` (alongside the existing liveness clear), the `L` lock action, and
`run()` quit cleanup (alongside the clipboard clear).
- [ ] event_loop: call `app.drain_askpass()` in the per-tick block next to
`drain_liveness()`.
- [ ] Tests: `vault_secret_kinds` masks password when disabled / returns both when
enabled / `None` when locked or proxied; indicator-vs-connect parity (spec "Indicator
parity").
- [ ] gates + commit.

---

## Task 8: connect dispatch interception (`connect_by_alias`)

**Files:** `src/update.rs`.

- [ ] Extract `connect_by_alias(app, terminal, alias, mode)`; route both
`connect_selected` and `handle_action_menu` through it.
- [ ] Dispatch per spec "Connect-time flow" steps 1–6: candidacy (`vault_secret_kinds`)
→ off-thread `resolve_config` (skip if `HostView` has `Match exec` via `has_match_exec`)
→ proxy check (`rc.proxy_jump`/`proxy_command`) → TOFU (`tofu_lookup_key` +
`is_host_known` over `rc` known_hosts files) → locked⇒defer `Unlock` → password⇒defer
`PasswordConfirm` → else arm + spawn. Each miss: connect normally, no env, with the
correct toast (gate-skip vs proxy-skip distinguished).
- [ ] v1: arm only for `ConnectMode::Inline` (new-tab passes `&[]` until the spike
flips `NEW_TAB_AUTOFILL`). Inline owns the `AskpassListener` in a scope-guard around
`run_ssh_inline`; teardown joins + zeroizes regardless of Ok/Err/panic.
- [ ] Tests per spec: proxy gate, TOFU gate, Match-exec (degrade without running
`ssh -G`; counter once per dispatch), scoping (env only on the Command).
- [ ] gates + commit.

---

## Task 9: deferred unlock → confirm → connect state machine

**Files:** `src/event_loop.rs`, `src/update.rs`, `src/app.rs`.

- [ ] The single terminal-bearing drain in `event_loop.rs`, immediately after
`update::handle_key(...)?` inside the `KeyEventKind::Press` block: `if let Some(pc) =
app.pending_connect.take()` → branch on `awaiting`: `PasswordConfirm` → open the modal +
re-defer with `awaiting=Ready`; `Ready` → run the connect with auto-fill then clear.
- [ ] `submit_vault_unlock` success when `pending_connect.is_some()`: set
`awaiting=PasswordConfirm` (if a password armed) else `Ready`, `Screen::List`,
`prev_screen=None`, **skip** the `Screen::Vault` transition + "vault unlocked" toast.
Clear the intent on every unlock exit (success/cancel) so it can't leak into a `P`-unlock.
- [ ] Re-derive at drain time via `connect_by_alias` (alias gone → toast + drop, never
panic). Compute `kinds` from `vault_secret_kinds` on the re-derived HostView.
- [ ] Tests: the two-keypress `PasswordConfirm → Ready` hop; unlock-then-connect resume;
`P`-unlock never auto-connects; Match-exec counter once across the hops.
- [ ] gates + commit.

---

## Task 10: `Screen::PasswordConfirm` (UI four-touch)

**Files:** `src/app.rs`, `src/update.rs`, `src/ui/mod.rs`, `src/ui/vault.rs` (or new
`ui/confirm`).

- [ ] (1) `Screen::PasswordConfirm { alias: String, mode: ConnectMode, kinds:
MatchedKinds }` (carries **no secret**). (2) `handle_key` dispatch arm: Enter = confirm
(add resolved `<user@host>` to `confirmed_password_targets`, `awaiting=Ready`, return to
List; the drain of *this* keypress runs the connect); Esc/`n` = decline (drop Password
from the armed set, keep Passphrase, `awaiting=Ready`). (3) draw arm in `ui/mod.rs`. (4)
`base_screen()` arm returning `prev_screen.unwrap_or(List)`.
- [ ] Render mirrors `vault::draw_unlock` (centered `modal_block`, shows resolved
`<user>@<host>`, "Enter confirm · Esc decline"). It is a consent/typo guard, not MITM
defense — copy that framing into the modal text.
- [ ] gates + commit.

## Task 11: pre-connect indicator glyph

**Files:** `src/ui/theme.rs`, `src/ui/list.rs` (+ detail).

- [ ] theme glyph constants: password-only / passphrase-only / both, + a muted
(`theme::FAINT`) variant for candidacy-match-but-not-yet-known. Active color from theme.
- [ ] In `draw_list_pane` row build (list.rs:79–87), prepend a glyph span computed from
`app.vault_secret_kinds(h)` (active) plus the muted variant when the host is a candidate
but `is_host_known` is false (the indicator consults the known_hosts probe **only** for
muted-vs-active — do NOT move the probe into `vault_secret_kinds`). Not shown while
locked.
- [ ] Tests: indicator predicate ≡ connect matcher (multi-pattern, glob, proxied).
- [ ] gates + commit.

## Task 12: outcome + gate-skip toasts

**Files:** `src/update.rs` / `src/app.rs` (`drain_askpass`), `src/ui` (existing toast).

- [ ] Emit the spec's exhaustive outcome toasts (`Served{kind}`+exit0/255,
`Declined{password|KeyboardInteractive}`, `NotAttempted`) from `drain_askpass`; the
pre-connect gate-skip toast (TOFU not-yet-known, distinct from the permanent proxy skip)
from the dispatch; the password-off discoverability toast (one-time per session).
- [ ] gates + commit.

---

## Self-review checklist (run before handoff)

- [ ] **Spec coverage:** connect-flow steps 1–8, state machine (two-keypress hop,
unlock resume, P-never-connects), App ownership (join not detach, zeroize on
quit/lock/rebuild), session-scoped confirm suppression (resolved `<user@host>`, blanket
`rebuild_hosts` clear), password opt-in mask folded into `vault_secret_kinds`, connect.rs
both-arm env param, indicator parity, outcome toasts, main.rs env dispatch before
`other=>exit(2)`, vault `\r\n` rejection.
- [ ] **Conservative-by-default:** password OFF; new-tab auto-fill gated off; every gate
miss degrades to a normal connect with no env (fails safe, never wrongly releases).
- [ ] **Cross-platform clippy:** no always-present single-OS-read field; Windows helpers
cfg-gated; linux cross-clippy green each task.
- [ ] **No regressions:** the 114 tests stay green; additive only until each wiring task.
- [ ] **Security:** secrets never on argv/env (only token+addr); helper + listener
zeroize; identity binding is the load-bearing release gate; TOFU gate arms only
already-known hosts; password-confirm is consent/typo guard, not MITM defense.
