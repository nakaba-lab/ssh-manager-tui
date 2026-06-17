# Vault auto-fill — design

Date: 2026-06-17 (rev 4, 2026-06-18)
Status: approved (brainstorm); **revised three times after multi-angle adversarial
reviews** (round 1: 23 findings; round 2: 29 findings incl. a TOFU blocker; round 3:
27 findings — 0 blockers, mostly precision of the rev-3 fixes: `ssh -G` side
effects/bounding, TOFU-gate exactness, password opt-in); pending implementation plan
Branch context: builds on the encrypted password vault (`os/vault.rs`, PR #13)

## Problem

Stored secrets (per-host login passwords and key passphrases) can only be used
today by copying them to the clipboard (`y`/`c`) and pasting into OpenSSH's
prompt. That is tedious and leaves the plaintext in the clipboard. We want sshm
to supply the secret to `ssh` directly when connecting, for **both** key
passphrases and login passwords.

> Caveat surfaced up front: **login-password** auto-fill works only against servers
> that negotiate OpenSSH's legacy `password` method; the common stock-Linux default
> (Debian/Ubuntu/RHEL PAM via `keyboard-interactive`, and all MFA/OTP) is **not**
> covered — there it is withheld and (under `force`) an attempt is burned with no
> manual fallback. It is therefore **off by default**. **Passphrase** auto-fill (the
> local, unphishable case) is the frictionless win; key + passphrase (or ssh-agent)
> remains the supported path for the common server case.

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
(possibly several), `port`, `hostkeyalias`, `proxyjump`/`proxycommand`,
`userknownhostsfile`/`globalknownhostsfile` from its output. This single move:

- gives the real `user@hostname` to compare against the password prompt and to
  show in the confirm modal,
- enumerates the effective IdentityFile entries (incl. `Host *`/`Match`-inherited
  ones) to bind passphrase prompts — note `ssh -G` returns these with `~`/`%h`/
  `%d`/`%u`… **UNexpanded** (verified: 9.5p2 leaves `~/.ssh/id_rsa` and `%`-tokens
  literal); the listener expands them itself per the algorithm in
  "Classification + identity binding". `ssh -G` resolves HostName/User/proxy but
  does **not** expand IdentityFile tokens.
- surfaces **inherited** proxy directives that `HostView::is_proxied()` cannot see.

**`ssh -G` is run exactly once per connect dispatch** and its parsed result is
threaded through the proxy check, the listener identity binding, and the
unlock→confirm→drain re-derive (cached for the connect's duration), so a
non-idempotent `Match exec` predicate is never run 2–3 times.

**`ssh -G` executes `Match exec`** (see Security) — to avoid triggering it, first
syntactically scan the already-parsed `HostView` for a `Match exec` directive and,
if present, **degrade to manual entry without invoking `ssh -G` at all**.
(Inherited/Included `Match exec` from a global block can still slip through; for
those the bounded resolver below contains the blast radius.)

**Bounded, off-thread resolution.** `ssh -G` (and the `ssh-keygen -F` TOFU probe)
runs on a short-lived **worker thread** (mirroring `os/liveness.rs`: worker + mpsc
+ per-tick drain), never on the UI thread, spawned with `stdin = Stdio::null()`.
`std::process` has no wait-with-timeout, so a named `SSH_G_RESOLVE_TIMEOUT`
(~300–500 ms, sized at the Step-0 spike) is enforced via a `try_wait()` poll loop
(or a `wait_timeout` dep) + `kill()` on expiry. The connect dispatch is therefore
**async**: arm-or-degrade is decided when the resolver result arrives (at the
drain), not inline, so the TUI keeps drawing; show a brief "resolving connection…"
state. On failure / timeout / dynamic identity (CanonicalizeHostname, `%C`, any
unhandled token), **degrade to manual entry** (no askpass env).

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

**Fix:** before arming (setting the force env), require the resolved host to
already have a **plain, non-marker** entry in `known_hosts`. If absent → connect
**normally with no askpass env** so the user accepts the key at the terminal/tab;
offer auto-fill on a later connect once trusted. There is no force-compatible way
to both auto-fill and let the human accept an unknown key.

The probe must be precise — `ssh-keygen -F` and `parse_known_hosts()` are **not**
interchangeable:

- **Probe:** shell out to `tools().ssh_keygen -F <lookupkey> -f <file>`
  (System32-preferred), iterating over **every** path in the `userknownhostsfile`
  **and** `globalknownhostsfile` lists from the same `ssh -G` dump (not just
  `~/.ssh/known_hosts`). `ssh-keygen -F` correctly matches **hashed** entries
  (HMAC), which `parse_known_hosts()` cannot. The in-repo
  `parse_known_hosts()`/`known_hosts_path()` is at most a plain-only, single-file
  fast path that **fails closed** (hashed file / non-22 entry / multi-file config →
  "use `ssh-keygen -F`"), never a substitute — `HashKnownHosts yes` is the
  Debian/Ubuntu default and a parse-only gate would defeat the headline workflow.
- **Lookup key (built from the same dump, never the raw alias):** `HostKeyAlias`
  verbatim if set; else the resolved `hostname` if `port == 22`; else
  `[<hostname>]:<port>`.
- **Marker exclusion:** the gate passes **only** on a plain key line with no
  marker, wildcard, or negation. A `@revoked` entry → never arm (treat as
  not-trusted); a `@cert-authority` or wildcard/negation match → **not** a
  per-host pin, so degrade to manual (do not arm on it). Both `ssh-keygen -F` and a
  naive parse would otherwise admit exactly the hosts the gate must exclude — so
  after `ssh-keygen -F` reports a hit, confirm the matched line is plain
  (re-inspect the returned line / file) before arming.
- **Direction:** a gate miss is a **false negative** (silent non-arming, fails
  safe).

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
2. Resolve via the bounded off-thread `ssh -G <alias>` (single invocation, cached;
   skipped entirely if the literal `HostView` contains `Match exec`). If it reports
   any `proxyjump`/`proxycommand` (own block OR inherited), or a
   dynamic/unresolvable identity, or it times out → connect normally, **no askpass
   env**.
3. Check the resolved lookup key (HostKeyAlias / hostname / `[host]:port`) has a
   plain non-marker `known_hosts` entry (per the TOFU gate above). If not → connect
   normally, **no askpass env**, and emit the gate-skip toast (see Discoverability).
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
the effective IdentityFile entries (as `ssh -G` reports them — still token-bearing;
the listener expands them itself, see below).

- **Passphrase** (local, unphishable): release only if the prompt matches the
  literal key form `Enter passphrase for key '<path>': ` **and** `<path>` matches
  an expected IdentityFile. Match algorithm: expand each `ssh -G`-reported
  IdentityFile (raw — `ssh -G` does not expand it) the way OpenSSH does, sourcing
  each token from the **same dump**: tilde-expand (`~`, `~user`), then percent-expand
  the full client set — `%d` home, `%h` resolved hostname, `%i` uid, `%l`/`%L`
  localhost, `%u` **local** OS user, `%r` **remote** user (`ssh -G user`; do not
  conflate with `%u`), `%k` HostKeyAlias (else host arg), `%n` the alias/host arg,
  `%p` `ssh -G port`, `%j` `ssh -G proxyjump` host — make absolute, normalize
  separators (`/`≡`\` on Windows), compare case-insensitively on Windows /
  case-sensitively on unix, and account for the prompt's `%.100s` truncation
  (compare the first 100 bytes). **Fail-safe:** any percent token outside this
  handled set, or any token whose source value the dump did not provide, or `%C` /
  dynamic HostName / unknown `~user` → do **not** auto-fill the passphrase.
- **Password** (server-facing, phishable): release only if the prompt **equals
  exactly** `<user>@<host>'s password: ` — an **anchored full-string** match with
  **no** leading `(user@host) ` instance prefix — where:
  - `<host>` = `HostKeyAlias` if set (compared **verbatim**), else the `ssh -G
    hostname` value compared **verbatim** — `ssh -G` has already applied OpenSSH's
    own ASCII-only `lowercase()` (`misc.c tolower((u_char)*s)`), the exact transform
    `sshconnect.c` applies to the prompt host, so no client-side fold is needed. The
    listener MUST NOT apply Rust `str::to_lowercase()` (full-Unicode: `İ`→`i̇`,
    `CAFÉ`→`café` diverge byte-for-byte and silently fail the anchor); if any fold
    is applied for robustness it MUST be `eq_ignore_ascii_case` / `to_ascii_lowercase`.
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
  **different** expected path is served. This helps the **shared-passphrase**
  multi-key host (key A rejected → fall back to key B with the *same* stored
  passphrase) instead of feeding key B an empty (force-skipping) passphrase. It
  does **not** help distinct-passphrase keys: the vault stores at most one
  passphrase per host, so a host whose effective config lists multiple IdentityFiles
  with *different* passphrases auto-fills only the matching key; others get the
  stored passphrase, fail to decrypt locally (benign skip, no server traffic), and
  fall through to manual. (See Out of scope; ssh-agent is the cleaner long-term
  answer.)

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
`Win32_Storage_FileSystem` (`ReadFile`/`WriteFile` if raw I/O is chosen),
`Win32_Foundation` (`HANDLE`/`CloseHandle`/`LocalFree`). It provides the **entire**
Windows named-pipe server — `CreateNamedPipeW`, the `SECURITY_ATTRIBUTES`/SDDL DACL,
`ConnectNamedPipe`, `DisconnectNamedPipe`/`CloseHandle` — none of which exist in Rust
std; it is load-bearing IPC, not optional hardening.

This is the project's **first unsafe FFI** (warranting tighter review than a
config-layer change), so the FFI is confined to two functions — `create_secured_pipe()`
(`CreateNamedPipeW` + SDDL DACL, returning the **owned raw `HANDLE`**) and
`connect_next()` (`ConnectNamedPipe`, then hand the connection to safe Rust) — with
the token/prompt/secret byte loop entirely in safe Rust.

**HANDLE ownership (resolves the reuse-loop hazard):** the single owned resource is
the raw `HANDLE`, created once and kept alive for the listener's whole lifetime
(preserving `FILE_FLAG_FIRST_PIPE_INSTANCE` squat-protection), closed/dropped **only**
at teardown. Do **not** wrap it in a per-connection `FromRawHandle` `std::fs::File`
that would `CloseHandle` the persistent instance on `Drop` between connections. Pick
**one** I/O form and document it: (A) raw `ReadFile`/`WriteFile` on the persistent
`HANDLE` (needs `Win32_Storage_FileSystem`); or (B) **one** long-lived `File` wrapped
once before the loop, with `Disconnect`/`ConnectNamedPipe` called via
`as_raw_handle()` — never re-wrap per connection. Only one owner ever calls
`CloseHandle` (if a `File` owns it, `Drop` does it — do not also call `CloseHandle`).

The vault file keeps its existing best-effort **icacls** `restrict_acl` (icacls
cannot secure a `\\.\pipe\` kernel object that has no filesystem path); this
filesystem-ACL vs kernel-object-ACL asymmetry is intentional (migrating
`restrict_acl` to a shared SID-DACL helper is an out-of-scope follow-up).

### Stdout framing (helper → ssh)

The helper writes the secret's **raw UTF-8 bytes followed by exactly one `\n`**,
nothing else — no trim, normalize, or re-encode. OpenSSH reads ≤1023 bytes and
truncates at the first `\r`/`\n`. Therefore:

- A secret containing `\r`/`\n` cannot survive → **reject at vault-entry time**
  (preferred) and/or the helper refuses to serve it rather than truncating.
- A secret >1023 bytes → documented limit; helper refuses/falls back.

### `SSH_ASKPASS` path normalization

Set `SSH_ASKPASS` to `std::env::current_exe()`, then apply an **unconditional
verbatim-prefix strip** to whatever it returns — `Path::components()` mapping
`Prefix::VerbatimDisk` → `DiskPrefix` (`\\?\C:\…` → `C:\…`) and `Prefix::VerbatimUNC`
→ `\\server\share\…` — because Win32-OpenSSH's `CreateProcessW` rejects a
`\\?\`-prefixed argv0, and `current_exe()` emits that prefix when installed at a path
> 260 chars. Note `current_exe()` *already* calls `GetModuleFileNameW` internally, so
`GetModuleFileNameW` is at best an alternative **source** of the same string, never a
prefix-dodge — if kept as a fallback it must be fed through the same stripper and add
`Win32_System_LibraryLoader` to the windows-sys features; otherwise drop it
(`current_exe()` + strip suffices). Spaces/parentheses in the path are fine
(argv-style spawn), so no quoting is needed there.

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
high-latency PAM/keyboard-interactive round-trips and high-RTT direct connections
**only** — proxied/jump hosts are skipped at step 2, and the host-key prompt is
excluded by the known-hosts gate. **Env inheritance is the open risk:**
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
PasswordConfirm, Ready }`. `PasswordConfirm → Ready` **spans two keypresses**:
keypress A (the connect) opens the modal and re-defers; keypress B (the modal
Enter/Esc) sets `awaiting=Ready`, and **B's own** post-`handle_key` drain runs the
connect (it is not A's drain).

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
- **`kinds` source:** the drain computes `kinds` by calling
  `vault_secret_kinds(host)` on the re-derived HostView (the listener does not exist
  yet — it is started only after confirm, step 6). `MatchedKinds` is e.g. `struct
  MatchedKinds { password: bool, passphrase: bool }`, so the decline path can
  distinguish "both armed" from "password only".
- **Confirm modal contract** — a `Screen::PasswordConfirm { alias, mode, kinds }`
  variant carrying **no secret** (only alias + mode + which kinds are armed). Open
  it by directly setting `app.screen` from the drain. Keybindings: Enter = confirm
  (add to `confirmed_password_targets`, set `awaiting=Ready`, return to List; the
  post-`handle_key` drain of **this (the confirm) keypress** then runs the connect);
  Esc/`n` = decline (drop the Password from the armed set, keep Passphrase, set
  `awaiting=Ready`, return to List). Adding this Screen is the standard **four-touch**
  contract (mirroring `VaultUnlock`): (1) the `Screen` enum (app.rs), (2) a dispatch
  arm in `handle_key` (update.rs), (3) a draw/overlay arm in `ui/mod.rs` `draw()`,
  (4) a `base_screen()` arm in `ui/mod.rs` returning
  `app.prev_screen.unwrap_or(Screen::List)` — deterministically `List` here because
  connect dispatch is only reachable from `Screen::List` (so `prev_screen` is `None`
  on the direct path and `Some(List)` on the deferred path, both resolving to
  `List`); the deferred `submit_vault_unlock` branch also sets `prev_screen=None`
  for state hygiene.

### Session-scoped password-confirm suppression

`App::confirmed_password_targets: HashSet<String>` keyed on the **resolved**
`<user@host>` (not the bare alias). Populated on confirm; the modal is skipped when
the resolved target is already present. **Cleared on lock (alongside zeroizing
listeners), on quit, and in `rebuild_hosts()`** (which already runs on every config
save and clears the liveness maps), so a mid-session HostName/User edit re-requires
confirmation. Suppression and plaintext are forgotten at the same lock boundary, so
it adds no disk/persistence exposure. Per-host **persistent** (cross-session) trust
stays out of scope.

The key is a config-derived string, **not** the real server. Two genuinely
different servers can map to one key within an unlocked session with no config edit
(so `rebuild_hosts` never clears it): most notably when distinct Host blocks share a
`HostKeyAlias` (which by design also makes OpenSSH verify both against the same pin,
giving no backstop), and less commonly when a DNS name / literal IP is rebound to a
different machine that presents the same host key. Per OpenSSH's own host-key
verification, a rebind to a machine with a *different* key aborts before the password
is sent, so the carry-over is bounded by shared-host-key reuse. Acceptable under the
existing threat model (no host-key binding): the per-resolved-target confirm is a
UX/typo + consent guard scoped to a config string, not per-server isolation — and
only config-string edits, not real-server identity changes, re-prompt.

This `rebuild_hosts()` clear is **deliberately blanket, not surgical**:
`rebuild_hosts()` runs on **every** config save — host add/edit (`save_host`),
delete (`DeleteHost`), and set-identity (`s` in the key manager) — so *any* one of
them re-arms the confirm modal for **all** previously-confirmed targets, including
ones whose `<user@host>` did not change (e.g. deleting host B re-confirms unrelated
host A). This is intentional over-clearing, accepted for two reasons: (1) it is
fail-safe — the only cost is one extra benign confirm modal on the next connect,
never a withheld or wrongly-released secret, and the cache is session-only/never
persisted; (2) the surgical alternative (clear only targets whose resolved
`<user@host>` actually changed) would have to re-resolve every confirmed target via
`ssh -G` on every save, which is exactly the per-resolve cost M-02 rules out. So the
coarse clear is the right trade. **Layering note:** this couples a vault/confirm
concern into `rebuild_hosts()`, but that is consistent with the existing design —
`rebuild_hosts()` is a method on `App` (the all-state orchestration layer that
already owns `vault`, the liveness maps, etc.), **not** on the `config/` domain
module (which stays vault-unaware). The method already clears the `os/liveness`
maps for the same "config changed → dependent session state is now stale" reason;
clearing the confirm set is the same pattern, not a new cross-layer leak.

### Password auto-fill enablement (the opt-in)

Password auto-fill is **off by default** (see Problem caveat: it only fires for the
`password` method and can burn an attempt under `force`). v1 model:

- **Storage:** an in-memory `App::password_autofill_enabled: bool`, default `false`,
  **not persisted** across restart (persistence — e.g. a key in a sshm settings
  file or the vault header — is a follow-up). State plainly that, with no UI wired,
  the feature otherwise "ships dark".
- **Toggle:** a keybinding (e.g. on `Screen::Vault`, surfaced in its footer/help)
  flips it for the session, mirroring the existing per-screen key-handling pattern.
- **Discoverability while OFF:** because the indicator predicate masks the Password
  kind when disabled (see Indicator), a password-only host shows no glyph. To avoid
  "my stored password silently does nothing", emit a **one-time** toast on the first
  connect to a password-candidate host while OFF: "a login password is stored for
  <alias>; password auto-fill is off (server-facing, can burn an auth attempt) —
  enable with <key>."
- The step-5 gate references this field; passphrase auto-fill is unaffected by it.

### connect.rs signatures

`run_ssh_inline(args, env: &[(OsString, OsString)])` and
`connect_new_tab(alias, args, env)` apply `Command::envs(env)` before spawn (the two
existing call sites pass `&[]`). **`connect_new_tab` has two cfg arms** (windows /
non-windows) and both gain `env`; the non-windows stub binds it as `_env` to satisfy
the Linux unused-parameter clippy gate. `connect.rs` does **not** start the channel
or own the listener — `os/askpass` generates `(channel_addr, token)`, starts the
listener, and returns the env bundle to the caller, which passes it in. `SSH_ASKPASS`
is the normalized absolute `current_exe()` path, resolved in `os/`.

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
  it would actually auto-fill — the **same candidacy predicate** as connect
  dispatch: a vault entry matches any pattern (case-sensitive, not a glob/negation)
  and the host is not proxied. Implement as a single shared
  `App::vault_secret_kinds(host) -> Option<MatchedKinds>` (a pure host↔entry match,
  **not** the listener's release logic) that both the indicator and connect call.
  **The password-enabled setting is folded into this predicate:** while password
  auto-fill is OFF, `vault_secret_kinds` masks out the Password kind, so a
  password-only host shows **no** glyph and a both-kinds host downgrades to the
  passphrase-only glyph (passphrase has no such gate) — keeping the "iff it would
  actually auto-fill" invariant true and not over-promising the phishable kind.
  Read `app.vault` directly each render (no cached field) so it stays correct across
  lock/unlock/`rebuild_hosts`. Distinct glyphs for password-only / passphrase-only /
  both. Not shown while locked. Glyph color from `ui/theme.rs`.
- **Pending (untrusted-host) glyph:** when a host is a candidacy match but the
  resolved HostName/HostKeyAlias is not yet in `known_hosts`, render a **muted /
  theme-dimmed** glyph ("stored, will auto-fill once trusted") rather than the
  active glyph. The indicator additionally consults the same `known_hosts` probe the
  gate uses for the muted-vs-active distinction **only** — do **not** move the
  known_hosts check into the shared candidacy predicate (that would desync the
  indicator from connect dispatch's candidacy).
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
- **Pre-connect gate-skip toast:** when a host passes candidacy (step 1) and the
  step-2 proxy check but is skipped **solely** by the step-3 TOFU gate
  (not-yet-known host), connect normally with no askpass env and emit a non-error
  toast: "host key not yet trusted for <alias> — accept it at the prompt, then
  reconnect to auto-fill the stored secret." Distinguish this from the **permanent**
  step-2 proxy skip so the two no-op paths are not conflated. (Pre-connect toast,
  not a listener-outcome toast.) This makes the headline "add host → store password →
  connect" a clear **second-connect** feature: connect #1 establishes trust at the
  terminal, auto-fill arms from #2 onward — and the steady state is then one confirm
  modal per resolved `<user@host>` per session, zero typing thereafter.
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
- **`ssh -G` executes `Match exec`:** resolving the effective config spawns the
  user's shell and **runs every `Match exec "<cmd>"` predicate** that could gate the
  host (empirically verified on System32 9.5p2 and MSYS 10.3p1; `BatchMode=yes` does
  **not** suppress it; `ProxyCommand`/`LocalCommand`/`RemoteCommand` are **not** run
  by `-G`). This moves Match-exec execution from "inside the user's explicit
  connect" to "sshm's pre-connect resolution". Mitigations: (1) a literal `Match
  exec` in the host's own `HostView` → skip `ssh -G` entirely and degrade to manual;
  (2) for inherited/Included `Match exec`, the bounded off-thread resolver
  (`stdin=null`, `SSH_G_RESOLVE_TIMEOUT`, `kill()` on expiry) caps the blast radius;
  (3) the once-per-connect contract means it runs at most once, not 2–3×. The
  line-48 "degrade on `Match exec`" rule prevents the *auto-fill*, not the *exec*.
- **TOCTOU (resolution ≠ connect):** `ssh -G` (vetting), the known_hosts gate, and
  the real `ssh` connect are three independent invocations that each re-parse the
  config; the secret binds to the `user@host` **string** from the first resolution,
  not a live peer/host key. Config/Include/synced-`~/.ssh` mutation in the small
  arm-before-spawn window could make the connect resolve to a different same-named
  target. A pure DNS rebind does **not** defeat this — SSH verifies the host key
  before the password prompt and the gate only arms already-pinned hosts, so a
  rebind to a peer lacking the real key never reaches the password stage. Inherent
  to SSH_ASKPASS; agent-based passphrase auth is the only path without this window.
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
- **TOFU gate:** a not-yet-known host MUST NOT arm. Edge cases that MUST arm: a
  **hashed** `known_hosts` fixture where the host is known; a non-22-port host keyed
  as `[host]:port`; a host trusted only in a custom `UserKnownHostsFile` /
  `known_hosts2`; `alias != HostName`. MUST NOT arm: a `@revoked` entry; a host
  matched only by a `@cert-authority` or wildcard/negation line.
- **Match exec:** a host with a literal `Match exec` in its own block degrades to
  manual **without** invoking `ssh -G` (assert the exec command is not run); a
  `Match exec "sleep N"` (inherited) fixture degrades within `SSH_G_RESOLVE_TIMEOUT`
  and does **not** freeze the loop; a Match-exec counter increments **exactly once**
  per connect dispatch (incl. across the unlock→confirm→drain hops).
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
- **Path normalization:** synthetic `\\?\C:\…` and `\\?\UNC\server\share\…` inputs
  are de-verbatimed to `C:\…` and `\\server\share\…`; a real `\\?\`-verbatim
  `SSH_ASKPASS` (copy the exe to a >260-char path) still launches.
- **windows-sys symbols:** a tiny `#[cfg(windows)]` harness references every named
  windows-sys symbol so a missing feature is a **compile-time** failure, not a
  runtime one.
- **Pipe reuse (Windows integration):** a `#[cfg(windows)]` test round-trips two
  sequential helpers (token + prompt + reply) over **one** persistent instance and
  asserts the second still gets a reply — catching the `File`-drop-closes-handle
  hazard that unit tests cannot.
- **Token-set expansion:** an `IdentityFile ~/.ssh/id_%k` host (HostKeyAlias set) →
  sshm computes the same expanded path and fills; an `IdentityFile ~/.ssh/id_%C`
  host → fail-safe no-fill.
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
- Measure wt-launch + ssh-to-first-prompt latency (high-latency PAM/keyboard-
  interactive round-trips and high-RTT direct connections only — proxied/jump hosts
  are skipped at step 2 and the host-key prompt is excluded by the known-hosts gate)
  to **ground** `NEW_TAB_ASKPASS_TIMEOUT`. Also measure the **pre-connect `ssh -G`
  spawn** (it is on the connect path) to size `SSH_G_RESOLVE_TIMEOUT`.
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

- `os/askpass.rs` (new): channel (named pipe + SID DACL / unix socket + optional
  peer-cred), token, wire protocol, `ConnectSecrets` + wire-reply `Secret`, listener
  thread + outcome, classification + identity binding (incl. IdentityFile token
  expansion), the helper body, the bounded off-thread `ssh -G` resolver
  (`SSH_G_RESOLVE_TIMEOUT`, `stdin=null`, `kill()` on expiry), and the
  `SSH_ASKPASS` verbatim-prefix stripper.
- `os/connect.rs`: `run_ssh_inline`/`connect_new_tab` (both cfg arms) gain an `env`
  parameter.
- `os/known_hosts.rs`: TOFU gate — prefer `tools().ssh_keygen -F <lookupkey> -f
  <file>` across all `ssh -G`-reported known_hosts files, with marker/wildcard
  exclusion; `parse_known_hosts()` only as a plain-only fail-closed fast path.
- `os/vault.rs`: fetch a host's secret(s) by alias; reject `\r`/`\n` in secrets at
  entry time.
- `main.rs`: env-presence askpass dispatch **before** the existing arg loop's
  `other =>` exit(2) arm.
- `app.rs`: `pending_connect: Option<PendingConnect>` (with `awaiting` stage),
  `askpass_listeners`, `confirmed_password_targets`, `password_autofill_enabled`,
  `drain_askpass`, `vault_secret_kinds`; `Screen::PasswordConfirm`.
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
- **Persisting** the password-autofill-enabled toggle across restarts (v1 is
  in-memory, default OFF).
- **Per-key distinct passphrases** — sshm stores at most one passphrase per host, so
  a host whose effective config lists multiple IdentityFiles with *different*
  passphrases only auto-fills the matching key; others fall through to manual.
  ssh-agent is the cleaner answer for passphrase-heavy / multi-key users.
- Fuzzy host matching (HostName/user) and ad-hoc override connects.
- `SO_PEERCRED` unix peer-uid check (dep-gated hardening).
- Binding the secret to the verified remote host key (impossible via SSH_ASKPASS).
- Migrating the vault's `restrict_acl` to a shared SID-DACL helper.
- An `L` lock/zeroize binding on `Screen::List` (panic-button for new-tab listeners).
