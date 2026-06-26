# SFTP dual-pane browser — password auto-fill design

Date: 2026-06-27
Status: approved (brainstorm); pending implementation plan
Branch context: builds on the SFTP support (PR #28) and the vault auto-fill
connect path (PRs #13/#14, `os/askpass.rs`, `update.rs::connect_plan`)

## Problem

The dual-pane SFTP browser (`b` from the host list) cannot connect to a host
that authenticates by **password** (no key/agent). Opening the browser on such a
host leaves the remote pane stuck and surfaces the raw OpenSSH failure:

```
user@localhost: Permission denied (publickey,password).
```

Reproduced live against a real password-method `sshd` (WSL Debian on
`localhost:2222`, host key already trusted). The inline connect (`Enter`) and the
inline SFTP session (`F`) both auto-fill the stored password correctly; only the
dual-pane browser does not. We want the browser to auto-fill the stored password
the same way, so password-only hosts are browsable.

## Root cause (two distinct defects)

**1. `sftp -b` forces `BatchMode` on the ssh transport — askpass is never invoked.**
Every browse op runs as a short-lived `sftp -b <script> -- <alias>` child
(`os/sftp.rs::run_op`). Two things suppress auto-fill:

- `SftpSession` passes an explicit `-o BatchMode=yes` (`os/sftp.rs:217`/`:224`).
- **More fundamentally**, `sftp -b <batchfile>` makes the *sftp client itself*
  inject `-obatchmode yes` into the `ssh` subprocess it spawns. Verified via
  `sftp -vvv`:

  ```
  debug3: spawning "...\ssh.exe" ... "-obatchmode yes" ... -s -- WSL-Debian sftp as subprocess
  ```

  `BatchMode=yes` disables password/passphrase prompting **and** `SSH_ASKPASS`
  entirely. So merely deleting the explicit `-o BatchMode=yes` is **not enough** —
  the `-b` flag re-imposes it.

The browser also never arms an askpass listener and never consults the vault
(`open_sftp_browser`, `os/sftp.rs::SftpSession`), so even with BatchMode handled
there is nothing to serve.

**2. Pre-existing security gap (GAP 1/2): `arm_sftp_secrets` omits two blanket
gates that `connect_plan` enforces.** The inline-transfer arming
(`update.rs::arm_sftp_secrets`, used by **both** the guided transfer
`execute_sftp_transfer`←`submit_sftp_transfer` **and** the in-browser transfer
`browser_transfer`) enforces candidacy, Match-exec, resolve, and the
password-consent gate — but **not** the two blanket gates `connect_plan` applies:

- **not-proxied** (`connect_plan` returns `Normal` when `is_proxied`), and
- **host-known / TOFU** (`connect_plan` returns `Normal` when `!is_known`).

`connect_plan` (`update.rs:429-441`) is explicit that the TOFU gate is
"load-bearing for **both** kinds — not just the server-facing password": arming
sets `SSH_ASKPASS_REQUIRE=force`, which routes the host-key prompt to the helper
too, so an unknown host must stay un-armed. `arm_sftp_secrets` arming a passphrase
(or a session-consented password) for an **unknown or proxied** host is therefore
a live divergence from the connect-path threat model. This must be fixed
regardless of the browser feature.

## The Windows unknown — empirically resolved

The load-bearing question for in-browser auto-fill on the primary platform: does
`SSH_ASKPASS_REQUIRE=force` make Win32 OpenSSH `sftp` use the askpass helper for a
**background, piped-stdio** child (no inherited TTY in raw-mode-TUI context),
without falling back to the console (`CONIN$`) or hanging?

A spike against the real System32 `sftp.exe` + the WSL password host settled it
(probe = a tiny exe that records invocation + returns a wrong password;
`stdin=DEVNULL`, `stdout/stderr` piped, mirroring `Command::output()`):

| Config | `-o BatchMode` | askpass invoked? | result |
|--------|---------------|:---:|--------|
| current browser | `yes` (explicit + `-b`) | **no** | fast `Permission denied` |
| drop explicit only | (still `yes` via `-b`) | **no** | fast `Permission denied` |
| **`-o BatchMode=no`, inherited console** | `no` | **YES** | askpass served pw, 3 tries, clean fail |
| **`-o BatchMode=no`, `CREATE_NO_WINDOW`** | `no` | **YES** | askpass served pw, **no console, no hang** |

**Conclusion:** passing an explicit **`-o BatchMode=no`** overrides the
`-b`-implied batchmode, and Win32 OpenSSH `sftp` then invokes the armed askpass
helper for the password — **even with no console at all** (`CREATE_NO_WINDOW`),
with **no console fallback and no hang**. The full in-browser auto-fill is
therefore viable on Windows. `os/askpass.rs` needs **no changes**.

## Design

Two deliverables. STEP 0 ships regardless of the feature.

### A. STEP 0 — close GAP 1/2 in `arm_sftp_secrets` (mandatory)

Before arming, evaluate the two blanket gates `connect_plan` already computes,
applied to **both** password and passphrase, reusing the *same* helpers so the
SFTP and connect paths can never diverge again:

- not-proxied: `host.is_proxied() || rc.proxy_jump.is_some() || rc.proxy_command.is_some()`
- host-known/TOFU: `tofu_lookup_key(&rc).is_some_and(|k| is_host_known(&k, &known_hosts_files(&rc)))`

A miss returns `(None, Vec::new())` — the existing fail-safe-to-unarmed-transfer
pattern. This fixes the live threat-model violation on both transfer entry points
and is the highest-value, lowest-risk piece of the whole effort.

### B. In-browser per-op armed auto-fill (the feature)

Keep today's fast `BatchMode=yes` path for key/agent hosts untouched. Only when
the browsed host is a **true auto-fill candidate** — passing the *full* gate set
(vault unlocked, `password_autofill_enabled` for the password kind, exact-alias
vault match, no Match-exec, `ssh -G` resolved, **not proxied**, **host known**,
password session-consented, OpenSSH ≥ 8.5, and `tools().is_system32` so the
Git/MSYS `[PATH ssh]` build is never used) — open the session in an **armed**
mode:

- The browse `run_op` for an armed session passes **`-o BatchMode=no`** (instead
  of `-o BatchMode=yes`) plus the askpass force-env bundle.
- **Per op, a fresh `arm_connect`** mints its own `AskpassListener` +
  `ConnectSecrets` (`password_served = false`). Each op is a separate ssh connect,
  so the per-connect single-shot is consumed exactly once per op **by
  construction** — `os/askpass.rs` and its single-shot logic are untouched. After
  the op's `sftp` exits, the op calls `listener.stop_and_join()` (secrets
  zeroize).
- The identity (`ResolvedIdentity`) + which kinds to arm are computed **once** at
  browser open (one `ssh -G`, the gate decision). Per op, the UI-thread dispatch
  re-gathers the `Secret`s from the still-unlocked `app.vault` and hands them to
  the worker, so no extra plaintext residency beyond the already-unlocked vault.

A new shared helper `compute_sftp_arm(app, host, alias) -> Option<SftpArm>`
encodes the gate decision (STEP 0 gates + the rest) and is used by **both** the
inline transfer path and the browser, so the two never diverge.

### Error handling & fallbacks

- **First-failure circuit-breaker (Windows lockout guard).** Windows has no
  ControlMaster, so N navigations = N fresh auth attempts. A stale/wrong vault
  password would otherwise rack up N server-side failures (fail2ban/AD/faillock).
  On the **first** armed op that fails with an auth error, the session reverts to
  `BatchMode=yes` (un-armed) for the rest of its life and surfaces a toast
  ("stored password rejected — re-check the vault, or press F"). One failed
  attempt, not N.
- **`F` steer remains as fallback.** When the host is *not* an armable candidate
  (vault locked, password un-consented, OpenSSH < 8.5, non-System32 ssh, or the
  circuit-breaker tripped), keep the current behavior plus an advisory steer to
  the inline `F` session (which routes through the fully-gated `connect_by_alias`).
  A new in-browser `KeyCode::Char('F')` (the `_ => {}` arm at
  `update.rs:1834`) resolves the alias from **`b.host`** (bounds-guarded; never
  the list cursor) and calls `connect_by_alias(Inline, Protocol::Sftp,
  PasswordChoice::Ask, …)`.
- **Any arm failure degrades to a plain `BatchMode=yes` op** (never blocks the
  browse).

### Discoverability

- Add an `F sftp` pair to `SFTP_BROWSER_FOOTER` (`ui/mod.rs`), staying within the
  `footers_fit_80_cols` ≤80-col budget (shorten/drop an existing pair).
- Add the `F` line to the `SftpBrowser` help block (`ui/help.rs:94-101`).

## Cross-platform behavior

- **unix:** the existing ControlMaster (`ControlMaster=auto`/`ControlPersist`)
  means the first armed op authenticates once (via askpass) and later ops reuse
  the master — `-o BatchMode=no` lets the master auth by password; subsequent ops
  pay no re-auth. The per-op fresh listener still works (only the first op's
  master actually prompts).
- **Windows:** no ControlMaster → each op re-authenticates. Per-op latency is one
  SSH handshake + auth (~0.2–3 s per directory open). Acceptable and unavoidable;
  the circuit-breaker bounds the failure cost.
- **clippy `-D warnings` (Linux gate):** any new `#[cfg(windows)]`-only symbol
  must stay cross-platform-referenced (the `find_wt`/`escape_wt_arg` trap). Keep
  the armed/un-armed `common_args` split symmetric across `cfg(unix)` /
  `cfg(not(unix))`.

## Security analysis

- **Gate parity.** `compute_sftp_arm` enforces the *same* gate set as
  `connect_plan`, from the same helpers — including the STEP 0 TOFU + not-proxied
  blanket gates. No host is armed that `connect_plan` would refuse.
- **Identity binding unchanged.** `arm_connect` binds secrets to the `ssh -G`
  resolved `user@host` and IdentityFile paths exactly as the connect path; the
  per-op listener is the same code.
- **Single-shot intact.** Each op gets a fresh `ConnectSecrets`
  (`password_served=false`); the password is served at most once per op (= per
  connect), so the per-connect single-shot invariant is preserved, not relaxed.
  `os/askpass.rs` is not modified.
- **No new on-screen oracle.** The steer message must not reveal vault contents
  (no "this host has a stored password" suffix triggered by a remote-induced
  failure).
- **`is_auth_failure` classifier** (for the steer + circuit-breaker) scans **all**
  stderr lines for the parenthesized method-list form (`Permission denied (`),
  `Authentication failed`, `Too many authentication failures` — never bare
  `Connection closed` (collides with KEX/host-key phase). A directory-ACL
  `Permission denied` with no `(method-list)` is a negative; a server banner
  containing the substring must not forge a positive.

## Testing strategy

- **Unit (GAP 1/2):** `compute_sftp_arm` gate decision returns un-armed for a
  proxied host (ProxyJump and ProxyCommand) and an unknown host, for both a
  passphrase-only and a both-kinds host; still arms a known, non-proxied,
  consented host. Mirror the `connect_plan` truth-table tests.
- **Unit:** `is_auth_failure` — positive on real auth-failure forms; negative on
  directory-ACL `Permission denied`, bare `Connection closed`, and a banner that
  merely contains the substring before a successful listing; fed multi-line
  stderr.
- **Unit:** armed `run_op` builds the sftp args with `-o BatchMode=no` + the
  force-env for an armed session, and `-o BatchMode=yes` (no env) otherwise.
- **Headless:** the in-browser `F` resolves the alias from `b.host` (not the list
  cursor); an out-of-bounds `b.host` yields a toast, not a panic.
- **Regression:** the `os/askpass.rs` single-shot tests remain untouched and green
  (a signal the approach doesn't disturb the crown-jewel module).
- **UI width:** `footers_fit_80_cols` after the footer edit, on Linux and Windows.
- **CI gates on both OSes:** `cargo fmt --all -- --check`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test --all`.
- **Manual (throwaway `--config` + scratch vault):** against the WSL password
  host — armed candidate browses and lists via auto-fill; a wrong stored password
  trips the circuit-breaker after exactly one failure; a key/agent host browses
  unchanged (still `BatchMode=yes`, no steer).

## Out of scope (YAGNI)

- No persistent interactive `sftp` REPL (the module deliberately avoids
  sentinel-framed REPLs; per-op `-b` + per-op arming is sufficient and was
  proven on Windows).
- No change to `os/askpass.rs` / the single-shot model.
- No new-tab (`t`) auto-fill (still gated off by `NEW_TAB_AUTOFILL`).
- Keyboard-interactive / MFA servers remain unsupported for password auto-fill
  (unchanged from the connect path).
