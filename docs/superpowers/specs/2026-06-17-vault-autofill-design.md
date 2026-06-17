# Vault auto-fill — design

Date: 2026-06-17 (rev 3, 2026-06-18)
Status: approved (brainstorm); **revised twice after multi-angle adversarial
reviews** (round 1: 23 findings; round 2: 29 findings, incl. a new TOFU blocker);
pending implementation plan
Branch context: builds on the encrypted password vault (`os/vault.rs`, PR #13)

## Problem

Stored secrets (per-host login passwords and key passphrases) can only be used
today by copying them to the clipboard (`y`/`c`) and pasting into OpenSSH's
prompt. That is tedious and leaves the plaintext in the clipboard. We want sshm
to supply the secret to `ssh` directly when connecting, for **both** key
passphrases and login passwords.

## Why the naive approaches do not work

OpenSSH reads passwords/passphrases from the controlling terminal, not stdin —
`echo pw | ssh host` is ignored. `sshpass` (the Linux PTY trick) does not work
with Windows OpenSSH. The supported escape hatch is `SSH_ASKPASS`: OpenSSH
invokes an external helper to obtain a passphrase/password. With
`SSH_ASKPASS_REQUIRE=force` (OpenSSH 8.4+) the helper is used **even when a TTY
is present** — and, critically, the TTY is then **not** used as a fallback (see
"The force reality" below).

> Binary note: sshm resolves `ssh` via `os::binaries::tools().ssh`, which prefers
> the **System32 OpenSSH** (`OpenSSH_for_Windows`, 9.5p2 on this machine), not the
> Git/MSYS build that `where ssh` surfaces first (10.3p1). The spike (Step 0) must
> exercise the binary `tools()` actually resolves, and the `[PATH ssh]` fallback.

## Resolving the effective config — use `ssh -G`

Several controls below need the *effective* connection identity (resolved
`HostName`, `User`, `IdentityFile`, and whether any proxy applies). The literal
`HostView` is **not** sufficient: it is the host's own block only, so it misses
globally-inherited `Host *` / `Match` / `Include`d directives, and it holds raw
unexpanded tokens. **Before arming auto-fill, sshm runs `ssh -G <alias>`** (via
the resolved `tools().ssh`) and parses `hostname`, `user`, `identityfile`
(possibly several), `proxyjump`/`proxycommand` from its output. This single move:

- gives the real `user@hostname` to compare against the password prompt and to
  show in the confirm modal,
- gives the resolved IdentityFile path(s) to bind passphrase prompts,
- surfaces **inherited** proxy directives that `HostView::is_proxied()` cannot see.

If `ssh -G` fails, times out, or reports a dynamic identity (CanonicalizeHostname,
`Match exec`, `%C`/non-`%h` tokens), **degrade to manual entry** (no auto-fill).

## Decisions (brainstorm + 2 review rounds)

| Question | Decision |
|----------|----------|
| Trigger | **Automatic on connect** for passphrases. Passwords are **automatic but gated by a one-time-per-session, per-resolved-target confirmation** (see below), and **default to opt-in/OFF until the Step-0 spike confirms the per-server-method behavior**. |
| Targets | **Both** key passphrases and login passwords, hardened differently (passphrase is local/unphishable; password is server-facing/phishable and only fires for the dedicated `password` method). |
| Connection paths | Inline (`Enter`) always; **new-tab (`t`, `wt.exe`) contingent on the env-inheritance spike**. If the spike fails, new-tab ships without auto-fill (inline-only). |
| Locked vault on connect | **Prompt for the master password, then auto-fill.** Cancel → connect normally, no auto-fill. |
| Unknown host (TOFU) | **No auto-fill unless the resolved host is already in `known_hosts`** — force hijacks the host-key confirmation and would abort the connect (see blocker below). |
| Confirm cadence | Password-confirm shown **once per resolved `<user@host>` per unlocked-vault session** (session-scoped, never persisted); cleared on lock/quit/host-edit. |
| Mechanism | **Method A — sshm itself as the `SSH_ASKPASS` helper** (selected by env, not a flag), with classification + identity binding performed in the **listener** (the trusted TUI process), not the helper. |
| Proxied hosts | **No auto-fill for proxied hosts in v1** — detected via `ssh -G` (not just `is_proxied()`). Hop-aware multi-secret is a follow-up. |

Rejected: ssh-agent+askpass hybrid (more moving parts; possible follow-up);
PTY/ConPTY scraping (fragile on Windows).

## The force reality (this shaped the whole design)

`SSH_ASKPASS_REQUIRE=force` flips `allow_askpass=1` independent of `DISPLAY` at
the top of `read_passphrase`, **before** any DISPLAY/isatty/TTY check, so for the
**entire lifetime** of that `ssh` process **every** prompt routed through
`read_passphrase` goes to the helper and **never** to the terminal. This includes
prompts the helper should not answer. Verified against openssh-portable
`readpass.c` / `sshconnect.c` / `sshconnect2.c`.

- **No in-process manual fallback.** Once `ssh` starts under force, the human
  cannot answer any prompt at the TTY for that connection.
- **Host-key (TOFU) confirmation is hijacked too** — see the blocker below.
- **A non-zero/empty helper reply is NOT a benign no-op for server-facing prompts.**
  `read_passphrase` returns `xstrdup("")` when the caller did not set
  `RP_ALLOW_EOF`. The callers behave differently:
  - **Password method** (`userauth_passwd`, flags=0): **no** empty-check — the
    empty string is sent to the server, burning one of the (default 3)
    `NumberOfPasswordPrompts` attempts.
  - **Keyboard-interactive / PAM** (`input_userauth_info_req`, flags=0): **no**
    empty-check either — the empty string is packed and sent, **also burning an
    attempt**. (This is the path most Debian/Ubuntu-PAM and all MFA/OTP servers
    use; see the password-method gap below.)
  - **Passphrase** (`load_identity_file`, flags=0): the caller **does** guard
    `if (*passphrase=='\0') break;` — so an empty reply skips that key locally
    (benign, no server traffic).

We accept that an auto-fill miss on a password-method host costs an attempt and
shows "Permission denied". Key-based auth + agent remains the recommended posture.

## BLOCKER fix — gate arming on known-host status (TOFU)

Under force, `sshconnect.c confirm()` (the "Are you sure you want to continue
connecting (yes/no/[fingerprint])?" prompt) calls `read_passphrase(msg, RP_ECHO)`
— no `RP_ALLOW_EOF`, no opt-out — so it routes to the helper. The helper returns
nothing (it is not a secret), `read_passphrase` yields `""`, and `confirm()`
treats `p[0]=='\0'` as **"no" → "Host key verification failed" → abort**. The
helper cannot return "yes" (that would defeat TOFU and is not its job). So
**arming auto-fill would abort every first connect to a not-yet-known host** — the
exact headline workflow (add host, store password, connect).

**Fix:** before arming (setting the force env), require the resolved
`HostName`/`HostKeyAlias` to already be present in `known_hosts` — probe via
`ssh-keygen -F <host>` or reuse `os/known_hosts::parse_known_hosts()` /
`known_hosts_path()`. If absent → connect **normally with no askpass env** so the
user accepts the key at the terminal/tab; offer auto-fill on a later connect once
the key is trusted. There is no force-compatible way to both auto-fill and let the
human accept an unknown key.

## Architecture

All new logic lives in the **`os/` layer** (zero ratatui dependency). The UI only
triggers connects, renders the unlock + password-confirm modals, and shows
indicators/toasts.

### Connect-time flow (saved-host connect only)

Connect interception lives **inside `connect_selected()`** (update.rs:293) so both
entry points are covered: `handle_list` (Enter/`t`) and `handle_action_menu`
(Enter on sel 0/1). All connects resolve the selection to an alias and delegate to
a `connect_by_alias(alias, mode)` helper.

1. Look up vault entries matching the alias (see "Host ↔ secret matching"). Applies
   to the saved-host path (`ConnectOverrides::default()`); ad-hoc override connects
   are out of scope for v1. No match → connect normally, silent.
2. Run `ssh -G <alias>`. If it reports any `proxyjump`/`proxycommand` (own block OR
   inherited), or a dynamic/unresolvable identity → connect normally, **no askpass
   env**.
3. Check the resolved `HostName`/`HostKeyAlias` is in `known_hosts`. If not → connect
   normally, **no askpass env** (TOFU blocker above).
4. If the vault is **locked** → store `PendingConnect { alias, mode, awaiting:
   Unlock }`, open the existing `VaultUnlock` modal. On success → continue this
   flow; on cancel/Esc → discard the intent and connect normally. The intent is
   cleared on **every** unlock exit (success, cancel) so it cannot leak into a
   later `P`-opened unlock.
5. If the matched secrets **include a Password** AND password auto-fill is enabled
   AND the resolved `<user@host>` is **not** already in
   `App::confirmed_password_targets` → show the one-time **password-confirm modal**
   displaying the resolved `<user>@<host>` (a consent/typo guard — see Security; it
   is **not** a redirect/MITM defense). Confirm → arm the password and add the
   target to the session set; decline → connect with passphrase still armed (if
   any) but **password withheld**. **Passphrase needs no confirmation.**
6. `os/askpass` generates a per-connect token and starts the channel **listener**
   (holding only the armed secrets + their resolved identities), **confirms it is
   accepting before** building the `ssh` command (TOCTOU: a helper connecting
   before the listener is ready fails closed → manual entry), then spawns `ssh`
   with env:
   - `SSH_ASKPASS = <absolute, prefix-normalized path to the running sshm exe>`
   - `SSH_ASKPASS_REQUIRE = force`
   - `SSHM_ASKPASS_CHANNEL = <pipe/socket address>`
   - `SSHM_ASKPASS_TOKEN = <256-bit random, hex>`
7. When `ssh` needs a secret it execs `sshm "<prompt text>"` (argv[1] = raw
   prompt, **no flag**). The helper relays `[token][prompt]`; the **listener**
   verifies the token (constant-time), classifies + identity-binds + applies
   single-shot, and returns **the one matching secret or a zero-length reply**.
8. Inline: teardown (stop + join + zeroize) runs as a scope-guard after `ssh`
   exits. New-tab: the listener tears down on first-both-served-or-rejected or
   `NEW_TAB_ASKPASS_TIMEOUT`, whichever first.

### The askpass helper — `sshm <prompt>` (selected by env, not a flag)

OpenSSH calls `execlp(askpass, askpass, msg, NULL)` — argv[1] is the raw prompt,
there is **no flag**. The helper therefore enters askpass mode **iff
`SSHM_ASKPASS_CHANNEL` is present in the environment**, and this check runs
**first, before any `--config`/`--list`/`other` arg parsing**, so a prompt that
begins with `-` (localized or server-controlled text) is never misparsed as a
flag. In askpass mode it takes `argv[1]` (if present) verbatim as the opaque
prompt and ignores all flags/other args.

It **short-circuits**: no `resolve_path()`, no SSH config load, no `App`, no
ratatui. It reads `SSHM_ASKPASS_CHANNEL`/`SSHM_ASKPASS_TOKEN`, connects, relays
`[token][prompt]`, prints the returned secret bytes, zeroizes, exits. A missing
prompt, unreachable channel, token mismatch, or zero-length reply → exit non-zero
with **no stdout**. (Optionally still name the branch `Command::Askpass(String)`,
but it is dispatched by env detection, not by matching a token.)

### Classification + identity binding (in the LISTENER, not the helper)

Putting the decision in the listener keeps both secrets out of the helper process
and lets the trusted TUI process bind the secret to the **`ssh -G`-resolved**
identity. The listener holds, per armed connect, the resolved `<user>@<host>` and
the resolved IdentityFile path(s).

- **Passphrase** (local, unphishable): release only if the prompt matches the
  literal key form `Enter passphrase for key '<path>': ` **and** `<path>` matches
  an expected IdentityFile. Match algorithm: expand each expected IdentityFile the
  way OpenSSH does — tilde-expand (`~`, `~user`), percent-expand the client tokens
  OpenSSH resolves for IdentityFile (`%d` home, `%h` resolved HostName, `%u`
  remote user, `%i` uid, `%l`/`%L` localhost), make absolute, normalize separators
  (treat `/`≡`\` on Windows), compare case-insensitively on Windows /
  case-sensitively on unix, and account for the prompt's `%.100s` truncation
  (compare the first 100 bytes). Any unresolvable token (`%C`, dynamic HostName,
  unknown `~user`) → do **not** auto-fill the passphrase.
- **Password** (server-facing, phishable): release only if the prompt **equals
  exactly** `<user>@<host>'s password: ` — an **anchored full-string** match with
  **no** leading `(user@host) ` instance prefix — where:
  - `<host>` = `HostKeyAlias` if set (compared **verbatim**, outside the lowercase
    fold), else `lowercase(ssh -G hostname)` (ASCII-fold both sides; beware IDN),
  - `<user>` = `ssh -G user` (which already resolves the OpenSSH default = the
    local OS account when `User` is unset). If `<user>` cannot be resolved, or the
    resolved user differs from the prompt's, → do **not** auto-fill (degrade like a
    dynamic HostName).
  Rejecting any prompt that begins with the OpenSSH ≥8.5 `(user@host) `
  keyboard-interactive instance prefix is a **deliberate, load-bearing
  discriminator** that backstops the anchored match (Step 0 confirms the resolved
  build emits it; pre-8.5 builds degrade password auto-fill to manual).
- **Everything else** — bare `Password:`, `Verification code:`, `One-time password
  (OATH):`, `Token:`, Duo, `[sudo] password for`, localized strings, any
  `(user@host) …` keyboard-interactive prompt (even one crafted to end in `'s
  password: `) → return a zero-length reply.

**Single-shot (per kind for password, per resolved path for passphrase):**
- **Password:** served at most **once per connect** — a wrong password must never
  be replayed across the server's `NumberOfPasswordPrompts` attempts (fail2ban /
  PAM faillock / AD lockout).
- **Passphrase:** served at most **once per resolved IdentityFile path** per
  connect. The listener tracks the set of already-served expanded paths; a second
  request for an **already-served** path returns nothing (OpenSSH retries one key
  up to `NumberOfPasswordPrompts` with the identical prompt), but a request for a
  **different** expected path is served — so OpenSSH's multi-IdentityFile fallback
  (key A rejected → key B, same stored passphrase) succeeds instead of feeding
  key B an empty (force-skipping) passphrase.

The channel accepts N sequential helper connections over its lifetime; the
single-shot rules — not a numeric cap — bound useful serving.

### Channel + wire protocol

A **named pipe** (Windows) / **unix-domain socket** (Unix):

- Windows: a **single persistent pipe instance** (`nMaxInstances=1`) named
  `\\.\pipe\sshm-askpass-<CSPRNG-suffix ≥128 bits>` (reuse `random_bytes` from
  vault.rs), created once with `FILE_FLAG_FIRST_PIPE_INSTANCE` + a
  `SECURITY_ATTRIBUTES` whose DACL contains **only the current user's SID** (the
  default named-pipe SD grants read to Everyone + anonymous). To accept each of
  the N sequential helpers it loops `DisconnectNamedPipe → ConnectNamedPipe` on the
  **same secured handle** — it never recreates the name (a recreated instance loses
  squat-protection) and never opens additional instances, so the SD +
  squat-protection cover the whole lifetime. On token mismatch / short read /
  unclassifiable prompt it Disconnects that one connection and loops back to
  ConnectNamedPipe (it does **not** tear down the listener).
- Unix: a socket inside a `0700` directory (the dir + per-connect token is the
  **real** v1 boundary, a full implementation, no stub). `SO_PEERCRED` peer-uid
  check is an **optional**, dep-gated hardening — not the stable-std baseline.

Wire protocol (concrete target for the named tests):
1. Helper writes the token (32 raw bytes), then the prompt (`u32-LE length` +
   UTF-8 bytes).
2. Listener compares the token in **constant time**; on mismatch, Disconnect with
   no reply and await the next helper.
3. Listener classifies + identity-binds + single-shots; on a match it writes the
   chosen secret as `u32-LE length` + raw bytes; on no-match a **zero length**
   (helper then exits non-zero).
4. Types: a wire-reply secret read into a `Secret`, and a listener-side
   `ConnectSecrets` holding both kinds + their resolved identities + per-kind /
   per-path **served** state — both wrapping `Secret` so `Drop` zeroizes. (The
   classification runs in the listener; the helper never holds a bundle.)

The token is the **cryptographic gate**; the ACL / dir-perms is defense-in-depth.

#### Dependency posture

`Cargo.toml` adds **`windows-sys`** (smaller, no proc-macro), feature-gated to
`Win32_System_Pipes`, `Win32_Security`, `Win32_Security_Authorization` (SDDL),
`Win32_Foundation`. It provides the **entire** Windows named-pipe server —
`CreateNamedPipeW`, the `SECURITY_ATTRIBUTES`/SDDL DACL, `ConnectNamedPipe`,
`DisconnectNamedPipe`/`CloseHandle` — none of which exist in Rust std; it is
load-bearing IPC, not optional hardening. The Windows arm is unsafe FFI confined to
create/secure/connect/disconnect; once the `HANDLE` exists it may be wrapped via
`FromRawHandle` into `std::fs::File` for the byte-mode read/write loop. I/O model:
**blocking reads on a dedicated listener thread** (mirroring `os/liveness.rs`), not
overlapped I/O. The vault file keeps its existing best-effort **icacls**
`restrict_acl` (icacls cannot secure a `\\.\pipe\` kernel object that has no
filesystem path); this filesystem-ACL vs kernel-object-ACL asymmetry is intentional
(migrating `restrict_acl` to a shared SID-DACL helper is an out-of-scope follow-up).

### Stdout framing (helper → ssh)

The helper writes the secret's **raw UTF-8 bytes followed by exactly one `\n`**,
nothing else — no trim, normalize, or re-encode. OpenSSH reads ≤1023 bytes and
truncates at the first `\r`/`\n`. Therefore:

- A secret containing `\r`/`\n` cannot survive → **reject at vault-entry time**
  (preferred) and/or the helper refuses to serve it rather than truncating.
- A secret >1023 bytes → documented limit; helper refuses/falls back.

### `SSH_ASKPASS` path normalization

Set `SSH_ASKPASS` to `std::env::current_exe()` as an absolute path, but **strip a
leading `\\?\` verbatim prefix** (`\\?\UNC\` → `\\`) — via `Path::components()` /
`Prefix` matching with a `GetModuleFileNameW` fallback — because Win32-OpenSSH's
`CreateProcessW` rejects a `\\?\`-prefixed argv0 (`current_exe()` emits it when
installed at a path > 260 chars). Spaces/parentheses in the path are fine (argv-style
spawn), so no quoting is needed there.

### Lifecycle — inline vs new-tab

**Inline (`Enter`):** the TUI thread blocks in `run_ssh_inline`, so the listener
runs on a **separate thread**, spawned (and confirmed accepting) before the ssh
run. Teardown (stop + join + zeroize) runs as a **scope-guard** so it executes
regardless of `run_ssh_inline`'s Ok/Err and of an early `?` from `restore_tui` or a
panic-unwind (the crate is `panic="unwind"`). The inline listener is owned by this
scope, **not** registered in `App`.

**New tab (`t`, `wt.exe`):** fire-and-forget, so each connect spawns a **background
listener thread** with `NEW_TAB_ASKPASS_TIMEOUT = 60s` (named, tunable). It tears
down on whichever comes first: (a) **both armed kinds served-or-rejected**, or (b)
the timeout — tearing down on the *first* serve would starve a second armed kind
(Password + Passphrase on one host). The 60s upper bound is sized (Step 0) against
high-latency jump/PAM factors **only** — under the known-hosts gate, the host-key
prompt is no longer an in-band concern. **Env inheritance is the open risk:**
`wt.exe -w 0` hands the command to an **already-running** Terminal host, so the
tab's `ssh` may load a fresh environment and **not** inherit the per-connect env
(microsoft/terminal #15066/#15430/#15496; `compatibility.reloadEnvironmentVariables`
defaults on). The spike tests the `-w 0` attach case specifically; if env does not
arrive, new-tab ships **without** auto-fill for v1. Enablers to evaluate (not
assume): `--inheritEnvironment`, a fresh instance (`-w -1`), or a transient launcher
shim that sets the env inside the tab's own process before exec'ing ssh.

### App ownership of listeners (mirror liveness, but join — do not detach)

The detached new-tab listeners hold plaintext, so `App` owns them like the liveness
pool, with one key difference:

- `App::askpass_listeners: Vec<AskpassListener>` — each holds a `JoinHandle`, a
  stop signal, and an independent `Zeroizing` secret bundle.
- Each detached listener signals a **terminal outcome** (`Served{kind}` /
  `Declined{reason}` / `TimedOut` / `NotAttempted`) over its own mpsc (or an
  `AtomicBool`) once its worker has dropped its secret copy. `drain_askpass()`
  (called from `event_loop` alongside `drain_liveness()`) treats that signal as
  "finished" and **`join()`s** the handle (unlike `LivenessProbe`, whose handles
  are harmlessly detached because they hold no secrets) so the worker has dropped
  its secret before the `Zeroizing` bundle is freed. `is_finished()` is only a
  non-blocking reap *hint*, never a substitute for the join.
- **Zeroize on quit AND on lock:** `run()`'s cleanup (today clipboard-only)
  stop+join+zeroizes every listener in `askpass_listeners` on quit; the `L` lock
  action does the same. **Reachability caveat:** `L` is bound only on
  `Screen::Vault`, while new-tab connects originate from `Screen::List`, so a
  detached listener's plaintext lifetime is bounded by **quit OR
  NEW_TAB_ASKPASS_TIMEOUT, whichever first** — `L` is an opportunistic
  early-zeroize once the user navigates into the vault, not an always-available
  panic button. (Optional follow-up: bind `L`/zeroize in `handle_list`.)

### Deferred unlock → confirm → connect state machine

`PendingConnect { alias, mode, awaiting }` where `awaiting ∈ { Unlock,
PasswordConfirm, Ready }`:

- **Connect dispatch (already unlocked):** match → if a Password is armed set
  `awaiting=PasswordConfirm` else `Ready`; store the intent, return to `List` (the
  keypress handler does not connect).
- **`submit_vault_unlock` on success when `pending_connect.is_some()`:** set
  `awaiting = PasswordConfirm` (if a Password is armed) else `Ready`, set
  `Screen::List`, and **skip** the `Screen::Vault` transition AND the "vault
  unlocked" toast. When `pending_connect.is_none()` → today's behavior
  (`Screen::Vault` + toast). An unlock opened via `P` never auto-connects.
- **The single terminal-bearing drain** lives in `event_loop.rs` immediately after
  `update::handle_key(app, key, terminal)?`, inside the `KeyEventKind::Press` block
  (the intent is only ever set from a keypress, so it need not run on tick-only
  iterations). It does `if let Some(pc) = app.pending_connect.take() { ... }` and
  branches on `awaiting`: `PasswordConfirm` → open the confirm modal, **re-defer**
  (re-set the intent with `awaiting=Ready`); `Ready` → run the connect with
  auto-fill, then clear. `take()` must not lose the intent across the two-hop
  deferral.
- **Re-derive at drain time:** the drain re-finds the `HostView` by alias via
  `connect_by_alias`; if the alias no longer resolves it toasts and drops the
  intent (never panics). This naturally re-evaluates proxy/identity. (Not a race
  fix — the architecture is single-threaded with no file watcher; it is defensive
  robustness.)
- **Confirm modal contract** — a `Screen::PasswordConfirm { alias, mode, kinds }`
  variant carrying **no secret** (only alias + mode + which kinds are armed). Open
  it by directly setting `app.screen` from the drain. Keybindings: Enter = confirm
  (add to `confirmed_password_targets`, set `awaiting=Ready`, return to List for
  the same drain); Esc/`n` = decline (drop the Password from the armed set, keep
  Passphrase, set `awaiting=Ready`, return to List). Adding this Screen is the
  standard **four-touch** contract (mirroring `VaultUnlock`): (1) the `Screen` enum
  (app.rs), (2) a dispatch arm in `handle_key` (update.rs), (3) a draw/overlay arm
  in `ui/mod.rs` `draw()`, (4) a `base_screen()` arm in `ui/mod.rs` returning
  `app.prev_screen.unwrap_or(Screen::List)` so the right base renders underneath.

### Session-scoped password-confirm suppression

`App::confirmed_password_targets: HashSet<String>` keyed on the **resolved**
`<user@host>` (not the bare alias). Populated on confirm; the modal is skipped when
the resolved target is already present. **Cleared on lock (alongside zeroizing
listeners), on quit, and in `rebuild_hosts()`** (which already runs on every config
save and clears the liveness maps), so a mid-session HostName/User edit re-requires
confirmation. Suppression and plaintext are forgotten at the same lock boundary, so
it adds no disk/persistence exposure. Per-host **persistent** (cross-session) trust
stays out of scope.

### connect.rs signatures

`run_ssh_inline(args, env: &[(OsString, OsString)])` and
`connect_new_tab(alias, args, env)` apply `Command::envs(env)` before spawn (the two
existing call sites pass `&[]`). `connect.rs` does **not** start the channel or own
the listener — `os/askpass` generates `(channel_addr, token)`, starts the listener,
and returns the env bundle to the caller, which passes it in. `SSH_ASKPASS` is the
normalized absolute `current_exe()` path, resolved in `os/`.

## Host ↔ secret matching

- Match `HostView::alias()` (the bare ssh destination) — and **any** pattern on a
  multi-pattern Host line — against `VaultEntry.host`. Case-sensitive (consistent
  with the verbatim alias passed to ssh); the stored host is trimmed on save, so
  surrounding-whitespace mismatch cannot occur. Ignore any `user@` prefix on a
  future ad-hoc destination.
- **Never** auto-fill when the matched alias contains a glob (`*`, `?`) or negation
  (`!`).
- Note the layering: this host↔entry match decides *candidacy* (which secrets to
  load + whether to show an indicator); the **listener's** `ssh -G`-resolved
  identity binding decides *release*. A multi-pattern host may be a candidate yet
  still not release a password if the resolved HostName never equals the prompt's.
- A host may have a Password and/or Passphrase entry; armed secrets are loaded into
  the listener and released per the verified prompt.
- No match → connect normally; **stay silent** (no nudge, to avoid noise on every
  keyless host).

## Discoverability / user-facing feedback

- **Pre-connect indicator (shared predicate):** a host shows a lock/key glyph iff
  it would actually auto-fill — i.e. it passes the **same candidacy predicate** as
  connect dispatch: a vault entry matches any pattern (case-sensitive, not a
  glob/negation) and the host is not proxied. Implement as a single shared
  `App::vault_secret_kinds(host) -> Option<MatchedKinds>` (a pure host↔entry match,
  **not** the listener's release logic) that both the indicator and connect call,
  so they cannot diverge. Read `app.vault` directly each render (no cached field)
  so it stays correct across lock/unlock/`rebuild_hosts`. Optionally distinct
  glyphs for password-only / passphrase-only / both. Not shown while locked. Glyph
  color from `ui/theme.rs`. For the **password kind** the glyph must convey the
  weaker promise (auto-fills only when the server uses the `password` method;
  keyboard-interactive/PAM/MFA falls back), not over-promise.
- **Post-fill toast (inline)** — exhaustive over the four outcomes:
  - `Served{kind}` then exit 0 → "auto-filled <kind> for <alias>".
  - `Served{kind}` then exit 255 → "auto-fill secret rejected — check the stored
    <kind> for <alias>".
  - `Declined{password}` then exit 255 → "auth failed for <alias> — you declined
    auto-filling the stored password (force gives no manual prompt; press P to copy
    it, or reconnect to auto-fill)".
  - `Declined{KeyboardInteractive}` (a stored Password existed but the server used
    keyboard-interactive, so it was withheld) → "password not auto-filled: server
    used keyboard-interactive — enter it manually".
  - `NotAttempted` then exit 255 → "auth failed — the stored secret for <alias> was
    never requested (key/other factor failed)".
- **New-tab feedback:** if new-tab auto-fill ships, replace the bare "opened new
  tab" toast with an armed-state acknowledgement, e.g. "new tab: ssh <alias>
  (password auto-fill armed ~60s)", since per-fill outcomes are unobservable. If a
  mode is left on manual entry (spike failed), `t` connects exactly as today with
  **no** lookup and **no** confirm modal — never show a confirm modal when nothing
  will be filled.

## Error handling (honest about force)

- With `force`, there is **no terminal fallback within a running connect**.
- A zero-length reply on the **password OR keyboard-interactive** path sends an
  empty string and burns one attempt; on the **passphrase** path the key is
  skipped (benign).
- The real escape hatch to retain manual entry is to connect with **no askpass env
  at all** (neither secret armed → force not set → TTY works) — e.g. declining the
  password on a host that has *only* a password means we set no env.
- **Passphrase-armed fall-through:** arming a passphrase (no Password, or password
  declined) still sets `force`. If such a host falls through to password auth
  (encrypted key rejected, or the server offers password/keyboard-interactive after
  publickey), the password prompt gets a zero-length reply → empty send → all
  attempts consumed, **no manual fallback** for that connect. This is a deliberate
  trade-off, not a miss: arming a passphrase removes today's "key rejected → type
  your password manually" recovery. The post-fill toast should make this
  diagnosable, and a passphrase-armed host that also permits password auth is a
  candidate for a warning.
- Channel unreachable / token mismatch / unclassifiable / identity-mismatched →
  zero-length reply → helper exits non-zero (user-visible effect per path above).
- Listener timeout / tab closed before auth → no secret served, no hang.

## Security

- **Threat model:** the channel ACL/dir-perms + per-connect token defend against
  **other local users** and network/loopback attackers. They do **not** defend
  against **same-user malware** (which can read `ssh`'s environment block on
  Windows via `NtQueryInformationProcess`→PEB, or `/proc/<pid>/environ` on Linux,
  steal the token, and reach the user-scoped channel). Accepted: such an attacker
  already has strictly stronger options against the unlocked vault.
- **Descendant inheritance:** the askpass env + token are inherited by **every
  descendant** of the connect ssh (ProxyJump's nested ssh, ProxyCommand /
  LocalCommand / RemoteCommand running ssh/scp/rsync). The token does **not** gate
  descendants. `is_proxied()` only sees the host's own block, so the **`ssh -G`
  proxy check (step 2) is the primary skip** for inherited proxies; the listener's
  per-secret **identity binding** is the load-bearing backstop — a descendant's
  differing `user@host` / key path gets nothing.
- **Prompt-injection:** a malicious/compromised server controls the
  keyboard-interactive prompt text, reaching the helper as argv[1] indistinguishably
  from a local prompt. argv[1] is **untrusted**; the password is released only to
  the anchored, instance-prefix-free `user@host's password: ` form for the resolved
  host, never to a `(user@host) …` prompt.
- **Trust boundary:** auto-fill binds the secret to the token + user-scoped channel
  + prompt identity — **not** to the remote host key. The password-confirm modal is
  a **consent/typo guard**, not a redirect/MITM defense — its config-derived
  `user@host` is identical whether or not the host key is legitimate. The real
  surface reductions against phishing are (a) preferring key + agent (passphrase is
  local/unphishable) and (b) the listener's prompt-identity match. The known-hosts
  gate (TOFU blocker) means auto-fill only ever arms for already-trusted hosts.
  Tying the secret to the verified host key is impossible via SSH_ASKPASS (out of
  scope).
- **Shoulder-surfing (nit):** while unlocked, the pre-connect indicator passively
  surfaces *which* hosts have a stored secret on the always-visible list. This is
  membership metadata, never the value, and is the same inventory one `P` keystroke
  away (the vault list never masks the host column) — a deliberate discoverability
  trade-off, only while unlocked.
- Secrets never appear on argv or in env (only the token + channel address do). The
  helper zeroizes after printing; `App` zeroizes listeners on channel close, quit,
  and lock.
- **Residual:** the served secret necessarily transits an OS pipe buffer and ssh's
  own memory, which sshm cannot zeroize — the same exposure class as today's
  clipboard paste.

## Testing

- **Classification/identity:** `(user@host) Enter passphrase for key
  '/path/id_ed25519':` with a matching **expanded** identity path → Passphrase
  (test a stored `~/.ssh/id_ed25519` matching the expanded prompt path on **both**
  OSes, a `%d/.ssh/k` token case, and an unresolvable-token → no-fill);
  `<user>@<host>'s password: ` with a matching resolved host → Password (include a
  `Host web1 / HostName 10.0.0.5` worked example showing the exact prompt and which
  value is compared, an uppercase-HostName lowercase-fold case, a `HostKeyAlias`
  case, and a no-`User` case where the resolved OS username matches). MUST return
  nothing: bare `(user@host) Password:`, `(user@host) Verification code: `,
  `(user@host) <user>@<host>'s password: ` (kbd-interactive crafted to satisfy the
  suffix), `One-time password (OATH):`, `[sudo] password for user:`, a non-English
  prompt, a passphrase prompt whose key path mismatches, a password prompt whose
  resolved host or user differs.
- **Single-shot:** same Password twice → second nothing. Passphrase: two **distinct**
  resolved paths → both served; the **same** path twice → second nothing. Passphrase
  then Password → both served.
- **TOFU gate:** a connect to a not-yet-known host MUST NOT arm the force env.
- **Proxy gate:** a `Host *  ProxyJump bastion` + `Host web1` fixture (where
  `web1.is_proxied()==false`) MUST NOT arm (caught via `ssh -G`).
- **Stdout framing:** leading/trailing spaces delivered byte-for-byte; non-ASCII
  (`pä55-🔐-é`) exact UTF-8; embedded `\n` rejected/falls back; (optional) >1023
  bytes falls back.
- **Channel:** constant-time token verify (mismatch → no reply, and a wrong-token
  connection is closed without blocking subsequent legit connections); a second
  same-kind connection is **accepted** and returns zero-length (channel stays
  serviceable until teardown); a helper connecting before accepting / to a
  torn-down channel gets nothing and exits non-zero; `ConnectSecrets` zeroize.
- **Path normalization:** a `\\?\`-verbatim `SSH_ASKPASS` (copy the exe to a
  >260-char path) is normalized and still launches.
- **Scoping:** env vars set only on the connect Command, NOT in sshm's own
  environment; no env when proxied/unknown-host.
- **CLI:** with `SSHM_ASKPASS_CHANNEL` set, an arbitrary `argv[1]` (incl. one
  starting with `-`, e.g. `-x`) routes to the helper without hitting `other =>`
  exit(2) and without init'ing ratatui/loading config; with it **absent**,
  `--config`/`--list`/help/version parse normally.
- **Indicator parity:** the indicator predicate and connect matcher agree on a
  multi-pattern host keyed on the 2nd pattern (both true), a glob alias (both
  false), and a proxied host (both false).
- **Outcome toasts:** 255 after `Declined{password}` → decline-aware toast (distinct
  from Served-rejected); 255 after `NotAttempted` → never-requested toast.
- **Cross-platform clippy gate:** `cargo clippy --all-targets -- -D warnings` on
  **Linux** (cross/CI or `--target x86_64-unknown-linux-gnu`) AND Windows before
  merge — local Windows clippy will not surface a Linux-only dead_code failure.

### Step 0 — manual spike (gating, concrete protocol)

A stub (`sshm` invoked **with the env set**, no `--askpass`) writes a **sentinel
file** recording the exact prompt text received; run it against the **System32 ssh**
that `tools()` resolves (not the MSYS `where ssh`). Success = the sentinel exists
with the expected prompt.

Ship/degrade matrix axes: `{passphrase, password} × {inline, new-tab}` **× server
method `{password, keyboard-interactive}`**, plus a `[PATH ssh]` (Git/MSYS) fallback
row (confirm it invokes the helper given a Windows-absolute, prefix-normalized
`SSH_ASKPASS`, honors force, and passes the channel/token through unmangled — for
both prompt kinds). Rules:
- Password auto-fill ships **enabled only for the dedicated `password`-method case**
  and **defaults to opt-in/OFF** until the spike confirms behavior; the
  keyboard-interactive cell is documented to burn an attempt (so cap further
  arming for that connect once a kbd-interactive decline is observed).
- new-tab env not inherited → ship inline-only, defer new-tab.
- Measure wt-launch + ssh-to-first-prompt latency (high-latency jump/PAM only — the
  host-key prompt is excluded by the known-hosts gate) to **ground**
  `NEW_TAB_ASKPASS_TIMEOUT`.
- Confirm the resolved build emits the `(user@host) ` kbd-interactive prefix
  (OpenSSH ≥8.5, bz#3224); pre-8.5 → degrade password to manual.
- Any failing cell degrades to manual entry; partial-pass is shippable. (Win32-OpenSSH
  #2115: `force` makes askpass work on Windows 11; `DISPLAY` not required.)

## Cross-platform clippy discipline

CLAUDE.md's last Windows-first bullet (the `find_wt`/`escape_wt_arg` precedent)
applies: a symbol used only from `#[cfg(windows)]` code trips `dead_code`/`unused`
on the **Linux** build, which a Windows-only local clippy never sees; CI gates `-D
warnings` on **both** OSes.

- Keep the channel/listener/token abstraction **platform-neutral**; gate only the
  platform-specific impls.
- Gate every Windows-only helper (SID/DACL builder, pipe-name formatter, windows
  constants) with `#[cfg(windows)]`; use `#[cfg(any(windows, test))]` for helpers a
  Linux unit test exercises.
- **Struct fields:** do **not** carry an always-present field read by only one OS
  arm (a stored pipe name / socket path / raw HANDLE / peer-cred fd) — that trips
  `dead_code` ("field is never read") on the other OS. Gate per-OS **fields** (not
  just reader methods), or use a cfg-gated inner struct. This is a new pattern (no
  current struct field is cfg-gated), so the gated-function precedent does not cover
  it.
- The `#[cfg(unix)]` socket arm is a real implementation (no `todo!()`).

## Affected / new files

- `os/askpass.rs` (new): channel (named pipe / unix socket + SID DACL / optional
  peer-cred), token, wire protocol, `ConnectSecrets` + wire-reply `Secret`, listener
  thread + outcome, classification + identity binding, the helper body, and an
  `ssh -G` resolver helper.
- `os/connect.rs`: `run_ssh_inline`/`connect_new_tab` gain an `env` parameter.
- `os/known_hosts.rs`: reuse `parse_known_hosts()`/`known_hosts_path()` for the TOFU
  gate (or shell `ssh-keygen -F`).
- `os/vault.rs`: fetch a host's secret(s) by alias; reject `\r`/`\n` in secrets at
  entry time.
- `main.rs`: env-presence askpass dispatch **before** the existing arg loop's
  `other =>` exit(2) arm.
- `app.rs`: `pending_connect: Option<PendingConnect>` (with `awaiting` stage),
  `askpass_listeners`, `confirmed_password_targets`, `drain_askpass`,
  `vault_secret_kinds`; `Screen::PasswordConfirm`.
- `update.rs`: connect interception inside `connect_selected` (both call sites:
  `handle_list`, `handle_action_menu`), proxied/TOFU skip, password-confirm,
  pending-intent state machine, resume-after-unlock, lock-clears-listeners.
- `event_loop.rs`: single terminal-bearing drain after `handle_key`; `drain_askpass`
  each tick; zeroize listeners in `run()` cleanup.
- `ui/`: pre-connect indicator (four-touch nothing — list/detail glyph),
  password-confirm modal (four-touch Screen), post-fill toasts.
- `Cargo.toml`: `windows-sys` (feature-gated; named-pipe server + SID DACL).

## Out of scope (possible follow-ups)

- Hop-aware multi-secret for proxied hosts (jump + target).
- New-tab auto-fill if the env-inheritance spike fails.
- ssh-agent integration for passphrases (load once, reuse) — note the round-2
  reviews observed agent is the cleanest passphrase path (no force, no TOFU/burn),
  worth revisiting if force-on-passphrase friction proves high.
- Per-host **persistent** (cross-session, on-disk) trust for password confirm.
- Fuzzy host matching (HostName/user) and ad-hoc override connects.
- `SO_PEERCRED` unix peer-uid check (dep-gated hardening).
- Binding the secret to the verified remote host key (impossible via SSH_ASKPASS).
- Migrating the vault's `restrict_acl` to a shared SID-DACL helper.
- An `L` lock/zeroize binding on `Screen::List` (panic-button for new-tab listeners).
