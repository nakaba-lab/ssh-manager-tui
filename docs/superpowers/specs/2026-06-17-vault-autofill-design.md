# Vault auto-fill — design

Date: 2026-06-17
Status: approved (brainstorm), pending implementation plan
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
with Windows OpenSSH. The supported escape hatch is `SSH_ASKPASS`: OpenSSH can
invoke an external helper to obtain a passphrase/password. With
`SSH_ASKPASS_REQUIRE=force` (OpenSSH 8.4+; this machine has 10.3p1) the helper is
used even when a TTY is present.

## Decisions (from brainstorming)

| Question | Decision |
|----------|----------|
| Trigger | **Automatic on connect** — if the vault is unlocked and a secret matches the host, supply it. No extra keypress. |
| Connection paths | **Both** inline (`Enter`) and new Windows Terminal tab (`t`, `wt.exe`). |
| Locked vault on connect | **Prompt for the master password, then auto-fill.** Cancel → fall back to a normal connection. |
| Mechanism | **Method A — a unified `SSH_ASKPASS` helper** (sshm invoked as `sshm --askpass`) serving both passphrase and password prompts. |

Rejected: ssh-agent+askpass hybrid (more moving parts; can be added later if
repeated passphrase connects warrant it); PTY/ConPTY scraping (fragile on
Windows).

## Architecture

All new logic lives in the **`os/` layer** (zero ratatui dependency), beside the
existing connect/vault code. The UI only triggers connects and renders the
unlock modal.

### Connect-time flow

1. On connect (inline or new-tab) sshm looks up vault entries whose `host`
   matches the connection **alias** (exact match).
2. Match found, vault **locked** → stash a *pending connect intent* (target +
   mode) on `App`, open the existing `VaultUnlock` modal. On success → run the
   pending intent (with auto-fill). On cancel → discard intent, connect normally.
3. Match found, vault **unlocked** → open a one-shot channel carrying the
   matching secret(s), spawn `ssh` with:
   - `SSH_ASKPASS = <path to the running sshm executable>`
   - `SSH_ASKPASS_REQUIRE = force`
   - `SSHM_ASKPASS_CHANNEL = <pipe/socket address>`
   - `SSHM_ASKPASS_TOKEN = <256-bit random, hex>`
4. When `ssh` needs a secret it execs `sshm --askpass "<prompt text>"`.
5. The helper connects to the channel, presents the token, receives the secret
   bundle, classifies the prompt, prints the chosen secret on stdout (one line),
   zeroizes, exits 0.
6. After `ssh` exits (inline) or the listener times out (new-tab), the channel is
   torn down and the in-memory secret is zeroized.

### The askpass helper — `sshm --askpass <prompt>`

A new CLI branch that does **not** start the TUI. It:

- Reads `SSHM_ASKPASS_CHANNEL` / `SSHM_ASKPASS_TOKEN` from the environment.
- Connects to the channel, sends the token; on mismatch, exits non-zero.
- Receives the secret bundle (the host's Password and/or Passphrase).
- Classifies the prompt text (passed as the first CLI argument by OpenSSH),
  case-insensitively and conservatively:
  - contains `passphrase` → return the **Passphrase** secret
  - contains `password` → return the **Password** secret
  - otherwise → print nothing, exit non-zero (ssh falls back to its own prompt)
- Prints the secret to stdout, zeroizes its copy, exits 0.

### The secure channel

A **named pipe** (Windows-first):

- Windows: `\\.\pipe\sshm-askpass-<random>` with an ACL restricting access to the
  current user.
- Unix: a unix-domain socket inside a `0700` directory.

Chosen over loopback TCP because the OS enforces access control and there is no
port to brute-force. The secret is never placed on argv or in an environment
variable — only the channel address and the token are, and the token alone
yields nothing without reaching the (user-scoped) channel.

**Token auth:** a fresh 256-bit random token per connection, passed via
`SSHM_ASKPASS_TOKEN`. The helper must present it before any secret is sent;
mismatch → immediate disconnect. The channel is short-lived and serves at most a
small number of requests (a passphrase may be followed by a password) before
being torn down on a timeout.

### Lifecycle — inline vs new-tab

**Inline (`Enter`):** the TUI thread blocks in `run_ssh_inline` waiting on `ssh`,
so the channel **listener runs on a separate thread**, spawned before the ssh
run and holding the secret. The helper connects during the ssh window. After
`ssh` exits, the listener is stopped and the secret zeroized, then the TUI
resumes.

```
spawn listener thread (holds secret)
  -> suspend TUI -> run_ssh_inline (sync wait)
       ^ helper connects & receives here
  -> ssh exits -> stop listener, zeroize -> resume TUI
```

**New tab (`t`, `wt.exe`, fire-and-forget):** the TUI keeps running, so each
connect spawns a **background listener thread with a timeout** (e.g. 60s). The
tab's `ssh` may reach the prompt seconds later; the helper connects then. After
serving or timing out, the listener tears down and zeroizes.

- Env vars are set on the `wt.exe` `Command`; the tab's `ssh` (wt's child)
  inherits them, and the helper (`ssh`'s child) inherits in turn. **This env
  inheritance through `wt.exe` must be verified by a spike (see below).**
- Each connect uses a unique pipe name + token, so concurrent tabs do not cross.

### Host ↔ secret matching

- Match `VaultEntry.host == <connection alias>` (exact).
- A host may have a Password and/or a Passphrase entry; both are bundled and the
  helper picks by prompt text.
- No match → no auto-fill; connect normally (not an error).

## Error handling (always degrades gracefully)

- Channel unreachable, token mismatch, or unclassifiable prompt → helper exits
  non-zero → `ssh` falls back to its own terminal prompt (today's behavior).
- Windows OpenSSH not honoring askpass for the password prompt → same fallback
  (the spike de-risks this before implementation).
- Listener timeout (e.g. tab closed before auth) → no secret served, no hang.

## Security

- One-time 256-bit token per connection; user-scoped named pipe / `0700` socket;
  short-lived, single-/few-use channel.
- Secrets never appear on argv or in environment variables.
- Helper zeroizes its copy after printing; the TUI zeroizes on channel close,
  reusing the vault's `Secret` / `Zeroizing` machinery.
- `SSH_ASKPASS` points at sshm itself, but the real gate is the channel env +
  token: an `ssh` without them gets nothing.
- Auto-filling a *login password* is inherently weaker than key+passphrase+agent;
  this is accepted, and key-based auth remains the recommended posture.

## Testing

- Unit: prompt classification (passphrase / password / unknown), alias matching,
  token verification, secret-bundle serialize + zeroize.
- Subprocess test: run `sshm --askpass "Enter passphrase for key ..."` against a
  mock channel and assert the correct secret is returned (and that a bad token
  yields nothing).
- **Step 0 — manual spike (gating):** confirm Windows OpenSSH 10.3 invokes the
  askpass helper for **both** a passphrase prompt and a password prompt, on
  **both** the inline and the new-tab (`wt.exe`) paths. Implementation proceeds
  only once this holds; if a path fails, that path degrades to manual entry.

## Affected / new files (approximate)

- `os/askpass.rs` (new): channel (named pipe / unix socket), token, listener
  thread, and the helper body.
- `os/connect.rs`: connect functions extended to set the env and start the
  channel.
- `main.rs`: `--askpass` subcommand branch.
- `app.rs` / `update.rs`: pending-connect intent + resume-after-unlock.
- `os/vault.rs`: helper to fetch a host's secret(s) by alias.

## Out of scope (possible follow-ups)

- ssh-agent integration for passphrases (load once, reuse across the session).
- Migrating password-auth hosts toward key-based auth.
- Fuzzy host matching (hostname/user) beyond exact alias.
