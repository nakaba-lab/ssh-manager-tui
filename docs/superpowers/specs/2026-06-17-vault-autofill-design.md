# Vault auto-fill — design

Date: 2026-06-17
Status: approved (brainstorm); **revised after a 30-agent multi-angle adversarial
review** (23 confirmed findings folded in); pending implementation plan
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
> exercise the binary `tools()` actually resolves.

## Decisions (brainstorm + review)

| Question | Decision |
|----------|----------|
| Trigger | **Automatic on connect** for passphrases. Passwords are **automatic but gated by a one-time per-connect confirmation** (see below). |
| Targets | **Both** key passphrases and login passwords, hardened differently (passphrase is local/unphishable; password is server-facing/phishable). |
| Connection paths | Inline (`Enter`) always; **new-tab (`t`, `wt.exe`) contingent on the env-inheritance spike** (Step 0). If the spike fails, new-tab ships without auto-fill (inline-only) and is a follow-up. |
| Locked vault on connect | **Prompt for the master password, then auto-fill.** Cancel → connect normally, no auto-fill. |
| Mechanism | **Method A — sshm itself as the `SSH_ASKPASS` helper**, with classification + identity binding performed in the **listener** (the trusted TUI process), not the helper. |
| Proxied hosts | **No auto-fill for proxied hosts in v1** (`HostView::is_proxied()` → connect normally). Hop-aware multi-secret is a follow-up. |

Rejected: ssh-agent+askpass hybrid (more moving parts; possible follow-up);
PTY/ConPTY scraping (fragile on Windows).

## The force reality (this shaped the whole design)

`SSH_ASKPASS_REQUIRE=force` flips `allow_askpass=1` independent of `DISPLAY`, so
for the **entire lifetime** of that `ssh` process **every** secret prompt (key
passphrase, login password, password retries, PAM/keyboard-interactive factors)
goes to the helper and **never** to the terminal. Consequences we must design
around (verified against openssh-portable `readpass.c` / `sshconnect2.c`,
matching the target build):

- **No in-process manual fallback.** Once `ssh` starts under force, the human
  cannot answer any prompt at the TTY for that connection.
- **A non-zero helper exit does not re-prompt the user.** For the password path
  (`userauth_passwd`, `flags=0`), a NULL/empty helper result is sent to the
  server as an **empty password**, burning one of the (default 3)
  `NumberOfPasswordPrompts` attempts. For the passphrase path the empty result is
  detected and the key is skipped (benign, local).
- Therefore the helper **must** return the correct secret on the first call, and
  the design must avoid replaying a wrong secret (see single-shot below).

We accept this: an auto-fill miss on a password host costs an attempt and shows
"Permission denied". Key-based auth + agent remains the recommended posture.

## Architecture

All new logic lives in the **`os/` layer** (zero ratatui dependency). The UI only
triggers connects, renders the unlock + password-confirm modals, and shows
indicators/toasts.

### Connect-time flow (saved-host connect only)

1. On connect (inline or new-tab) sshm looks up vault entries whose `host`
   matches the connection alias (see "Host ↔ secret matching"). Applies to the
   saved-host path (`ConnectOverrides::default()`); ad-hoc override connects are
   out of scope for v1.
2. If the host **is proxied** (`HostView::is_proxied()`), connect normally with
   **no** askpass env (a nested ssh to the bastion would otherwise inherit the
   env+token and misroute the secret — see Security). No auto-fill in v1.
3. If a match exists and the vault is **locked** → store a `PendingConnect {
   alias, mode }` on `App`, open the existing `VaultUnlock` modal. On success →
   run the pending connect with auto-fill; on cancel/Esc → discard the intent and
   connect normally. The intent is cleared on **every** unlock exit (success,
   cancel, repeated-failure-then-Esc) so it can never leak into a later unlock
   opened via `P`.
4. If the matched secrets **include a Password** (we cannot predict at connect
   time whether the server will ask for it, so we gate up front) → show a
   one-time **password-confirm modal** displaying the exact target
   (`<user>@<host>` resolved from config) and asking the user to confirm
   auto-filling the stored password for this connect. On decline, connect with
   passphrase auto-fill still armed (if any) but the password withheld. This
   restores the human "eyeball the target" moment that `force` removes.
   **Passphrase auto-fill needs no confirmation** (local, unphishable). On the
   locked-vault path this modal follows a successful unlock (step 3).
5. sshm (in `os/askpass`) generates a per-connect token and starts the channel
   **listener** (holding only the needed secret bundle), **confirms it is
   accepting before** building the `ssh` command (TOCTOU: a helper that connects
   before the listener is ready must fail closed → manual entry), then spawns
   `ssh` with env:
   - `SSH_ASKPASS = <absolute path to the running sshm exe>` (`std::env::current_exe()`)
   - `SSH_ASKPASS_REQUIRE = force`
   - `SSHM_ASKPASS_CHANNEL = <pipe/socket address>`
   - `SSHM_ASKPASS_TOKEN = <256-bit random, hex>`
6. When `ssh` needs a secret it execs `sshm --askpass "<prompt text>"`.
7. The helper sends `[token][prompt]` to the listener; the **listener** verifies
   the token (constant-time), classifies + identity-binds + applies per-kind
   single-shot, and returns **the one matching secret or nothing**. The helper
   prints what it received (one line) and exits 0, or exits non-zero with no
   output if it got nothing.
8. Inline: after `ssh` exits, the listener is torn down and the secret zeroized
   (via a scope-guard, robust to early `?`/panic). New-tab: the listener tears
   down on its timeout.

### The askpass helper — `sshm --askpass <prompt>`

A new CLI branch (`Command::Askpass(String)`) capturing exactly the next argv
element as the prompt (OpenSSH execs `sshm <prompt>` as a single argv[1], no
shell). It must **short-circuit**: it does **not** call `resolve_path()`, load
the SSH config, build `App`, or init ratatui. `--config`/`--list` are ignored in
this mode. It only reads `SSHM_ASKPASS_CHANNEL`/`SSHM_ASKPASS_TOKEN`, connects,
relays `[token][prompt]`, prints the returned secret, zeroizes, exits.

- A missing/empty prompt, unreachable channel, token mismatch, or empty reply →
  exit non-zero with **no stdout** (OpenSSH discards output on non-zero exit).
- Classification keys solely on the prompt argument, never on `argv[0]` (which is
  always sshm).

### Classification + identity binding (in the LISTENER, not the helper)

Putting the decision in the listener keeps both secrets out of the helper process
and lets the trusted TUI process bind the secret to the resolved remote identity.
The listener knows, from the `HostView` at connect time, the expected
`<user>@<hostname>` and the expected IdentityFile path(s).

- **Passphrase** (local, unphishable): release the stored Passphrase **only** if
  the prompt matches the literal OpenSSH key form `Enter passphrase for key
  '<path>': ` **and** `<path>` matches an expected IdentityFile for the
  connecting host. If no identity path is resolvable (agent/default keys), do
  **not** auto-fill the passphrase (avoids feeding it to an unknown key/host).
- **Password** (server-facing, phishable): release the stored Password **only**
  if the prompt matches the exact suffix `<user>@<host>'s password: ` **and**
  `<host>` equals the resolved HostName (account for `HostKeyAlias`). This
  rejects every keyboard-interactive prompt (those arrive as `(user@host)
  <server-text>` and have a different shape), which is where a malicious server
  controls the text. When HostName resolves dynamically (Match exec, tokens,
  canonicalization), degrade to manual entry.
- **Everything else** — bare `Password:`, `Verification code:`, `One-time
  password (OATH):`, `Token:`, Duo, `[sudo] password for`, localized strings,
  any `(user@host) …` keyboard-interactive prompt → return nothing.

**Per-kind single-shot:** the listener serves each kind (Password / Passphrase)
**at most once** per connect. A second request for an already-served kind returns
nothing. The channel accepts N sequential helper connections over its lifetime
(OpenSSH re-execs the helper for every prompt and retries password auth up to
`NumberOfPasswordPrompts`), but the per-kind single-shot — **not** a fixed
numeric cap — prevents a wrong Password from being replayed across the 3 server
attempts and tripping fail2ban / PAM faillock / AD lockout.

### Channel + wire protocol

A **named pipe** (Windows) / **unix-domain socket** (Unix):

- Windows: `\\.\pipe\sshm-askpass-<CSPRNG-suffix>`, created with
  `FILE_FLAG_FIRST_PIPE_INSTANCE` and a `SECURITY_ATTRIBUTES` whose DACL contains
  **only the current user's SID** (the default named-pipe SD grants read to
  Everyone + anonymous). Named pipes are **not** in Rust std — add a direct
  `windows`/`windows-sys` dependency and build the SID DACL (`OpenProcessToken` +
  `GetTokenInformation(TokenUser)`, or an SDDL string). `icacls` (the vault's
  `restrict_acl` approach) **cannot** secure a `\\.\pipe\` channel — the
  descriptor must be applied at creation.
- Unix: a socket inside a `0700` directory; authenticate the connecting peer via
  `SO_PEERCRED` (do not trust the dir alone). A **real** implementation, not a
  stub, so the `#[cfg(unix)]` arm is not dead code on Windows.

Wire protocol (gives the named tests a concrete target):
1. Helper writes the token (32 raw bytes) then the prompt (`u32-LE length` +
   UTF-8 bytes).
2. Listener compares the token in **constant time**; on mismatch, close
   immediately with no reply.
3. Listener classifies + identity-binds + single-shots; on a match it writes the
   chosen secret as `u32-LE length` + raw bytes; on no-match it writes a zero
   length (helper then exits non-zero).
4. A `SecretBundle`/transfer type wraps `Secret` so `Drop` zeroizes on both ends.

The token is the **cryptographic gate**; the ACL/peer-cred is defense-in-depth.

### Stdout framing (helper → ssh)

The helper writes the secret's **raw UTF-8 bytes followed by exactly one `\n`**,
nothing else — no trim, normalize, or re-encode. OpenSSH reads ≤1023 bytes and
truncates at the first `\r`/`\n`. Therefore:

- A secret containing `\r`/`\n` cannot survive this line channel → **reject at
  vault-entry time** (preferred) and/or the helper refuses to serve it
  (exit non-zero) rather than silently truncating.
- A secret >1023 bytes → documented limit; helper refuses/falls back rather than
  truncating.

### Lifecycle — inline vs new-tab

**Inline (`Enter`):** the TUI thread blocks in `run_ssh_inline` waiting on `ssh`,
so the listener runs on a **separate thread**, spawned (and confirmed accepting)
before the ssh run. Teardown (stop + join + zeroize) runs as a **scope-guard** so
it executes regardless of `run_ssh_inline`'s Ok/Err and regardless of an early
`?` return from `restore_tui` or a panic-unwind (the crate is `panic="unwind"`).

**New tab (`t`, `wt.exe`):** fire-and-forget, so each connect spawns a
**background listener thread** with `NEW_TAB_ASKPASS_TIMEOUT = 60s` (named,
tunable; the upper bound — tear down earlier on first successful serve where
observable). The 60s balances secret residency against missing a slow prompt
(unknown-host-key acceptance, high-latency jump). **Env inheritance is the open
risk:** `wt.exe -w 0` hands the command to an **already-running** Terminal host,
so the tab's `ssh` may load a fresh environment and **not** inherit the per-connect
env (microsoft/terminal #15496; `compatibility.reloadEnvironmentVariables`). The
spike must test the `-w 0` attach case specifically; if env does not arrive,
new-tab ships **without** auto-fill for v1. Enablers to evaluate (not assume):
`--inheritEnvironment`, a fresh instance (`-w -1`), or a transient launcher shim
that sets the env inside the tab's own process before exec'ing ssh.

### App ownership of listeners (mirror liveness)

The detached new-tab listeners hold plaintext, so `App` must own them like the
liveness pool:

- `App::askpass_listeners: Vec<AskpassListener>` — each holds a `JoinHandle`, a
  stop signal, and an independent `Zeroizing` secret bundle.
- `drain_askpass()` reaps finished listeners each tick (called from
  `event_loop` alongside `drain_liveness()`).
- **Zeroize on quit AND on lock:** extend the `run()` cleanup (today
  clipboard-only) to stop+join+zeroize every listener on quit; make the `L` lock
  action ALSO stop+join+zeroize all listeners, so "lock" truly forgets all live
  plaintext, not just `app.vault`.

### Unlock-resume plumbing

`submit_vault_unlock` today unconditionally goes to `Screen::Vault` and toasts
"vault unlocked". Resume must be **conditional** on `app.pending_connect` being
`Some` (an unlock opened via `P` must never auto-connect). Because the inline
connect needs `&mut DefaultTerminal` (which the unlock handlers do not have),
`submit_vault_unlock` only **sets** `pending_connect` and returns to
`Screen::List`; a single terminal-bearing drain point in `handle_key`/`event_loop`
runs the pending connect after dispatch — keeping terminal plumbing out of the
secret-handling path. Store the **alias** (not a host index) so `rebuild_hosts`
cannot invalidate it.

### connect.rs signatures

`run_ssh_inline(args, env: &[(OsString, OsString)])` and
`connect_new_tab(alias, args, env)` apply `Command::envs(env)` before spawn (the
two existing call sites pass `&[]`). `connect.rs` does **not** start the channel
or own the listener — `os/askpass` generates `(channel_addr, token)`, starts the
listener, and returns the env bundle to the caller, which passes it in.
`SSH_ASKPASS` is the absolute `current_exe()` path, resolved in `os/`.

## Host ↔ secret matching

- Match `HostView::alias()` (the bare ssh destination) — and **any** pattern on a
  multi-pattern Host line — against `VaultEntry.host`. Case-sensitive (consistent
  with the verbatim alias passed to ssh); the stored host is already trimmed on
  save, so surrounding-whitespace mismatch cannot occur. Ignore any `user@`
  prefix on a future ad-hoc destination (match `alias()`, not `user@alias`).
- **Never** auto-fill when the matched alias contains a glob (`*`, `?`) or
  negation (`!`) — a wildcard is not a usable vault key.
- A host may have a Password and/or a Passphrase entry; both are loaded into the
  bundle and the listener releases the one matching the (verified) prompt.
- No match → connect normally; **stay silent** (no "no stored secret" nudge, to
  avoid noise on every keyless host).

## Discoverability / user-facing feedback

- **Pre-connect indicator:** when the vault is **unlocked** and an entry matches
  the selected host's alias, render a lock/key glyph in the host list and/or
  detail pane (color from `ui/theme.rs`, never hardcoded). Not shown while locked
  (the app cannot know without decrypting; acceptable).
- **Post-fill toast (inline):** the listener records an outcome — `Served{kind}` /
  `Declined{reason}` / `TimedOut` / `NotAttempted` — read after `run_ssh_inline`
  (extending the `describe_exit`→toast block) into e.g. "auto-filled password for
  web1", or a distinct "auto-fill secret rejected — check the stored Password for
  web1" when ssh exits 255 after a `Served` outcome (so wrong-secret vs
  no-secret is diagnosable rather than the generic auth-failed message).
- **New-tab outcome is best-effort/out-of-scope for v1** — sshm has no handle to
  the tab's ssh grandchild; ssh's own in-tab prompt is the user's signal. Stated
  explicitly rather than left implicit.

## Error handling (honest about force)

- With `force`, there is **no terminal fallback within a running connect** (see
  "The force reality"). A helper non-zero exit on the password path sends an empty
  string (one burned attempt); on the passphrase path the key is skipped.
- Channel unreachable / token mismatch / unclassifiable or identity-mismatched
  prompt → listener returns nothing → helper exits non-zero. This is the intended
  degradation, but its *user-visible* effect differs by path per the above.
- Listener timeout (tab closed before auth, slow prompt) → no secret served, no
  hang.
- A failing path (e.g. password on a build that ignores askpass) degrades to
  today's manual entry on the **next** attempt only outside force; inside a
  force'd connect it costs attempts — hence the spike gates which cells ship.

## Security

- **Threat model:** the channel ACL/peer-cred + per-connect token defend against
  **other local users** and network/loopback attackers. They do **not** defend
  against **same-user malware** (which can read `ssh`'s environment block on
  Windows via `NtQueryInformationProcess`→PEB, or `/proc/<pid>/environ` on Linux,
  steal the token, and reach the user-scoped channel). Accepted: a same-user
  attacker already has strictly stronger options against the unlocked vault
  (reading sshm's in-memory secrets, keylogging the master password); the channel
  adds no new exposure.
- **Descendant inheritance:** `SSH_ASKPASS`, `SSH_ASKPASS_REQUIRE` and the
  channel env+token are inherited by **every descendant** of the connect ssh
  (ProxyJump's nested ssh, ProxyCommand/LocalCommand/RemoteCommand running
  ssh/scp/rsync). The token does **not** gate descendants. This is why (a) v1
  skips proxied hosts entirely and (b) the listener binds each secret to the
  expected prompt **identity** (key path / user@host), so a descendant's
  differing prompt gets nothing.
- **Prompt-injection:** a malicious/compromised server controls the
  keyboard-interactive prompt text, which reaches the helper as argv[1]
  indistinguishably from a local prompt. argv[1] is therefore **untrusted**; the
  password is released only to the exact client-derived `user@host's password: `
  form with a matching resolved host, never to a `(user@host) …` prompt.
- **Trust boundary:** auto-fill binds the secret to the token + user-scoped
  channel + prompt identity — **not** to the remote host key. On a TOFU first
  connect or against a trusted-but-compromised host, the secret is sent
  automatically; the password-confirm modal mitigates by showing the target, but
  tying the secret to the verified host key is architecturally impossible via
  SSH_ASKPASS and is out of scope. Key + agent remains recommended.
- Secrets never appear on argv or in env (only the token + channel address do).
  The helper zeroizes after printing; `App` zeroizes listeners on channel close,
  quit, and lock, reusing the vault's `Secret`/`Zeroizing` machinery.
- **Residual:** the served secret necessarily transits an OS pipe buffer and
  ssh's own memory, which sshm cannot zeroize — the same exposure class as
  today's clipboard paste.

## Testing

- **Classification/identity unit tests:** `(user@host) Enter passphrase for key
  '/path/id_ed25519':` with a matching identity path → Passphrase; `user@host's
  password: ` with a matching host → Password. MUST return nothing: bare
  `(user@host) Password:`, `(user@host) Verification code: `, `One-time password
  (OATH):`, `[sudo] password for user:`, a non-English prompt, a passphrase prompt
  whose key path does not match the host's identity, a password prompt whose host
  differs from the alias.
- **Single-shot:** second Password request on the same channel returns nothing;
  first returns the secret. Same for Passphrase. A Passphrase request followed by
  a Password request are both served (different kinds).
- **Stdout framing:** a secret with leading/trailing spaces is delivered
  byte-for-byte; a non-ASCII secret (`pä55-🔐-é`) round-trips as exact UTF-8; a
  secret with embedded `\n` is rejected/falls back (not truncated); (optional) a
  >1023-byte secret triggers the documented fallback.
- **Channel:** token verification (constant-time, mismatch → no reply);
  `SecretBundle` serialize + zeroize.
- **Scoping:** env vars are set only on the connect Command and are NOT in sshm's
  own environment; auto-fill env is NOT applied when `host.is_proxied()`.
- **CLI:** `--askpass <prompt>` parses without hitting the `other =>` exit(2) arm
  and does not init ratatui/load config.
- **Cross-platform clippy gate:** `cargo clippy --all-targets -- -D warnings`
  passes on Linux AND Windows (see discipline below).

### Step 0 — manual spike (gating, concrete protocol)

A stub askpass (or `sshm --askpass`) writes a **sentinel file** recording the
exact prompt text it received; run it against the **System32 ssh** that `tools()`
resolves (not the MSYS `where ssh`). Success = the sentinel exists with the
expected prompt. Cover the **keyboard-interactive** method (most password-auth
servers use it), not only the `password` method — both route through
`read_passphrase` under `force`. Measure wt-launch + ssh-to-first-prompt latency
(cold host, unknown-host-key prompt) to **ground** `NEW_TAB_ASKPASS_TIMEOUT`.

Per-cell ship/degrade matrix `{passphrase, password} × {inline, new-tab}` plus a
`[PATH ssh]` (Git/MSYS) fallback row:
- e.g. password+inline fails but passphrase+inline passes → ship passphrase
  auto-fill, leave password on manual entry.
- new-tab env not inherited → ship inline-only, defer new-tab.
- Any failing cell degrades to today's manual entry; partial-pass is shippable,
  not a no-go. (Win32-OpenSSH #2115 indicates `force` makes askpass work on
  Windows 11; `DISPLAY` is not required in the env bundle.)

## Cross-platform clippy discipline

CLAUDE.md's last Windows-first bullet (the `find_wt`/`escape_wt_arg` precedent)
applies directly: a symbol used only from `#[cfg(windows)]` code trips
`dead_code`/`unused` on the **Linux** CI build, which a Windows-only local clippy
never sees, and CI gates `-D warnings` on **both** OSes.

- Keep the channel/listener/token/bundle abstraction **platform-neutral**; gate
  only the platform-specific impls.
- Gate every Windows-only helper (SID/DACL builder, pipe-name formatter, windows
  constants) with `#[cfg(windows)]`; use `#[cfg(any(windows, test))]` for any
  helper a Linux unit test exercises (as `os/connect.rs::escape_wt_arg` does).
- The `#[cfg(unix)]` socket arm is a real implementation (no `todo!()`).
- Gate the `os::askpass` imports in `os/connect.rs`/`main.rs` exactly as used.

## Affected / new files

- `os/askpass.rs` (new): channel (named pipe / unix socket + SID DACL /
  peer-cred), token, wire protocol, `SecretBundle`, listener thread + outcome,
  classification + identity binding, and the helper body (`run_helper`).
- `os/connect.rs`: `run_ssh_inline`/`connect_new_tab` gain an `env` parameter.
- `os/vault.rs`: fetch a host's secret(s) by alias; possibly reject `\r`/`\n` in
  secrets at entry time.
- `main.rs`: `Command::Askpass(String)` branch, short-circuiting before
  config/ratatui.
- `app.rs`: `pending_connect: Option<PendingConnect>`, `askpass_listeners`,
  `drain_askpass`; new `Screen`/modal for the password-confirm step.
- `update.rs`: connect dispatch (proxied skip, password-confirm, pending-intent),
  resume-after-unlock, lock-clears-listeners, terminal-bearing drain point.
- `event_loop.rs`: call `drain_askpass`; zeroize listeners in `run()` cleanup.
- `ui/`: pre-connect indicator, password-confirm modal, post-fill toasts.
- `Cargo.toml`: `windows`/`windows-sys` (named-pipe DACL).

## Out of scope (possible follow-ups)

- Hop-aware multi-secret for proxied hosts (jump + target).
- New-tab auto-fill if the env-inheritance spike fails.
- ssh-agent integration for passphrases (load once, reuse).
- Fuzzy host matching (HostName/user) and ad-hoc override connects.
- Binding the secret to the verified remote host key (impossible via SSH_ASKPASS).
