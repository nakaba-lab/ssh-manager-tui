# Vault auto-fill — Phase 1 (askpass core) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the headless-testable core of connect-time secret auto-fill — the `os/askpass.rs` module (prompt classification + identity binding, the wire protocol, the listener, the `SSH_ASKPASS` helper body, and `ssh -G`/known-hosts gating helpers) — with no UI wiring yet.

**Architecture:** sshm acts as its own `SSH_ASKPASS` helper. The trusted TUI-side **listener** holds the secrets and decides what to release by classifying the prompt OpenSSH passes and binding it to the `ssh -G`-resolved identity; the **helper** (a separate `sshm` process selected by the `SSHM_ASKPASS_CHANNEL` env var) only relays `[token][prompt]` over a user-scoped channel and prints the reply. Phase 1 delivers everything that is pure logic + the channel, unit-tested in isolation; Phases 2–4 wire it into connects and the UI.

**Tech Stack:** Rust (edition 2024), `zeroize`, `getrandom` (via the existing `random_bytes`), `windows-sys` (named pipe + SID DACL, Windows only), std `os::unix::net` (unix socket). No ratatui in `os/`.

**Spec:** `docs/superpowers/specs/2026-06-17-vault-autofill-design.md` (rev 4).

---

## Scope of this plan (Phase 1 of 4)

This plan covers **only** the headless `os/askpass.rs` core + shared plumbing. It produces a compiling, fully unit-tested module that nothing calls yet (gated to avoid dead-code warnings until Phase 3). The follow-on plans:

- **Phase 2 — resolution & gates:** the bounded off-thread `ssh -G` resolver (`SSH_G_RESOLVE_TIMEOUT`, `stdin=null`, `kill()`), the TOFU known-hosts gate (`ssh-keygen -F` across all reported files, marker/wildcard exclusion), and `App::vault_secret_kinds` candidacy.
- **Phase 3 — connect wiring:** `connect.rs` `env` params, `PendingConnect` state machine, `Screen::PasswordConfirm`, `password_autofill_enabled` opt-in, `App::askpass_listeners` ownership + `drain_askpass` + lock/quit zeroize, the `main.rs` env-presence dispatch.
- **Phase 4 — UI:** pre-connect/pending indicator, password-confirm modal rendering, post-fill/gate-skip toasts.

Each phase produces working, testable software on its own. **Phase 1 must not regress the existing 78 tests and must pass all three CI gates** (`cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings` on Linux *and* Windows, `cargo test --all`).

## File structure (Phase 1)

- **Create `src/os/askpass.rs`** — the whole module. Internal organization:
  - `Classified` enum + `classify(prompt) -> Classified` (pure: parse the prompt shape only).
  - `IdentityBinding` + the password/passphrase release predicates (pure, given resolved identity).
  - `ConnectSecrets` (listener-side: both kinds + resolved identities + per-kind/per-path served state; `Drop`-zeroizes).
  - `expand_identity_path(...)` (tilde + `%`-token expansion + normalization).
  - wire protocol read/write (`write_request`, `read_request`, `write_reply`, `read_reply`) over a generic `Read + Write`.
  - `run_helper(prompt: Option<String>) -> ExitCode` (the helper body; env-driven).
  - platform channel: `Listener` (Windows named pipe / unix socket), `connect_client(addr)`.
  - `Outcome` enum (`Served{kind}` / `Declined{reason}` / `TimedOut` / `NotAttempted`).
- **Modify `src/os/mod.rs`** — add `pub mod askpass;`.
- **Modify `src/os/vault.rs`** — make `random_bytes` reachable from `askpass` (move to a small `pub(crate)` in a shared spot, or `pub(crate) fn random_bytes`). Reject `\r`/`\n` in a secret at entry — *deferred to Phase 3's save path*; Phase 1 only adds a helper `secret_is_one_line(&str) -> bool` used by the helper's refusal path.
- **Modify `Cargo.toml`** — add `windows-sys` (Windows-only, feature-gated).

No existing file's behavior changes in Phase 1; `os/askpass.rs` is compiled but only reached by its own tests and the (Phase-3) helper dispatch.

---

## Task 1: Scaffold the module and shared `random_bytes`

**Files:**
- Create: `src/os/askpass.rs`
- Modify: `src/os/mod.rs`
- Modify: `src/os/vault.rs:413`

- [ ] **Step 1: Expose `random_bytes` to the crate**

In `src/os/vault.rs`, change the signature at line 413 from `fn random_bytes` to `pub(crate) fn random_bytes`. (Leave the body unchanged.)

- [ ] **Step 2: Create the module file with a doc header and a smoke test**

Create `src/os/askpass.rs`:

```rust
//! Connect-time secret auto-fill: sshm as its own `SSH_ASKPASS` helper.
//!
//! The trusted TUI-side *listener* (this process, while running the connect)
//! holds the secrets and decides what to release by classifying the prompt
//! OpenSSH passes to the helper and binding it to the `ssh -G`-resolved
//! identity. The *helper* (a separate `sshm` process, selected by the
//! `SSHM_ASKPASS_CHANNEL` env var) only relays `[token][prompt]` over a
//! user-scoped channel and prints the one secret the listener returns.
//!
//! Zero ratatui dependency (see CLAUDE.md layering).

use crate::os::vault::Secret;

/// Length of the per-connect authentication token (256 bits).
pub const TOKEN_LEN: usize = 32;

#[cfg(test)]
mod tests {
    #[test]
    fn module_compiles() {
        assert_eq!(super::TOKEN_LEN, 32);
    }
}
```

- [ ] **Step 3: Register the module**

In `src/os/mod.rs`, add after `pub mod keys;` (keep alphabetical-ish grouping):

```rust
pub mod askpass;
```

- [ ] **Step 4: Build and run the smoke test**

Run: `cargo test --lib os::askpass::tests::module_compiles`
Expected: PASS (1 test), and `cargo build` clean. The `Secret` import will warn as unused until Task 2 — if clippy `-D warnings` trips on it, add `#[allow(unused_imports)]` to the `use crate::os::vault::Secret;` line **temporarily**; Task 2 removes the allow by using it.

- [ ] **Step 5: Commit**

```bash
git add src/os/askpass.rs src/os/mod.rs src/os/vault.rs
git commit -m "feat(askpass): scaffold the askpass module; share random_bytes"
```

---

## Task 2: Prompt classification (pure)

The listener must decide *which kind of prompt* OpenSSH sent, by shape only. Per spec "Classification + identity binding": passphrase prompts are `Enter passphrase for key '<path>': `; password prompts are exactly `<user>@<host>'s password: ` with **no** leading `(user@host) ` instance prefix; everything else (incl. any `(user@host) …` keyboard-interactive prompt) is rejected.

**Files:**
- Modify: `src/os/askpass.rs`

- [ ] **Step 1: Write the failing test**

Add inside the `tests` module:

```rust
use super::{classify, Classified};

#[test]
fn classify_passphrase_prompt() {
    let p = "Enter passphrase for key '/home/u/.ssh/id_ed25519': ";
    assert_eq!(
        classify(p),
        Classified::Passphrase { key_path: "/home/u/.ssh/id_ed25519".to_string() }
    );
}

#[test]
fn classify_password_prompt() {
    assert_eq!(
        classify("deploy@web1's password: "),
        Classified::Password { user: "deploy".to_string(), host: "web1".to_string() }
    );
}

#[test]
fn classify_rejects_kbd_interactive_and_others() {
    // The (user@host) instance prefix (OpenSSH >=8.5) marks a server-driven
    // keyboard-interactive prompt — never classified as password, even when
    // the server crafts a "'s password: " suffix.
    for p in [
        "(deploy@web1) deploy@web1's password: ",
        "(deploy@web1) Password: ",
        "(deploy@web1) Verification code: ",
        "Password: ",
        "One-time password (OATH): ",
        "[sudo] password for deploy: ",
        "Passwort: ", // localized
        "",
    ] {
        assert_eq!(classify(p), Classified::Other, "should reject: {p:?}");
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib os::askpass::tests::classify`
Expected: FAIL — `classify` / `Classified` not found.

- [ ] **Step 3: Implement `classify`**

Add to `src/os/askpass.rs` (module body, above `tests`):

```rust
/// The shape of a prompt OpenSSH passed to the helper, decided by text only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classified {
    /// A local key-passphrase prompt; carries the key path from the prompt.
    Passphrase { key_path: String },
    /// A password-method prompt; carries the user and host from the prompt.
    Password { user: String, host: String },
    /// Anything else (keyboard-interactive, OTP, sudo, localized, empty) — the
    /// listener returns nothing for these.
    Other,
}

/// Classify a prompt by shape only. Server-controlled keyboard-interactive
/// prompts carry a leading `(user@host) ` instance prefix (OpenSSH >= 8.5) and
/// are always `Other`, even if crafted to end in `'s password: `.
pub fn classify(prompt: &str) -> Classified {
    // Reject the keyboard-interactive instance prefix outright.
    if prompt.starts_with('(') {
        return Classified::Other;
    }
    // Passphrase: Enter passphrase for key '<path>': (trailing space).
    const PP_PREFIX: &str = "Enter passphrase for key '";
    const PP_SUFFIX: &str = "': ";
    if let Some(rest) = prompt.strip_prefix(PP_PREFIX)
        && let Some(path) = rest.strip_suffix(PP_SUFFIX)
        && !path.is_empty()
    {
        return Classified::Passphrase { key_path: path.to_string() };
    }
    // Password: <user>@<host>'s password:  (anchored full string).
    const PW_SUFFIX: &str = "'s password: ";
    if let Some(userhost) = prompt.strip_suffix(PW_SUFFIX)
        && let Some((user, host)) = userhost.split_once('@')
        && !user.is_empty()
        && !host.is_empty()
        && !host.contains('@')
    {
        return Classified::Password { user: user.to_string(), host: host.to_string() };
    }
    Classified::Other
}
```

Remove the temporary `#[allow(unused_imports)]` on the `Secret` import if you added one — it is still unused after this task, so instead move the `use crate::os::vault::Secret;` line down to Task 4 where it is first used, or keep a single `#[allow(unused_imports)]` until Task 4. (Prefer: delete the import now; re-add it in Task 4.)

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib os::askpass::tests::classify`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/os/askpass.rs
git commit -m "feat(askpass): prompt classification (passphrase/password/other)"
```

---

## Task 3: IdentityFile path expansion (pure)

`ssh -G` reports IdentityFile entries **unexpanded** (raw `~`/`%`-tokens). The listener must expand them the way OpenSSH does to compare against the passphrase prompt's path. Per spec: handle `%d %h %i %l %L %u %r %k %n %p %j`; any other token → fail-safe (return `None`, meaning "do not auto-fill").

**Files:**
- Modify: `src/os/askpass.rs`

- [ ] **Step 1: Write the failing test**

```rust
use super::{expand_identity_path, IdentityTokens};

fn toks() -> IdentityTokens {
    IdentityTokens {
        home: "/home/u".into(),
        hostname: "web1.example.com".into(),
        local_user: "u".into(),
        remote_user: "deploy".into(),
        host_key_alias: Some("web1ka".into()),
        host_arg: "web1".into(),
        port: "22".into(),
        proxy_jump_host: None,
        uid: "1000".into(),
        localhost: "mybox".into(),
    }
}

#[test]
fn expand_tilde_and_percent_d() {
    assert_eq!(expand_identity_path("~/.ssh/id_ed25519", &toks()).as_deref(),
        Some("/home/u/.ssh/id_ed25519"));
    assert_eq!(expand_identity_path("%d/.ssh/k", &toks()).as_deref(),
        Some("/home/u/.ssh/k"));
}

#[test]
fn expand_percent_h_k_n_r() {
    assert_eq!(expand_identity_path("~/.ssh/id_%h", &toks()).as_deref(),
        Some("/home/u/.ssh/id_web1.example.com"));
    assert_eq!(expand_identity_path("~/.ssh/id_%k", &toks()).as_deref(),
        Some("/home/u/.ssh/id_web1ka"));
    assert_eq!(expand_identity_path("~/.ssh/id_%r", &toks()).as_deref(),
        Some("/home/u/.ssh/id_deploy"));
}

#[test]
fn unknown_token_is_fail_safe_none() {
    assert_eq!(expand_identity_path("~/.ssh/id_%C", &toks()), None);
    assert_eq!(expand_identity_path("~/.ssh/id_%z", &toks()), None);
}

#[test]
fn percent_percent_is_literal() {
    assert_eq!(expand_identity_path("~/100%%done", &toks()).as_deref(),
        Some("/home/u/100%done"));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib os::askpass::tests::expand`
Expected: FAIL — `expand_identity_path` / `IdentityTokens` not found.

- [ ] **Step 3: Implement expansion**

```rust
/// Resolved token values sourced from one `ssh -G` dump, used to expand
/// IdentityFile entries the way OpenSSH does.
#[derive(Debug, Clone)]
pub struct IdentityTokens {
    pub home: String,
    pub hostname: String,     // ssh -G hostname (%h)
    pub local_user: String,   // OS user (%u)
    pub remote_user: String,  // ssh -G user (%r)
    pub host_key_alias: Option<String>, // %k (else host_arg)
    pub host_arg: String,     // the alias passed to ssh (%n, %k fallback)
    pub port: String,         // ssh -G port (%p)
    pub proxy_jump_host: Option<String>, // %j
    pub uid: String,          // %i
    pub localhost: String,    // %l / %L
}

/// Expand one raw IdentityFile entry (tilde + `%`-tokens) to an absolute path.
/// Returns `None` (fail-safe: do not auto-fill) for any unhandled token, an
/// unresolvable source value, or an unknown `~user`.
pub fn expand_identity_path(raw: &str, t: &IdentityTokens) -> Option<String> {
    // Tilde first (only a leading ~/ or bare ~). ~user is unsupported -> None.
    let after_tilde = if let Some(rest) = raw.strip_prefix("~/") {
        format!("{}/{rest}", t.home)
    } else if raw == "~" {
        t.home.clone()
    } else if raw.starts_with('~') {
        return None; // ~user — unresolvable here
    } else {
        raw.to_string()
    };

    // Percent-token expansion.
    let mut out = String::with_capacity(after_tilde.len());
    let mut chars = after_tilde.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let Some(tok) = chars.next() else { return None };
        let val: String = match tok {
            '%' => "%".into(),
            'd' => t.home.clone(),
            'h' => t.hostname.clone(),
            'i' => t.uid.clone(),
            'l' => t.localhost.clone(),
            'L' => t.localhost.split('.').next().unwrap_or(&t.localhost).to_string(),
            'u' => t.local_user.clone(),
            'r' => t.remote_user.clone(),
            'k' => t.host_key_alias.clone().unwrap_or_else(|| t.host_arg.clone()),
            'n' => t.host_arg.clone(),
            'p' => t.port.clone(),
            'j' => match &t.proxy_jump_host {
                Some(h) => h.clone(),
                None => return None,
            },
            _ => return None, // %C and any other token -> fail-safe
        };
        out.push_str(&val);
    }
    Some(out)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib os::askpass::tests::expand`
Expected: PASS (4 tests). Also run `cargo test --lib os::askpass` (all module tests green).

- [ ] **Step 5: Commit**

```bash
git add src/os/askpass.rs
git commit -m "feat(askpass): IdentityFile tilde + percent-token expansion (fail-safe)"
```

---

## Task 4: `ConnectSecrets` + release decision (pure, with single-shot)

The listener-side state: the armed secrets, the resolved identity to bind against, and the served-state for single-shot. `decide(prompt)` returns the secret to send (and records single-shot), or `None`.

**Files:**
- Modify: `src/os/askpass.rs`

- [ ] **Step 1: Write the failing test**

```rust
use super::{ConnectSecrets, ResolvedIdentity};
use crate::os::vault::{Secret, SecretKind};

fn ident() -> ResolvedIdentity {
    ResolvedIdentity {
        user: "deploy".into(),
        host: "web1".into(),                 // already ASCII-lowercased by ssh -G
        host_key_alias: None,
        identity_paths: vec!["/home/u/.ssh/id_ed25519".into()],
    }
}

#[test]
fn releases_matching_password_once() {
    let mut cs = ConnectSecrets::new(
        ident(),
        Some(Secret::from("hunter2")),
        None,
    );
    // First matching password prompt -> served.
    let r = cs.decide("deploy@web1's password: ");
    assert_eq!(r.as_deref(), Some("hunter2"));
    // Second password prompt on the same connect -> nothing (single-shot).
    assert_eq!(cs.decide("deploy@web1's password: "), None);
}

#[test]
fn withholds_password_for_wrong_host_or_user() {
    let mut cs = ConnectSecrets::new(ident(), Some(Secret::from("hunter2")), None);
    assert_eq!(cs.decide("deploy@evil's password: "), None);
    assert_eq!(cs.decide("root@web1's password: "), None);
    assert_eq!(cs.decide("(deploy@web1) deploy@web1's password: "), None);
}

#[test]
fn passphrase_per_path_single_shot() {
    let mut cs = ConnectSecrets::new(
        ResolvedIdentity {
            identity_paths: vec![
                "/home/u/.ssh/id_a".into(),
                "/home/u/.ssh/id_b".into(),
            ],
            ..ident()
        },
        None,
        Some(Secret::from("pp")),
    );
    // key A: served, then refused on retry of the SAME path.
    assert_eq!(cs.decide("Enter passphrase for key '/home/u/.ssh/id_a': ").as_deref(), Some("pp"));
    assert_eq!(cs.decide("Enter passphrase for key '/home/u/.ssh/id_a': "), None);
    // key B: a DIFFERENT expected path is still served (multi-IdentityFile fallback).
    assert_eq!(cs.decide("Enter passphrase for key '/home/u/.ssh/id_b': ").as_deref(), Some("pp"));
    // an unexpected path is never served.
    assert_eq!(cs.decide("Enter passphrase for key '/home/u/.ssh/id_x': "), None);
}

#[test]
fn no_secret_armed_returns_none() {
    let mut cs = ConnectSecrets::new(ident(), None, None);
    assert_eq!(cs.decide("deploy@web1's password: "), None);
    assert_eq!(cs.decide("Enter passphrase for key '/home/u/.ssh/id_ed25519': "), None);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib os::askpass::tests`
Expected: FAIL — `ConnectSecrets` / `ResolvedIdentity` not found.

- [ ] **Step 3: Implement `ResolvedIdentity` + `ConnectSecrets`**

Add `use crate::os::vault::{Secret, SecretKind};` to the module's `use` block (this is where `Secret` is finally used — remove any earlier temporary import/allow). Then:

```rust
use std::collections::HashSet;

/// The `ssh -G`-resolved identity the listener binds secrets to. `host` is the
/// value OpenSSH's prompt carries: `host_key_alias` verbatim if set, else the
/// already-ASCII-lowercased resolved hostname.
#[derive(Debug, Clone)]
pub struct ResolvedIdentity {
    pub user: String,
    pub host: String,
    pub host_key_alias: Option<String>,
    /// Expected IdentityFile paths, already expanded+normalized by the caller.
    pub identity_paths: Vec<String>,
}

impl ResolvedIdentity {
    /// The host token OpenSSH puts in the password prompt for this identity.
    fn prompt_host(&self) -> &str {
        self.host_key_alias.as_deref().unwrap_or(&self.host)
    }
}

/// Listener-side per-connect secret state. Holds the armed secrets, the
/// identity to bind against, and single-shot served-state. Secrets zeroize on
/// drop (via `Secret`).
#[derive(Debug)]
pub struct ConnectSecrets {
    identity: ResolvedIdentity,
    password: Option<Secret>,
    passphrase: Option<Secret>,
    password_served: bool,
    served_paths: HashSet<String>,
}

impl ConnectSecrets {
    pub fn new(
        identity: ResolvedIdentity,
        password: Option<Secret>,
        passphrase: Option<Secret>,
    ) -> Self {
        ConnectSecrets {
            identity,
            password,
            passphrase,
            password_served: false,
            served_paths: HashSet::new(),
        }
    }

    /// Decide what to send for `prompt`, recording single-shot state. Returns
    /// the secret bytes to send (as a `String`), or `None` to send nothing.
    pub fn decide(&mut self, prompt: &str) -> Option<String> {
        match classify(prompt) {
            Classified::Password { user, host } => {
                let pw = self.password.as_ref()?;
                if self.password_served {
                    return None; // per-kind single-shot
                }
                // ASCII-only comparison: ssh -G already applied OpenSSH's fold.
                if user != self.identity.user
                    || !host.eq_ignore_ascii_case(self.identity.prompt_host())
                {
                    return None;
                }
                self.password_served = true;
                Some(pw.as_str().to_string())
            }
            Classified::Passphrase { key_path } => {
                let pp = self.passphrase.as_ref()?;
                if !self.identity.identity_paths.iter().any(|p| paths_equal(p, &key_path)) {
                    return None;
                }
                if !self.served_paths.insert(key_path) {
                    return None; // per-path single-shot
                }
                Some(pp.as_str().to_string())
            }
            Classified::Other => None,
        }
    }

    /// Which kind, if any, a successful `decide` served — for the outcome enum.
    pub fn last_kind(prompt: &str) -> Option<SecretKind> {
        match classify(prompt) {
            Classified::Password { .. } => Some(SecretKind::Password),
            Classified::Passphrase { .. } => Some(SecretKind::Passphrase),
            Classified::Other => None,
        }
    }
}

/// Compare two already-expanded key paths. On Windows treat `/`≡`\` and fold
/// case; on unix compare exactly. Accounts for the prompt's `%.100s` truncation
/// by comparing the first 100 bytes of each side.
fn paths_equal(expected: &str, from_prompt: &str) -> bool {
    fn norm(s: &str) -> String {
        let truncated: String = s.bytes().take(100).map(|b| b as char).collect();
        if cfg!(windows) {
            truncated.replace('\\', "/").to_ascii_lowercase()
        } else {
            truncated
        }
    }
    norm(expected) == norm(from_prompt)
}
```

> Note on `SecretKind`: it already exists in `os/vault.rs` (`Password` / `Passphrase`). `last_kind` is used by the listener (Task 7) to label the `Outcome`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib os::askpass::tests`
Expected: PASS (all, incl. the 4 new release tests).

- [ ] **Step 5: Commit**

```bash
git add src/os/askpass.rs
git commit -m "feat(askpass): ConnectSecrets release decision with single-shot"
```

---

## Task 5: Wire protocol (token + prompt → reply) over generic streams

The helper and listener speak a tiny framed protocol over the channel. Test it over an in-memory duplex so it is platform-independent.

**Files:**
- Modify: `src/os/askpass.rs`

- [ ] **Step 1: Write the failing test**

```rust
use super::{read_request, write_request, read_reply, write_reply, TOKEN_LEN};
use std::io::Cursor;

#[test]
fn request_roundtrips() {
    let token = [7u8; TOKEN_LEN];
    let mut buf = Vec::new();
    write_request(&mut buf, &token, "deploy@web1's password: ").unwrap();
    let mut cur = Cursor::new(buf);
    let (got_token, got_prompt) = read_request(&mut cur).unwrap();
    assert_eq!(got_token, token);
    assert_eq!(got_prompt, "deploy@web1's password: ");
}

#[test]
fn reply_roundtrips_secret_and_empty() {
    // A served secret.
    let mut buf = Vec::new();
    write_reply(&mut buf, Some("hunter2")).unwrap();
    let mut cur = Cursor::new(buf);
    assert_eq!(read_reply(&mut cur).unwrap().as_deref(), Some("hunter2"));

    // A zero-length (no-match) reply.
    let mut buf = Vec::new();
    write_reply(&mut buf, None).unwrap();
    let mut cur = Cursor::new(buf);
    assert_eq!(read_reply(&mut cur).unwrap(), None);
}

#[test]
fn read_request_rejects_oversized_prompt() {
    // length prefix claims a huge prompt -> error, not allocation blowup.
    let mut buf = Vec::new();
    buf.extend_from_slice(&[0u8; TOKEN_LEN]);
    buf.extend_from_slice(&u32::MAX.to_le_bytes());
    let mut cur = Cursor::new(buf);
    assert!(read_request(&mut cur).is_err());
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib os::askpass::tests::request_roundtrips os::askpass::tests::reply os::askpass::tests::read_request`
Expected: FAIL — protocol fns not found.

- [ ] **Step 3: Implement the protocol**

```rust
use std::io::{self, Read, Write};

/// Upper bound on a framed field, to cap allocation from a hostile length.
const MAX_FIELD: u32 = 64 * 1024;

fn write_lp(w: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    let len: u32 = bytes.len().try_into().map_err(|_| io::ErrorKind::InvalidInput)?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(bytes)
}

fn read_lp(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut lenb = [0u8; 4];
    r.read_exact(&mut lenb)?;
    let len = u32::from_le_bytes(lenb);
    if len > MAX_FIELD {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "field too large"));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

/// Helper → listener: the token then the prompt.
pub fn write_request(w: &mut impl Write, token: &[u8; TOKEN_LEN], prompt: &str) -> io::Result<()> {
    w.write_all(token)?;
    write_lp(w, prompt.as_bytes())?;
    w.flush()
}

/// Listener: read the token and prompt.
pub fn read_request(r: &mut impl Read) -> io::Result<([u8; TOKEN_LEN], String)> {
    let mut token = [0u8; TOKEN_LEN];
    r.read_exact(&mut token)?;
    let prompt = read_lp(r)?;
    let prompt = String::from_utf8(prompt)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "prompt not UTF-8"))?;
    Ok((token, prompt))
}

/// Listener → helper: the chosen secret (length-prefixed) or a zero-length
/// no-match reply.
pub fn write_reply(w: &mut impl Write, secret: Option<&str>) -> io::Result<()> {
    write_lp(w, secret.unwrap_or("").as_bytes())?;
    w.flush()
}

/// Helper: read the reply. A zero-length reply is `None` (send nothing).
pub fn read_reply(r: &mut impl Read) -> io::Result<Option<String>> {
    let bytes = read_lp(r)?;
    if bytes.is_empty() {
        return Ok(None);
    }
    let s = String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "reply not UTF-8"))?;
    Ok(Some(s))
}
```

> Note: an empty served secret is indistinguishable from no-match on the wire, which is correct — an empty secret is never armed (the save path rejects empties, and the helper refuses empty/CRLF secrets in Task 6).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib os::askpass::tests`
Expected: PASS (all).

- [ ] **Step 5: Commit**

```bash
git add src/os/askpass.rs
git commit -m "feat(askpass): framed token+prompt/reply wire protocol"
```

---

## Task 6: Constant-time token compare + one-line secret guard

**Files:**
- Modify: `src/os/askpass.rs`

- [ ] **Step 1: Write the failing test**

```rust
use super::{ct_eq, secret_is_one_line, TOKEN_LEN};

#[test]
fn ct_eq_matches_and_differs() {
    let a = [9u8; TOKEN_LEN];
    let mut b = a;
    assert!(ct_eq(&a, &b));
    b[TOKEN_LEN - 1] ^= 1;
    assert!(!ct_eq(&a, &b));
}

#[test]
fn one_line_guard() {
    assert!(secret_is_one_line("hunter2"));
    assert!(secret_is_one_line("with spaces and ünïçødé"));
    assert!(!secret_is_one_line("two\nlines"));
    assert!(!secret_is_one_line("carriage\rreturn"));
    // OpenSSH reads <=1023 bytes; refuse longer rather than truncate.
    assert!(!secret_is_one_line(&"x".repeat(1024)));
    assert!(secret_is_one_line(&"x".repeat(1023)));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib os::askpass::tests::ct_eq os::askpass::tests::one_line`
Expected: FAIL — fns not found.

- [ ] **Step 3: Implement**

```rust
/// Constant-time equality for the per-connect token (length is fixed).
pub fn ct_eq(a: &[u8; TOKEN_LEN], b: &[u8; TOKEN_LEN]) -> bool {
    let mut diff = 0u8;
    for i in 0..TOKEN_LEN {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// True if `s` can be delivered verbatim over OpenSSH's line-oriented askpass
/// channel: no `\r`/`\n` (OpenSSH truncates at the first one) and within its
/// 1023-byte read cap. The helper refuses to serve a secret that fails this.
pub fn secret_is_one_line(s: &str) -> bool {
    s.len() <= 1023 && !s.contains(['\r', '\n'])
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib os::askpass::tests`
Expected: PASS (all).

- [ ] **Step 5: Commit**

```bash
git add src/os/askpass.rs
git commit -m "feat(askpass): constant-time token compare + one-line secret guard"
```

---

## Task 7: Platform channel — unix socket arm (real, tested)

The unix arm is a real implementation (no stub) so the `#[cfg(unix)]` code is exercised on the Linux CI gate. It listens on a socket in a `0700` dir, accepts sequential clients, and drives the protocol against a `ConnectSecrets`. Windows named-pipe arm follows in Task 8.

**Files:**
- Modify: `src/os/askpass.rs`

- [ ] **Step 1: Write the failing test (unix-gated)**

```rust
#[cfg(unix)]
#[test]
fn unix_channel_serves_one_request() {
    use super::{Listener, connect_client, write_request, read_reply, TOKEN_LEN, ConnectSecrets, ResolvedIdentity};
    use crate::os::vault::Secret;
    use std::thread;

    let token = [3u8; TOKEN_LEN];
    let listener = Listener::bind(token).unwrap();
    let addr = listener.address().to_string();

    // Listener side: serve exactly one request from a background thread.
    let handle = thread::spawn(move || {
        let ident = ResolvedIdentity {
            user: "deploy".into(), host: "web1".into(),
            host_key_alias: None, identity_paths: vec![],
        };
        let mut secrets = ConnectSecrets::new(ident, Some(Secret::from("hunter2")), None);
        listener.serve_one(&mut secrets).unwrap();
    });

    // Client side: present the token + prompt, read the reply.
    let mut conn = connect_client(&addr).unwrap();
    write_request(&mut conn, &token, "deploy@web1's password: ").unwrap();
    let reply = read_reply(&mut conn).unwrap();
    assert_eq!(reply.as_deref(), Some("hunter2"));

    handle.join().unwrap();
}

#[cfg(unix)]
#[test]
fn unix_channel_rejects_bad_token() {
    use super::{Listener, connect_client, write_request, read_reply, TOKEN_LEN, ConnectSecrets, ResolvedIdentity};
    use crate::os::vault::Secret;
    use std::thread;

    let token = [3u8; TOKEN_LEN];
    let listener = Listener::bind(token).unwrap();
    let addr = listener.address().to_string();
    let handle = thread::spawn(move || {
        let ident = ResolvedIdentity { user: "deploy".into(), host: "web1".into(), host_key_alias: None, identity_paths: vec![] };
        let mut secrets = ConnectSecrets::new(ident, Some(Secret::from("hunter2")), None);
        // serve_one returns Ok(false) when the token mismatched.
        let served = listener.serve_one(&mut secrets).unwrap();
        assert!(!served);
    });
    let mut conn = connect_client(&addr).unwrap();
    write_request(&mut conn, &[0u8; TOKEN_LEN], "deploy@web1's password: ").unwrap();
    // No reply on bad token -> read_reply sees EOF/error.
    let _ = read_reply(&mut conn);
    handle.join().unwrap();
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib os::askpass::tests::unix_channel`
Expected: FAIL — `Listener` / `connect_client` not found. (On Windows these tests are cfg-skipped; verify on the Linux gate.)

- [ ] **Step 3: Implement the unix arm**

```rust
/// The per-connect authentication token, carried in the env to the helper.
pub type Token = [u8; TOKEN_LEN];

#[cfg(unix)]
mod chan {
    use super::*;
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;

    /// A user-scoped channel: a unix socket inside a 0700 directory.
    pub struct Listener {
        inner: UnixListener,
        path: PathBuf,
        dir: PathBuf,
        token: Token,
    }

    impl Listener {
        pub fn bind(token: Token) -> io::Result<Listener> {
            let base = std::env::temp_dir();
            let suffix = crate::os::vault::random_bytes(8)
                .map_err(|e| io::Error::other(e.to_string()))?;
            let hex: String = suffix.iter().map(|b| format!("{b:02x}")).collect();
            let dir = base.join(format!("sshm-askpass-{}-{hex}", std::process::id()));
            std::fs::DirBuilder::new().mode(0o700).recursive(false).create(&dir)?;
            let path = dir.join("sock");
            let inner = UnixListener::bind(&path)?;
            Ok(Listener { inner, path, dir, token })
        }

        /// The address string passed to the helper via SSHM_ASKPASS_CHANNEL.
        pub fn address(&self) -> &str {
            self.path.to_str().unwrap_or("")
        }

        /// Accept one client, verify the token, and serve at most one secret.
        /// Returns Ok(true) if a secret (or a deliberate no-match) was served to
        /// a token-valid client, Ok(false) if the token mismatched.
        pub fn serve_one(&self, secrets: &mut ConnectSecrets) -> io::Result<bool> {
            let (mut conn, _) = self.inner.accept()?;
            let (token, prompt) = read_request(&mut conn)?;
            if !ct_eq(&token, &self.token) {
                return Ok(false); // drop without reply
            }
            let secret = secrets.decide(&prompt);
            // Refuse a secret that cannot survive the line channel.
            let to_send = match secret {
                Some(s) if secret_is_one_line(&s) => Some(s),
                _ => None,
            };
            write_reply(&mut conn, to_send.as_deref())?;
            Ok(true)
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_dir(&self.dir);
        }
    }

    /// Helper side: connect to the listener's address.
    pub fn connect_client(addr: &str) -> io::Result<UnixStream> {
        UnixStream::connect(addr)
    }
}

#[cfg(unix)]
pub use chan::{connect_client, Listener};
```

> The Windows arm (Task 8) provides the same `Listener`/`connect_client` surface, so Task 9's helper body is platform-neutral.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib os::askpass::tests::unix_channel`
Expected: PASS (2 tests) on Linux/macOS.

- [ ] **Step 5: Commit**

```bash
git add src/os/askpass.rs
git commit -m "feat(askpass): unix-socket channel (0700 dir, token-gated serve)"
```

---

## Task 8: Platform channel — Windows named-pipe arm

A single persistent pipe instance with a SID-only DACL, reused across sequential clients. Same `Listener`/`connect_client` surface as the unix arm.

**Files:**
- Modify: `src/os/askpass.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1: Add the dependency**

In `Cargo.toml`, under `[target.'cfg(windows)'.dependencies]` (create the table if absent):

```toml
[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.59", features = [
    "Win32_Foundation",
    "Win32_Security",
    "Win32_Security_Authorization",
    "Win32_System_Pipes",
    "Win32_Storage_FileSystem",
] }
```

Run: `cargo build` (on Windows) — Expected: the crate resolves and builds. (On Linux this table is inactive.)

- [ ] **Step 2: Write the failing test (windows-gated)**

```rust
#[cfg(windows)]
#[test]
fn windows_channel_serves_two_sequential_requests() {
    use super::{Listener, connect_client, write_request, read_reply, TOKEN_LEN, ConnectSecrets, ResolvedIdentity};
    use crate::os::vault::Secret;
    use std::thread;

    let token = [5u8; TOKEN_LEN];
    let listener = Listener::bind(token).unwrap();
    let addr = listener.address().to_string();

    let handle = thread::spawn(move || {
        let ident = ResolvedIdentity {
            user: "deploy".into(), host: "web1".into(),
            host_key_alias: None,
            identity_paths: vec!["C:/Users/u/.ssh/id_ed25519".into()],
        };
        let mut secrets = ConnectSecrets::new(ident, Some(Secret::from("hunter2")), Some(Secret::from("pp")));
        // Serve two sequential clients over the SAME persistent instance.
        listener.serve_one(&mut secrets).unwrap();
        listener.serve_one(&mut secrets).unwrap();
    });

    let mut c1 = connect_client(&addr).unwrap();
    write_request(&mut c1, &token, "deploy@web1's password: ").unwrap();
    assert_eq!(read_reply(&mut c1).unwrap().as_deref(), Some("hunter2"));
    drop(c1);

    let mut c2 = connect_client(&addr).unwrap();
    write_request(&mut c2, &token, "Enter passphrase for key 'C:/Users/u/.ssh/id_ed25519': ").unwrap();
    assert_eq!(read_reply(&mut c2).unwrap().as_deref(), Some("pp"));

    handle.join().unwrap();
}
```

- [ ] **Step 3: Implement the Windows arm**

Add a `#[cfg(windows)] mod chan { ... }` exposing the same `Listener` (with `bind`/`address`/`serve_one`/`Drop`) and `connect_client`, plus `pub use chan::{connect_client, Listener};` gated `#[cfg(windows)]`. Implementation requirements (from spec "Channel + wire protocol" / "Dependency posture"):

```rust
#[cfg(windows)]
mod chan {
    use super::*;
    use std::os::windows::io::{FromRawHandle, AsRawHandle};
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, CloseHandle, LocalFree};
    use windows_sys::Win32::Security::{SECURITY_ATTRIBUTES, PSECURITY_DESCRIPTOR};
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows_sys::Win32::System::Pipes::{
        CreateNamedPipeW, ConnectNamedPipe, DisconnectNamedPipe,
        PIPE_ACCESS_DUPLEX, PIPE_TYPE_BYTE, PIPE_WAIT, FILE_FLAG_FIRST_PIPE_INSTANCE,
    };

    pub struct Listener {
        handle: HANDLE,      // the single owned, squat-protected instance
        name: Vec<u16>,      // \\.\pipe\... as a wide string for clients
        addr: String,
        token: Token,
        sd: PSECURITY_DESCRIPTOR, // owned; LocalFree on drop
    }

    // SAFETY: the HANDLE is owned solely by this Listener and only touched on
    // the listener thread.
    unsafe impl Send for Listener {}

    impl Listener {
        pub fn bind(token: Token) -> io::Result<Listener> {
            // 1. Random, unpredictable name: \\.\pipe\sshm-askpass-<pid>-<hex>.
            //    Use crate::os::vault::random_bytes(16) for >=128 bits.
            // 2. Build a SID-only DACL via
            //    ConvertStringSecurityDescriptorToSecurityDescriptorW with SDDL
            //    "D:(A;;GA;;;<current-user-SID>)" — obtain the SID from
            //    OpenProcessToken + GetTokenInformation(TokenUser), or use the
            //    well-known current-user SDDL string "D:(A;;GA;;;OW)" is NOT
            //    sufficient; resolve the actual user SID. Put the resulting
            //    PSECURITY_DESCRIPTOR into a SECURITY_ATTRIBUTES.
            // 3. CreateNamedPipeW(name, PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            //    PIPE_TYPE_BYTE | PIPE_WAIT, nMaxInstances=1, out_buf, in_buf,
            //    default_timeout, &security_attributes). INVALID_HANDLE_VALUE -> Err.
            // Keep `handle` for the whole lifetime (squat protection).
            unimplemented!("see steps 1-3; do NOT ship unimplemented! — implement fully")
        }

        pub fn address(&self) -> &str { &self.addr }

        pub fn serve_one(&self, secrets: &mut ConnectSecrets) -> io::Result<bool> {
            // ConnectNamedPipe(self.handle, null). ERROR_PIPE_CONNECTED is OK.
            // Borrow the handle for I/O WITHOUT taking ownership: do raw
            // ReadFile/WriteFile on self.handle (Win32_Storage_FileSystem), OR
            // wrap a *borrowed* File that you std::mem::forget so Drop does not
            // CloseHandle the persistent instance. Then:
            //   - read_request(&mut io) over the borrowed reader
            //   - if !ct_eq -> DisconnectNamedPipe, return Ok(false)
            //   - decide + secret_is_one_line guard -> write_reply
            //   - DisconnectNamedPipe(self.handle) to ready the next client
            unimplemented!("implement per the comment; reuse the SAME handle")
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            // SAFETY: handle/sd owned by self.
            unsafe {
                if !self.handle.is_null() && self.handle != INVALID_HANDLE_VALUE {
                    DisconnectNamedPipe(self.handle);
                    CloseHandle(self.handle);
                }
                if !self.sd.is_null() {
                    LocalFree(self.sd as _);
                }
            }
        }
    }

    pub fn connect_client(addr: &str) -> io::Result<std::fs::File> {
        // OpenOptions::new().read(true).write(true).open(addr) works for a
        // byte-mode named pipe path "\\.\pipe\...". Retry on ERROR_PIPE_BUSY
        // with WaitNamedPipeW if needed.
        use std::fs::OpenOptions;
        OpenOptions::new().read(true).write(true).open(addr)
    }
}

#[cfg(windows)]
pub use chan::{connect_client, Listener};
```

> **Implementer note:** the `unimplemented!()` markers above are a *sketch of the required FFI*, not acceptable shipped code — the task is to implement them fully per the inline comments and the spec's "HANDLE ownership" rule (the single owned resource is the raw HANDLE, closed only at teardown; never let a per-connection `File` drop and `CloseHandle` it). The SDDL must encode the **actual current-user SID**, not a generic well-known SID. Confine FFI to `bind` (create+secure) and the connect/disconnect calls; keep the byte loop in safe Rust via the borrowed reader/writer.

- [ ] **Step 4: Run the tests to verify they pass (on Windows)**

Run: `cargo test --lib os::askpass::tests::windows_channel`
Expected: PASS (1 test). Also run the full suite on Windows: `cargo test --all`.

- [ ] **Step 5: Commit**

```bash
git add src/os/askpass.rs Cargo.toml
git commit -m "feat(askpass): Windows named-pipe channel (single instance, SID DACL)"
```

---

## Task 9: The helper body — `run_helper`

The standalone `sshm <prompt>` path: env-driven, short-circuiting, prints the reply or exits non-zero. Tested by pointing it at a real in-process listener via env vars.

**Files:**
- Modify: `src/os/askpass.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(unix)]
#[test]
fn helper_prints_served_secret() {
    use super::{Listener, run_helper, TOKEN_LEN, ConnectSecrets, ResolvedIdentity, CHANNEL_ENV, TOKEN_ENV};
    use crate::os::vault::Secret;
    use std::thread;

    let token = [4u8; TOKEN_LEN];
    let listener = Listener::bind(token).unwrap();
    let addr = listener.address().to_string();
    let handle = thread::spawn(move || {
        let ident = ResolvedIdentity { user: "deploy".into(), host: "web1".into(), host_key_alias: None, identity_paths: vec![] };
        let mut secrets = ConnectSecrets::new(ident, Some(Secret::from("hunter2")), None);
        listener.serve_one(&mut secrets).unwrap();
    });

    // run_helper reads CHANNEL/TOKEN from a passed-in Env abstraction so the
    // test need not mutate process-global env.
    let hextok: String = token.iter().map(|b| format!("{b:02x}")).collect();
    let out = run_helper(
        Some("deploy@web1's password: ".to_string()),
        |k| match k {
            CHANNEL_ENV => Some(addr.clone()),
            TOKEN_ENV => Some(hextok.clone()),
            _ => None,
        },
    );
    assert_eq!(out.unwrap(), b"hunter2\n");
    handle.join().unwrap();
}

#[test]
fn helper_without_channel_env_is_none() {
    use super::run_helper;
    // No SSHM_ASKPASS_CHANNEL -> not in askpass mode -> None (caller falls through).
    let out = run_helper(Some("x".into()), |_| None);
    assert!(out.is_none());
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib os::askpass::tests::helper`
Expected: FAIL — `run_helper` / `CHANNEL_ENV` / `TOKEN_ENV` not found.

- [ ] **Step 3: Implement `run_helper`**

```rust
/// Env var carrying the channel address; its PRESENCE selects askpass mode.
pub const CHANNEL_ENV: &str = "SSHM_ASKPASS_CHANNEL";
/// Env var carrying the per-connect token (hex).
pub const TOKEN_ENV: &str = "SSHM_ASKPASS_TOKEN";

fn parse_token_hex(s: &str) -> Option<Token> {
    if s.len() != TOKEN_LEN * 2 {
        return None;
    }
    let mut out = [0u8; TOKEN_LEN];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// The askpass helper body. `get_env` abstracts environment lookup for testing.
///
/// Returns:
/// - `None` — not in askpass mode (no `SSHM_ASKPASS_CHANNEL`); the caller should
///   continue normal CLI parsing.
/// - `Some(bytes)` — askpass mode succeeded; print `bytes` to stdout and exit 0.
///   (Bytes already include the single trailing `\n`.)
/// - `Some(empty)` is never returned; a no-match/error yields `Some(Vec::new())`
///   meaning "exit non-zero, no stdout" — the caller distinguishes by emptiness.
///
/// Contract for the caller (Phase 3 main.rs): if this returns `Some(b)` with
/// `b` non-empty, write `b` to stdout and exit 0; if `Some(b)` empty, exit
/// non-zero with no stdout; if `None`, fall through to normal arg parsing.
pub fn run_helper<F>(prompt: Option<String>, get_env: F) -> Option<Vec<u8>>
where
    F: Fn(&str) -> Option<String>,
{
    let addr = get_env(CHANNEL_ENV)?; // PRESENCE selects askpass mode
    // From here we are committed to askpass mode: always Some(...), never None.
    let fail = || Some(Vec::new());

    let Some(prompt) = prompt else { return fail() };
    let Some(token) = get_env(TOKEN_ENV).as_deref().and_then(parse_token_hex) else {
        return fail();
    };

    let result = (|| -> io::Result<Option<String>> {
        let mut conn = connect_client(&addr)?;
        write_request(&mut conn, &token, &prompt)?;
        read_reply(&mut conn)
    })();

    match result {
        Ok(Some(secret)) if secret_is_one_line(&secret) => {
            let mut bytes = secret.into_bytes();
            bytes.push(b'\n');
            Some(bytes)
        }
        _ => fail(),
    }
}
```

> The `Some(empty)` vs `Some(non-empty)` convention keeps `run_helper` free of `std::process::exit`, so it is unit-testable; Phase 3's `main.rs` performs the actual stdout-write/exit.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib os::askpass::tests`
Expected: PASS (all module tests).

- [ ] **Step 5: Commit**

```bash
git add src/os/askpass.rs
git commit -m "feat(askpass): env-driven helper body (run_helper)"
```

---

## Task 10: Outcome enum + module gating, then full gate run

`Outcome` is the listener's terminal signal (consumed by Phase 3/4). Phase 1 only defines it and ensures the whole module passes clippy on both OSes without dead-code noise.

**Files:**
- Modify: `src/os/askpass.rs`

- [ ] **Step 1: Add the `Outcome` enum**

```rust
/// The terminal result of a connect's auto-fill, surfaced to the UI (Phase 3/4)
/// and used as the detached-listener reap signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Served { kind: SecretKind },
    Declined { reason: DeclineReason },
    TimedOut,
    NotAttempted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclineReason {
    /// The user declined the password-confirm modal.
    PasswordDeclined,
    /// The server used keyboard-interactive, so a stored password was withheld.
    KeyboardInteractive,
    /// Channel/token/identity mismatch or an unclassifiable prompt.
    NoMatch,
}
```

- [ ] **Step 2: Add a `#![allow]`-free dead-code guard**

Because nothing outside the module calls these public items until Phase 3, add a module-level allow ONLY for the items genuinely unused until then, scoped tightly. Prefer a single `#[allow(dead_code)]` on `Outcome`/`DeclineReason` (and any other Phase-3-only item) with a `// TODO(phase3): consumed by the connect wiring` comment, rather than a blanket module allow. Everything else is exercised by tests.

- [ ] **Step 3: Run all three CI gates locally**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all
```

Expected: fmt clean; clippy clean; **all prior 78 tests still pass** plus the new `os::askpass` tests. If clippy flags an unused public item, gate it with the scoped `#[allow(dead_code)]` + TODO as above (do NOT silence with a blanket module allow).

- [ ] **Step 4: Cross-platform clippy note**

The unix `chan` module is `#[cfg(unix)]`; on Windows it is inactive and the Windows `chan` is active. Confirm there is **no** always-present struct field read by only one arm (per spec "Cross-platform clippy discipline" — `Listener`'s fields are themselves inside the cfg-gated `chan` module, so this is satisfied). If CI is available, run the Linux gate explicitly: `cargo clippy --all-targets --target x86_64-unknown-linux-gnu -- -D warnings`.

- [ ] **Step 5: Commit**

```bash
git add src/os/askpass.rs
git commit -m "feat(askpass): outcome enum; phase-1 module passes all gates"
```

---

## Self-review checklist (run before handoff)

- [ ] **Spec coverage (Phase 1 scope):** classification ✓ (Task 2), identity-path expansion ✓ (Task 3), release decision + per-kind/per-path single-shot ✓ (Task 4), wire protocol + oversized-field guard ✓ (Task 5), constant-time token + one-line/`\r\n`/1023-byte guard ✓ (Task 6), unix channel + token reject ✓ (Task 7), Windows named-pipe single-instance SID-DACL ✓ (Task 8), env-presence helper dispatch + short-circuit ✓ (Task 9), Outcome enum ✓ (Task 10). Deferred to later phases (documented in "Scope"): `ssh -G` resolver, TOFU gate, connect wiring, opt-in, UI, secret-entry `\r\n` rejection on the save path.
- [ ] **Placeholder scan:** the only `unimplemented!()` is the Windows-FFI sketch in Task 8, explicitly flagged as "implement fully, do not ship" with the exact Win32 calls and the HANDLE-ownership rule named — acceptable as a guided-implementation task since the FFI cannot be shown as finished portable code here. No "TBD"/"add error handling"/"similar to" placeholders elsewhere.
- [ ] **Type consistency:** `Listener::{bind,address,serve_one}`, `connect_client`, `ConnectSecrets::{new,decide,last_kind}`, `ResolvedIdentity{user,host,host_key_alias,identity_paths}`, `IdentityTokens` fields, `classify`/`Classified`, `write_request`/`read_request`/`write_reply`/`read_reply`, `ct_eq`/`secret_is_one_line`, `run_helper`/`CHANNEL_ENV`/`TOKEN_ENV`, `Outcome`/`DeclineReason`, `SecretKind` (existing) are used consistently across tasks. `Secret::from(&str)` and `Secret::as_str()` exist in `os/vault.rs`.
- [ ] **No regressions:** no existing file's behavior changes except `random_bytes` visibility; the 78 existing tests must stay green (Task 10 gate).
