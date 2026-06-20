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

use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use zeroize::Zeroizing;

use crate::os::vault::{Secret, SecretKind};

/// Length of the per-connect authentication token (256 bits).
pub const TOKEN_LEN: usize = 32;

/// Upper bound on a framed field, to cap allocation from a hostile length.
const MAX_FIELD: u32 = 64 * 1024;

fn write_lp(w: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    let len: u32 = bytes
        .len()
        .try_into()
        .map_err(|_| io::ErrorKind::InvalidInput)?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(bytes)
}

fn read_lp(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut lenb = [0u8; 4];
    r.read_exact(&mut lenb)?;
    let len = u32::from_le_bytes(lenb);
    if len > MAX_FIELD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "field too large",
        ));
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

/// Helper: read the reply. A zero-length reply is `None` (send nothing). The
/// secret is wrapped in `Zeroizing` so this transient plaintext copy is scrubbed
/// on drop rather than left in the helper's freed heap.
pub fn read_reply(r: &mut impl Read) -> io::Result<Option<Zeroizing<String>>> {
    let bytes = Zeroizing::new(read_lp(r)?);
    if bytes.is_empty() {
        return Ok(None);
    }
    // Validate IN PLACE: `String::from_utf8` would hand back a non-Zeroizing Vec
    // it owns on the error path, dropping a transient un-scrubbed plaintext copy
    // (#13). `str::from_utf8` borrows; the only secret allocation is the Zeroizing
    // String built from the validated &str.
    let s = std::str::from_utf8(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "reply not UTF-8"))?;
    Ok(Some(Zeroizing::new(s.to_owned())))
}

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
        return Classified::Passphrase {
            key_path: path.to_string(),
        };
    }
    // Password: <user>@<host>'s password:  (anchored full string).
    const PW_SUFFIX: &str = "'s password: ";
    if let Some(userhost) = prompt.strip_suffix(PW_SUFFIX)
        && let Some((user, host)) = userhost.split_once('@')
        && !user.is_empty()
        && !host.is_empty()
        && !host.contains('@')
    {
        return Classified::Password {
            user: user.to_string(),
            host: host.to_string(),
        };
    }
    Classified::Other
}

/// True when the resolved `ssh` is OpenSSH >= 8.5, where keyboard-interactive
/// prompts carry the `(user@host) ` instance prefix that [`classify`] relies on
/// to refuse serving a stored password to a server-driven prompt. On older
/// clients (or when the version can't be read) returns false and the caller
/// withholds the *password* secret (passphrase auto-fill is local-only and stays
/// on). Probed once via `ssh -V`. (#6)
pub fn ssh_kbdint_prefix_supported() -> bool {
    static SSH_KBDINT_PREFIX: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SSH_KBDINT_PREFIX.get_or_init(|| ssh_version_at_least(8, 5).unwrap_or(false))
}

fn ssh_version_at_least(maj: u32, min: u32) -> Option<bool> {
    let out = std::process::Command::new(&crate::os::binaries::tools().ssh)
        .arg("-V")
        .output()
        .ok()?;
    // OpenSSH prints the banner to stderr; fall back to stdout if empty.
    let raw = if out.stderr.is_empty() {
        &out.stdout
    } else {
        &out.stderr
    };
    parse_openssh_atleast(&String::from_utf8_lossy(raw), maj, min)
}

/// Pure parser for an `ssh -V` banner: returns whether it is >= maj.min. Splits
/// after the first `OpenSSH_` token and reads the first two integer fields.
fn parse_openssh_atleast(text: &str, maj: u32, min: u32) -> Option<bool> {
    let after = text.split("OpenSSH_").nth(1)?;
    let mut nums = after
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty());
    let got_maj: u32 = nums.next()?.parse().ok()?;
    let got_min: u32 = nums.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    Some(got_maj > maj || (got_maj == maj && got_min >= min))
}

/// Resolved token values sourced from one `ssh -G` dump, used to expand
/// IdentityFile entries the way OpenSSH does.
#[derive(Debug, Clone)]
pub struct IdentityTokens {
    pub home: String,
    pub hostname: String,                // ssh -G hostname (%h)
    pub local_user: String,              // OS user (%u)
    pub remote_user: String,             // ssh -G user (%r)
    pub host_key_alias: Option<String>,  // %k (else host_arg)
    pub host_arg: String,                // the alias passed to ssh (%n, %k fallback)
    pub port: String,                    // ssh -G port (%p)
    pub proxy_jump_host: Option<String>, // %j
    pub uid: String,                     // %i
    pub localhost: String,               // %l / %L
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
        let tok = chars.next()?;
        let val: String = match tok {
            '%' => "%".into(),
            'd' => t.home.clone(),
            'h' => t.hostname.clone(),
            'i' => t.uid.clone(),
            'l' => t.localhost.clone(),
            'L' => t
                .localhost
                .split('.')
                .next()
                .unwrap_or(&t.localhost)
                .to_string(),
            'u' => t.local_user.clone(),
            'r' => t.remote_user.clone(),
            'k' => t
                .host_key_alias
                .clone()
                .unwrap_or_else(|| t.host_arg.clone()),
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

/// OS-sourced expansion inputs that `ssh -G` does not provide (`%d` home, `%u`
/// local user, `%i` uid, `%l`/`%L` localhost). Injected so the bridge below is
/// unit-testable; [`os_tokens`] fills them from the environment.
#[derive(Debug, Clone)]
pub struct OsTokens {
    pub home: String,
    pub local_user: String,
    pub uid: String,
    pub localhost: String,
}

/// Build the `ssh -G`-bound [`ResolvedIdentity`] for `host_arg` (the alias passed
/// to ssh) from a parsed [`crate::os::resolve::ResolvedConfig`] plus the OS-sourced
/// tokens. The IdentityFile set is the subset of `ssh -G`-reported entries that
/// expand cleanly; an entry with an unhandled token (`%C`, `~user`, …) is dropped
/// — fail-safe, the listener simply will not release a passphrase for it.
pub fn resolved_identity(
    rc: &crate::os::resolve::ResolvedConfig,
    host_arg: &str,
    os: &OsTokens,
) -> ResolvedIdentity {
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
        // `ssh -G` already ASCII-lowercased hostname — the exact form the prompt
        // carries — so do NOT re-fold it here.
        host: rc.hostname.clone().unwrap_or_else(|| host_arg.to_string()),
        host_key_alias: rc.host_key_alias.clone(),
        identity_paths,
    }
}

/// Fill [`OsTokens`] from the environment. Best-effort: a value the environment
/// does not expose becomes empty, so an IdentityFile token that needs it fails to
/// match (fail-safe, no auto-fill). `%i` (uid) has no portable `std` source and is
/// left empty — a `%i`-bearing IdentityFile is rare and degrades to manual.
pub fn os_tokens() -> OsTokens {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    let local_user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    let localhost = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_default();
    OsTokens {
        home,
        local_user,
        uid: String::new(),
        localhost,
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

    /// Decide what to send for `prompt`, recording single-shot state. Returns the
    /// secret to send, or `None` to send nothing. The chosen value is copied into a
    /// `Zeroizing<String>` so this transient plaintext (a distinct allocation from
    /// the armed `Secret`, which the worker zeroizes separately) is scrubbed on drop
    /// rather than left in the long-running listener's freed heap.
    pub fn decide(&mut self, prompt: &str) -> Option<Zeroizing<String>> {
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
                Some(Zeroizing::new(pw.as_str().to_string()))
            }
            Classified::Passphrase { key_path } => {
                let pp = self.passphrase.as_ref()?;
                if !self
                    .identity
                    .identity_paths
                    .iter()
                    .any(|p| paths_equal(p, &key_path))
                {
                    return None;
                }
                if !self.served_paths.insert(key_path) {
                    return None; // per-path single-shot
                }
                Some(Zeroizing::new(pp.as_str().to_string()))
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

/// The per-connect authentication token, carried in the env to the helper.
pub type Token = [u8; TOKEN_LEN];

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

/// Hex-encode the token for the `SSHM_ASKPASS_TOKEN` env var.
fn token_hex(t: &Token) -> String {
    t.iter().map(|b| format!("{b:02x}")).collect()
}

/// The env `(name, value)` pairs to apply to the connect `Command` so OpenSSH
/// routes its prompts to this sshm-as-helper: the normalized exe as
/// `SSH_ASKPASS`, `SSH_ASKPASS_REQUIRE=force` (use the helper even with a TTY),
/// and the channel address + per-connect token. Secrets never appear here — only
/// the token + address do.
pub fn arm_env(
    channel_addr: &str,
    token: &Token,
) -> io::Result<Vec<(std::ffi::OsString, std::ffi::OsString)>> {
    use std::ffi::OsString;
    Ok(vec![
        (
            OsString::from("SSH_ASKPASS"),
            OsString::from(askpass_exe_path()?),
        ),
        (
            OsString::from("SSH_ASKPASS_REQUIRE"),
            OsString::from("force"),
        ),
        (OsString::from(CHANNEL_ENV), OsString::from(channel_addr)),
        (OsString::from(TOKEN_ENV), OsString::from(token_hex(token))),
    ])
}

/// Strip a Windows verbatim path prefix so the result is a plain path that
/// Win32-OpenSSH's `CreateProcessW` accepts as argv0: `\\?\C:\…` -> `C:\…`,
/// `\\?\UNC\server\share\…` -> `\\server\share\…`. Any other string passes
/// through unchanged. Operates on the string form (not `Path::components`) so it
/// is unit-testable with synthetic inputs on every platform (the Linux gate runs
/// these too). `current_exe()` emits the verbatim prefix for installs at a path
/// longer than 260 chars, which OpenSSH would otherwise reject.
pub fn strip_verbatim_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        return rest.to_string();
    }
    path.to_string()
}

/// The absolute, prefix-normalized path to the running sshm executable, for the
/// `SSH_ASKPASS` env var. `current_exe()` already calls `GetModuleFileNameW`
/// internally, so the only fix-up needed is the verbatim-prefix strip.
pub fn askpass_exe_path() -> io::Result<String> {
    let exe = std::env::current_exe()?;
    Ok(strip_verbatim_prefix(&exe.to_string_lossy()))
}

/// The askpass helper body. `get_env` abstracts environment lookup for testing.
///
/// Returns:
/// - `None` — not in askpass mode (no `SSHM_ASKPASS_CHANNEL`); the caller should
///   continue normal CLI parsing.
/// - `Some(bytes)` — askpass mode succeeded; print `bytes` to stdout and exit 0.
///   (Bytes already include the single trailing `\n`.) The buffer is `Zeroizing`
///   so it scrubs on drop; the caller must still scrub explicitly before
///   `process::exit` (which runs no destructors).
/// - `Some(empty)` is never returned; a no-match/error yields a zero-length
///   buffer meaning "exit non-zero, no stdout" — the caller distinguishes by emptiness.
///
/// Contract for the caller (Phase 3 main.rs): if this returns `Some(b)` with
/// `b` non-empty, write `b` to stdout and exit 0; if `Some(b)` empty, exit
/// non-zero with no stdout; if `None`, fall through to normal arg parsing.
pub fn run_helper<F>(prompt: Option<String>, get_env: F) -> Option<Zeroizing<Vec<u8>>>
where
    F: Fn(&str) -> Option<String>,
{
    let addr = get_env(CHANNEL_ENV)?; // PRESENCE selects askpass mode
    // From here we are committed to askpass mode: always Some(...), never None.
    let fail = || Some(Zeroizing::new(Vec::new()));

    let Some(prompt) = prompt else { return fail() };
    let Some(token) = get_env(TOKEN_ENV).as_deref().and_then(parse_token_hex) else {
        return fail();
    };

    let result = (|| -> io::Result<Option<Zeroizing<String>>> {
        let mut conn = connect_client(&addr)?;
        write_request(&mut conn, &token, &prompt)?;
        read_reply(&mut conn)
    })();

    match result {
        Ok(Some(secret)) if secret_is_one_line(&secret) => {
            // Copy into a zeroizing buffer + append the newline; the source
            // `Zeroizing<String>` scrubs when this fn returns.
            let mut bytes = Zeroizing::new(Vec::with_capacity(secret.len() + 1));
            bytes.extend_from_slice(secret.as_bytes());
            bytes.push(b'\n');
            Some(bytes)
        }
        _ => fail(),
    }
}

/// The terminal result of a connect's auto-fill, surfaced to the UI (Phase 3/4)
/// and used as the detached-listener reap signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Served {
        kind: SecretKind,
    },
    Declined {
        reason: DeclineReason,
    },
    /// Returned by `AskpassListener::shutdown` when the worker does not acknowledge
    /// stop within `SHUTDOWN_GRACE` and is detached — a peer parked mid-handshake in
    /// a blocking read the accept-wake can't reach. On unix the per-connection read
    /// timeout still lets the worker self-exit (and zeroize) shortly after; on
    /// windows it lingers until process exit. (Also the reap signal reserved for the
    /// deferred new-tab teardown path.)
    TimedOut,
    NotAttempted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclineReason {
    /// The server used keyboard-interactive, so a stored password was withheld.
    KeyboardInteractive,
    /// Channel/token/identity mismatch, an unclassifiable prompt, or a secret the
    /// user withheld at the confirm modal (the password is dropped before arming,
    /// so the listener simply never matches it).
    NoMatch,
}

/// The result of serving one helper connection, accumulated by the listener loop
/// to compute the connect's terminal [`Outcome`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServeResult {
    /// Token mismatch — the connection was dropped without a reply.
    BadToken,
    /// A secret of this kind was released to a token-valid client.
    Served(SecretKind),
    /// Token valid, but nothing was released (no match, single-shot exhausted,
    /// identity mismatch, or an unclassifiable prompt). `kbd_interactive` flags
    /// the `(user@host) …` instance-prefixed form so the loop can report
    /// `Declined { KeyboardInteractive }` when a password was armed.
    NoMatch { kbd_interactive: bool },
}

/// Classify what a (token-valid) `decide` did into a [`ServeResult`], given the
/// prompt and whether a secret was released. Shared by both channel arms.
fn serve_result(prompt: &str, released: bool) -> ServeResult {
    if released {
        ServeResult::Served(ConnectSecrets::last_kind(prompt).unwrap_or(SecretKind::Password))
    } else {
        ServeResult::NoMatch {
            kbd_interactive: prompt.starts_with('('),
        }
    }
}

/// Reduce the per-connection [`ServeResult`]s of one connect into its terminal
/// [`Outcome`]. Token mismatches are ignored (a hostile/foreign client is not
/// this connect's auth flow). `Served` wins; else if no token-valid request ever
/// arrived → `NotAttempted`; else if a password was armed and a
/// keyboard-interactive prompt was seen → `Declined { KeyboardInteractive }`;
/// else `Declined { NoMatch }`. `TimedOut` is never produced here — it is
/// `shutdown`'s detach result and supersedes this worker Outcome when the bounded
/// teardown gives up on a stalled worker.
pub fn outcome_from(results: &[ServeResult], password_armed: bool) -> Outcome {
    let mut last_served: Option<SecretKind> = None;
    let mut saw_kbd_interactive = false;
    let mut any_valid_request = false;
    for r in results {
        match r {
            ServeResult::BadToken => {}
            ServeResult::Served(k) => {
                any_valid_request = true;
                last_served = Some(*k);
            }
            ServeResult::NoMatch { kbd_interactive } => {
                any_valid_request = true;
                saw_kbd_interactive |= *kbd_interactive;
            }
        }
    }
    match last_served {
        Some(kind) => Outcome::Served { kind },
        None if !any_valid_request => Outcome::NotAttempted,
        None if password_armed && saw_kbd_interactive => Outcome::Declined {
            reason: DeclineReason::KeyboardInteractive,
        },
        None => Outcome::Declined {
            reason: DeclineReason::NoMatch,
        },
    }
}

/// How long teardown waits for the worker to acknowledge stop before giving up on
/// the join and detaching it. The normal path (worker parked in `accept`, woken by
/// the self-connect) exits in well under a millisecond; this bound only bites if a
/// peer is parked mid-handshake in a blocking read the self-connect can't reach —
/// in which case we return [`Outcome::TimedOut`] rather than freeze the UI thread.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// Per-connection handshake read budget (unix): a client that connects but never
/// sends its `[token][prompt]` yields a timeout `Err` instead of parking the worker
/// forever. The real helper writes immediately on connect, so this is generous.
#[cfg(unix)]
const HANDSHAKE_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// A live connect-time askpass listener: the serve-loop worker thread, its stop
/// flag, the channel address (to unblock the worker's pending accept at teardown),
/// and a completion signal. The worker owns the [`ConnectSecrets`], so the secrets
/// are zeroized when it returns; teardown joins it but is **bounded** so a peer
/// stalled mid-handshake can never hang the UI thread.
pub struct AskpassListener {
    handle: Option<std::thread::JoinHandle<Outcome>>,
    /// The worker sends its terminal `Outcome` here right before it returns, so
    /// teardown can wait on it with a timeout instead of an unbounded `join`.
    done: std::sync::mpsc::Receiver<Outcome>,
    stop: Arc<AtomicBool>,
    addr: String,
    /// Windows-only: the worker publishes a duplicated handle to ITS OWN THREAD
    /// here so teardown can `CancelSynchronousIo` a stalled blocking pipe read.
    /// The worker clears+closes it (under the lock) right before returning, so a
    /// cancel never races the close. (#12)
    #[cfg(windows)]
    cancel_slot: Arc<std::sync::Mutex<Option<isize>>>,
}

impl AskpassListener {
    /// Stop the loop, unblock its pending accept (via a throwaway self-connect),
    /// and return the connect's terminal [`Outcome`]. On the normal path the worker
    /// has dropped (zeroized) its `ConnectSecrets` by the time this returns; on a
    /// stalled worker it returns [`Outcome::TimedOut`] after a bounded wait rather
    /// than blocking the UI forever (the worker is detached; a unix read timeout
    /// still lets it self-exit and zeroize shortly after).
    pub fn stop_and_join(mut self) -> Outcome {
        self.shutdown().unwrap_or(Outcome::NotAttempted)
    }

    /// Signal stop, wake the blocking accept, and wait (bounded) for the worker to
    /// finish. Idempotent: a second call (e.g. from `Drop`) is a no-op.
    fn shutdown(&mut self) -> Option<Outcome> {
        let handle = self.handle.take()?;
        self.stop.store(true, Ordering::SeqCst);
        // Unblock the worker's blocking accept with a throwaway connection: it
        // wakes, sees the stop flag, and breaks. NON-retrying (unlike the helper's
        // `connect_client`): if a stalled peer squats the channel, retrying here
        // would block the UI thread for seconds before `recv_timeout` could detach.
        wake_connect(&self.addr);
        // Bounded wait: if the worker acknowledges within the grace window, join it
        // (instant) and return its Outcome. Otherwise it is parked in a blocking
        // read the wake couldn't reach — detach rather than freeze the UI.
        match self.done.recv_timeout(SHUTDOWN_GRACE) {
            Ok(outcome) => {
                let _ = handle.join();
                Some(outcome)
            }
            Err(_) => {
                // The worker is parked in a blocking ConnectNamedPipe/ReadFile the
                // self-connect wake couldn't reach. Abort that SYNCHRONOUS I/O on
                // the worker thread so the call errors out, the worker sees `stop`,
                // returns, and zeroizes its ConnectSecrets promptly (unix already
                // self-exits via the read timeout). The slot lock serialises this
                // against the worker closing its thread handle. (#12)
                #[cfg(windows)]
                {
                    use windows_sys::Win32::Foundation::HANDLE;
                    use windows_sys::Win32::System::IO::CancelSynchronousIo;
                    if let Ok(slot) = self.cancel_slot.lock()
                        && let Some(h) = *slot
                    {
                        // SAFETY: while the slot holds Some, the worker has not yet
                        // closed its duplicated thread handle, so `h` is live.
                        unsafe {
                            CancelSynchronousIo(h as HANDLE);
                        }
                    }
                }
                Some(Outcome::TimedOut)
            }
        }
    }
}

impl Drop for AskpassListener {
    fn drop(&mut self) {
        // Guarantee teardown + secret zeroize even if the caller forgot to join.
        let _ = self.shutdown();
    }
}

/// Arm a connect: generate a per-connect token, bind the channel listener
/// (so the helper can connect the instant `ssh` spawns — TOCTOU: this returns
/// only after the listener is bound), spawn the serve-loop worker holding the
/// armed secrets, and return the handle plus the env bundle to apply to the `ssh`
/// `Command`. The caller spawns `ssh` only after this returns `Ok`.
pub fn arm_connect(
    identity: ResolvedIdentity,
    password: Option<Secret>,
    passphrase: Option<Secret>,
) -> io::Result<(
    AskpassListener,
    Vec<(std::ffi::OsString, std::ffi::OsString)>,
)> {
    let mut token = [0u8; TOKEN_LEN];
    let bytes =
        crate::os::vault::random_bytes(TOKEN_LEN).map_err(|e| io::Error::other(e.to_string()))?;
    token.copy_from_slice(&bytes);

    let listener = Listener::bind(token)?;
    let addr = listener.address().to_string();
    // A non-representable (e.g. non-UTF-8) address would arm the env with an empty
    // channel the helper could never reach (and teardown could never wake) — fail
    // closed so the caller degrades to a clean plain connect instead.
    if addr.is_empty() {
        return Err(io::Error::other(
            "askpass channel address is not representable",
        ));
    }
    let env = arm_env(&addr, &token)?;

    let password_armed = password.is_some();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_worker = Arc::clone(&stop);
    let (done_tx, done) = std::sync::mpsc::channel();
    #[cfg(windows)]
    let cancel_slot = Arc::new(std::sync::Mutex::new(None::<isize>));
    #[cfg(windows)]
    let cancel_slot_worker = Arc::clone(&cancel_slot);

    let handle = std::thread::spawn(move || {
        let mut secrets = ConnectSecrets::new(identity, password, passphrase);
        let mut results = Vec::new();
        // (#12, windows) publish a real handle to THIS worker thread so teardown
        // can CancelSynchronousIo its blocking pipe read. GetCurrentThread() is a
        // pseudo-handle valid only in this thread, so duplicate it into a real one.
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
            use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread};
            let mut dup: HANDLE = std::ptr::null_mut();
            // SAFETY: pseudo-handles are valid args; DuplicateHandle writes a real
            // handle into `dup` (or leaves it null and returns 0 on failure).
            let ok = unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    GetCurrentThread(),
                    GetCurrentProcess(),
                    &mut dup,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            };
            if ok != 0 {
                *cancel_slot_worker.lock().unwrap() = Some(dup as isize);
            }
        }
        loop {
            match listener.serve_one(&mut secrets) {
                Ok(r) => {
                    results.push(r);
                    if stop_worker.load(Ordering::SeqCst) {
                        break;
                    }
                }
                Err(_) => {
                    // The stop self-connect surfaces here (EOF on read); a real
                    // short-read/malformed client (or a unix handshake read timeout)
                    // also lands here — per spec we do NOT tear down for that, we
                    // loop back to accept. Stop is the only exit.
                    if stop_worker.load(Ordering::SeqCst) {
                        break;
                    }
                    // Back off so a persistent, immediately-returning accept() error
                    // (e.g. fd exhaustion) can't busy-spin a CPU core.
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        // Clear the slot and close the duplicated thread handle BEFORE returning,
        // so a concurrent teardown cancel can never touch a freed handle. (#12)
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
            if let Some(h) = cancel_slot_worker.lock().unwrap().take() {
                // SAFETY: `h` is the real handle we duplicated above; closed once.
                unsafe {
                    CloseHandle(h as HANDLE);
                }
            }
        }
        // `secrets` drops here -> all Secret material zeroized.
        let outcome = outcome_from(&results, password_armed);
        // Signal completion so a bounded teardown need not block on join.
        let _ = done_tx.send(outcome.clone());
        outcome
    });

    Ok((
        AskpassListener {
            handle: Some(handle),
            done,
            stop,
            addr,
            #[cfg(windows)]
            cancel_slot,
        },
        env,
    ))
}

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
            let suffix =
                crate::os::vault::random_bytes(8).map_err(|e| io::Error::other(e.to_string()))?;
            let hex: String = suffix.iter().map(|b| format!("{b:02x}")).collect();
            let dir = base.join(format!("sshm-askpass-{}-{hex}", std::process::id()));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .recursive(false)
                .create(&dir)?;
            let path = dir.join("sock");
            let inner = UnixListener::bind(&path)?;
            Ok(Listener {
                inner,
                path,
                dir,
                token,
            })
        }

        /// The address string passed to the helper via SSHM_ASKPASS_CHANNEL.
        pub fn address(&self) -> &str {
            self.path.to_str().unwrap_or("")
        }

        /// Accept one client, verify the token, and serve at most one secret.
        /// Returns the [`ServeResult`]: `BadToken` on token mismatch (dropped with
        /// no reply), else `Served`/`NoMatch` for a token-valid client.
        pub fn serve_one(&self, secrets: &mut ConnectSecrets) -> io::Result<ServeResult> {
            let (mut conn, _) = self.inner.accept()?;
            // Bound the handshake: a peer that connects then stalls before sending
            // its request must not park the worker forever (teardown's accept-wake
            // can't reach a thread blocked in read). A timed-out read returns Err,
            // looping the worker back to the stop check.
            conn.set_read_timeout(Some(HANDSHAKE_READ_TIMEOUT))?;
            let (token, prompt) = read_request(&mut conn)?;
            if !ct_eq(&token, &self.token) {
                return Ok(ServeResult::BadToken); // drop without reply
            }
            let secret = secrets.decide(&prompt);
            // Refuse a secret that cannot survive the line channel.
            let to_send = match secret {
                Some(s) if secret_is_one_line(&s) => Some(s),
                _ => None,
            };
            write_reply(&mut conn, to_send.as_deref().map(String::as_str))?;
            Ok(serve_result(&prompt, to_send.is_some()))
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

    /// Teardown side: a single throwaway connect to wake the worker's blocking
    /// accept. Returns immediately (no retry); a failed wake just means the worker
    /// already broke on its own.
    pub fn wake_connect(addr: &str) {
        let _ = UnixStream::connect(addr);
    }
}

#[cfg(unix)]
#[allow(unused_imports)]
pub use chan::{Listener, connect_client, wake_connect};

#[cfg(windows)]
mod chan {
    //! Windows named-pipe channel arm.
    //!
    //! HANDLE ownership (resolves the reuse-loop hazard): the single owned
    //! resource is the raw `HANDLE` from `CreateNamedPipeW`, created ONCE in
    //! `bind` and kept alive for the whole `Listener` lifetime. Keeping the same
    //! instance preserves the `FILE_FLAG_FIRST_PIPE_INSTANCE` squat-protection,
    //! and the handle is closed exactly once — in `Drop` via `CloseHandle`.
    //!
    //! I/O form (B): each `serve_one` wraps a *borrowed* `std::fs::File` around
    //! the persistent `HANDLE` via `File::from_raw_handle`, uses it for the byte
    //! loop (so the safe `read_request`/`write_reply` helpers apply), then
    //! `std::mem::forget`s it so its `Drop` does NOT `CloseHandle` the
    //! persistent instance. Only `Listener::drop` ever calls `CloseHandle`.

    use super::*;
    use std::fs::File;
    use std::os::windows::io::FromRawHandle;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, HANDLE,
        INVALID_HANDLE_VALUE, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers, PIPE_ACCESS_DUPLEX,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// Default per-instance buffer hints (kernel uses these only as hints).
    const PIPE_BUF: u32 = 4096;
    /// `nDefaultTimeOut` for the pipe (ms); only relevant to client timeouts.
    const PIPE_DEFAULT_TIMEOUT_MS: u32 = 5000;

    /// Encode a Rust `&str` as a NUL-terminated UTF-16 wide string for Win32.
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Resolve the current process user's SID and format it as an SDDL string
    /// of the form `D:(A;;GA;;;<SID>)` — a DACL granting GENERIC_ALL to ONLY the
    /// current user. The default named-pipe SD would grant Everyone + anonymous;
    /// this locks the pipe to the user that created it.
    fn current_user_sddl() -> io::Result<Vec<u16>> {
        // SAFETY: all FFI below uses freshly-owned locals; every acquired
        // resource (the process token, the SID string) is released before
        // return on every path.
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            // Helper so any error after the token is opened still closes it.
            let result = current_user_sddl_with_token(token);
            CloseHandle(token);
            result
        }
    }

    /// SID resolution given an already-opened process token (closed by caller).
    ///
    /// SAFETY: `token` must be a valid, live token handle with TOKEN_QUERY.
    unsafe fn current_user_sddl_with_token(token: HANDLE) -> io::Result<Vec<u16>> {
        unsafe {
            // First call sizes the TokenUser buffer.
            let mut needed: u32 = 0;
            let ok = GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
            // Expected to fail with ERROR_INSUFFICIENT_BUFFER and set `needed`.
            if ok != 0 || needed == 0 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
                    return Err(err);
                }
            }
            // Over-aligned backing store: the TOKEN_USER struct is read out of
            // this buffer, which the kernel fills with a SID trailing the struct.
            let mut buf = vec![0u8; needed as usize];
            if GetTokenInformation(
                token,
                TokenUser,
                buf.as_mut_ptr() as *mut core::ffi::c_void,
                needed,
                &mut needed,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            // The SID pointer points back into `buf`; keep `buf` alive until the
            // SID has been stringified.
            let token_user = &*(buf.as_ptr() as *const TOKEN_USER);
            let sid = token_user.User.Sid;
            if sid.is_null() {
                return Err(io::Error::other("token user SID is null"));
            }

            let mut sid_str: windows_sys::core::PWSTR = std::ptr::null_mut();
            if ConvertSidToStringSidW(sid, &mut sid_str) == 0 {
                return Err(io::Error::last_os_error());
            }
            // Copy the SID text into an owned String, then free the API buffer.
            let sid_owned = pwstr_to_string(sid_str);
            LocalFree(sid_str as _);

            let sddl = format!("D:(A;;GA;;;{sid_owned})");
            Ok(wide(&sddl))
        }
    }

    /// Copy a NUL-terminated wide string allocated by Win32 into a Rust String.
    ///
    /// SAFETY: `p` must be a valid, NUL-terminated UTF-16 pointer.
    unsafe fn pwstr_to_string(p: windows_sys::core::PWSTR) -> String {
        unsafe {
            let mut len = 0usize;
            while *p.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(p, len);
            String::from_utf16_lossy(slice)
        }
    }

    pub struct Listener {
        handle: HANDLE,           // the single owned, squat-protected instance
        name: Vec<u16>,           // \\.\pipe\... as a wide string for clients
        addr: String,             // the same name as UTF-8 for SSHM_ASKPASS_CHANNEL
        token: Token,             // per-connect auth token (constant-time compared)
        sd: PSECURITY_DESCRIPTOR, // owned; LocalFree on drop
    }

    // SAFETY: the HANDLE is owned solely by this Listener and only touched on
    // the listener thread; the security descriptor is likewise owned here.
    unsafe impl Send for Listener {}

    impl Listener {
        pub fn bind(token: Token) -> io::Result<Listener> {
            // 1. Random, unpredictable name with >=128 bits of entropy.
            let rnd =
                crate::os::vault::random_bytes(16).map_err(|e| io::Error::other(e.to_string()))?;
            let hex: String = rnd.iter().map(|b| format!("{b:02x}")).collect();
            let addr = format!(r"\\.\pipe\sshm-askpass-{}-{hex}", std::process::id());
            let name = wide(&addr);

            // 2. SID-only DACL via SDDL -> PSECURITY_DESCRIPTOR.
            let sddl = current_user_sddl()?;
            let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            // SAFETY: `sddl` is a valid NUL-terminated wide string; `sd` is an
            // out-param. On success the kernel allocates an SD freed in Drop.
            let sd_ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut sd,
                    std::ptr::null_mut(),
                )
            };
            if sd_ok == 0 {
                return Err(io::Error::last_os_error());
            }

            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd,
                bInheritHandle: 0,
            };

            // 3. Create the single squat-protected instance.
            // SAFETY: `name` is a valid NUL-terminated wide string and `sa`
            // outlives the call. On failure the SD is freed before returning.
            let handle = unsafe {
                CreateNamedPipeW(
                    name.as_ptr(),
                    PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                    // Reject remote (SMB) clients at the kernel level so the secret
                    // channel is local-only, matching the unix socket's confinement.
                    PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    1, // nMaxInstances: exactly one
                    PIPE_BUF,
                    PIPE_BUF,
                    PIPE_DEFAULT_TIMEOUT_MS,
                    &sa,
                )
            };
            if handle == INVALID_HANDLE_VALUE || handle.is_null() {
                let err = io::Error::last_os_error();
                // SAFETY: `sd` is the SD we just allocated and have not freed.
                unsafe {
                    LocalFree(sd as _);
                }
                return Err(err);
            }

            Ok(Listener {
                handle,
                name,
                addr,
                token,
                sd,
            })
        }

        /// The address string passed to the helper via SSHM_ASKPASS_CHANNEL.
        pub fn address(&self) -> &str {
            &self.addr
        }

        /// Accept one client over the persistent instance, verify the token, and
        /// serve at most one secret. Returns the [`ServeResult`]: `BadToken` on
        /// token mismatch, else `Served`/`NoMatch` for a token-valid client. The
        /// same handle is reused for the next client after DisconnectNamedPipe.
        pub fn serve_one(&self, secrets: &mut ConnectSecrets) -> io::Result<ServeResult> {
            // Wait for a client to connect to the persistent instance.
            // SAFETY: `self.handle` is the live, owned pipe instance.
            let connected = unsafe { ConnectNamedPipe(self.handle, std::ptr::null_mut()) };
            if connected == 0 {
                let err = io::Error::last_os_error();
                // A client that connected between CreateNamedPipe and
                // ConnectNamedPipe surfaces as ERROR_PIPE_CONNECTED — success.
                if err.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
                    return Err(err);
                }
            }

            // Borrow the persistent HANDLE as a File for the byte loop WITHOUT
            // taking ownership: mem::forget below stops its Drop from closing it.
            // SAFETY: the handle is valid and exclusively owned by self; we never
            // let this File's Drop run, so CloseHandle happens only in Listener::drop.
            let mut io = unsafe { File::from_raw_handle(self.handle as _) };

            // Run the connection through the safe wire protocol. We must NOT
            // early-return while `io` is live without forgetting it, or its Drop
            // would CloseHandle the persistent instance — so capture the result.
            let result = (|| -> io::Result<ServeResult> {
                let (token, prompt) = read_request(&mut io)?;
                if !ct_eq(&token, &self.token) {
                    return Ok(ServeResult::BadToken); // drop without reply
                }
                let secret = secrets.decide(&prompt);
                // Refuse a secret that cannot survive the line channel.
                let to_send = match secret {
                    Some(s) if secret_is_one_line(&s) => Some(s),
                    _ => None,
                };
                write_reply(&mut io, to_send.as_deref().map(String::as_str))?;
                Ok(serve_result(&prompt, to_send.is_some()))
            })();

            // Relinquish the borrowed File without closing the handle.
            std::mem::forget(io);

            // If we replied (Served or a zero-length NoMatch), block until the
            // client has drained the reply before disconnecting — DisconnectNamedPipe
            // discards un-read pipe data, so a premature disconnect makes the
            // client's read see ERROR_PIPE_NOT_CONNECTED. A BadToken wrote nothing.
            // SAFETY: same persistent handle.
            if matches!(
                result,
                Ok(ServeResult::Served(_) | ServeResult::NoMatch { .. })
            ) {
                unsafe {
                    FlushFileBuffers(self.handle);
                }
            }

            // Ready the instance for the next client regardless of outcome.
            // SAFETY: same persistent handle.
            unsafe {
                DisconnectNamedPipe(self.handle);
            }

            result
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            // SAFETY: handle/sd are owned solely by self and closed once here.
            unsafe {
                if !self.handle.is_null() && self.handle != INVALID_HANDLE_VALUE {
                    DisconnectNamedPipe(self.handle);
                    CloseHandle(self.handle);
                }
                if !self.sd.is_null() {
                    LocalFree(self.sd as _);
                }
            }
            // `name` is just a Vec<u16>; nothing to free.
            let _ = &self.name;
        }
    }

    /// Helper side: connect to the listener's named pipe. A byte-mode pipe path
    /// opens as an ordinary file handle. ERROR_PIPE_BUSY (all instances busy) is
    /// retried briefly via WaitNamedPipeW so the test connects reliably.
    pub fn connect_client(addr: &str) -> io::Result<std::fs::File> {
        use std::fs::OpenOptions;
        let name = wide(addr);
        // A few short waits cover the window between one client disconnecting and
        // the listener's DisconnectNamedPipe/ConnectNamedPipe readying the instance.
        for _ in 0..50 {
            match OpenOptions::new().read(true).write(true).open(addr) {
                Ok(f) => return Ok(f),
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                    // SAFETY: `name` is a valid NUL-terminated wide string.
                    unsafe {
                        WaitNamedPipeW(name.as_ptr(), 200);
                    }
                }
                Err(e) => return Err(e),
            }
        }
        // Final attempt; surface whatever error it yields.
        OpenOptions::new().read(true).write(true).open(addr)
    }

    /// Teardown side: a single non-retrying open to wake the worker's blocking
    /// `ConnectNamedPipe`. Unlike [`connect_client`], it does NOT retry on
    /// `ERROR_PIPE_BUSY`: if a stalled peer is squatting the single pipe instance,
    /// retrying would block the calling (UI) thread for seconds — and a failed wake
    /// is harmless because the bounded `recv_timeout` then detaches the worker.
    pub fn wake_connect(addr: &str) {
        use std::fs::OpenOptions;
        let _ = OpenOptions::new().read(true).write(true).open(addr);
    }
}

#[cfg(windows)]
#[allow(unused_imports)]
pub use chan::{Listener, connect_client, wake_connect};

#[cfg(test)]
mod tests {
    use super::{
        Classified, IdentityTokens, TOKEN_LEN, classify, ct_eq, expand_identity_path,
        secret_is_one_line,
    };

    #[test]
    fn module_compiles() {
        assert_eq!(super::TOKEN_LEN, 32);
    }

    #[test]
    fn parse_openssh_version_gate() {
        use super::parse_openssh_atleast as p;
        assert_eq!(p("OpenSSH_9.6p1, OpenSSL", 8, 5), Some(true));
        assert_eq!(p("OpenSSH_8.5p1", 8, 5), Some(true));
        assert_eq!(p("OpenSSH_8.4p1, OpenSSL", 8, 5), Some(false));
        assert_eq!(p("OpenSSH_for_Windows_9.5", 8, 5), Some(true));
        assert_eq!(p("garbage", 8, 5), None);
    }

    #[test]
    fn read_reply_rejects_non_utf8_without_panicking() {
        // length-prefixed (u32 LE = 2) then invalid UTF-8 bytes.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&[0xff, 0xfe]);
        let mut cur = std::io::Cursor::new(buf);
        let err = super::read_reply(&mut cur).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

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

    #[test]
    fn strips_verbatim_disk_and_unc_prefixes() {
        use super::strip_verbatim_prefix;
        assert_eq!(
            strip_verbatim_prefix(r"\\?\C:\Users\me\sshm.exe"),
            r"C:\Users\me\sshm.exe"
        );
        assert_eq!(
            strip_verbatim_prefix(r"\\?\UNC\server\share\sshm.exe"),
            r"\\server\share\sshm.exe"
        );
        // ordinary paths pass through unchanged
        assert_eq!(
            strip_verbatim_prefix(r"C:\Users\me\sshm.exe"),
            r"C:\Users\me\sshm.exe"
        );
        assert_eq!(
            strip_verbatim_prefix("/usr/local/bin/sshm"),
            "/usr/local/bin/sshm"
        );
    }

    #[test]
    fn token_hex_roundtrips_and_arm_env_carries_force_and_token() {
        use super::{CHANNEL_ENV, TOKEN_ENV, TOKEN_LEN, arm_env, parse_token_hex, token_hex};
        let token = [0xABu8; TOKEN_LEN];
        let hex = token_hex(&token);
        assert_eq!(hex.len(), TOKEN_LEN * 2);
        assert_eq!(parse_token_hex(&hex), Some(token));

        let env = arm_env(r"\\.\pipe\sshm-askpass-x", &token).unwrap();
        let get = |k: &str| {
            env.iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.to_string_lossy().into_owned())
        };
        assert_eq!(get("SSH_ASKPASS_REQUIRE").as_deref(), Some("force"));
        assert_eq!(
            get(CHANNEL_ENV).as_deref(),
            Some(r"\\.\pipe\sshm-askpass-x")
        );
        assert_eq!(get(TOKEN_ENV).as_deref(), Some(hex.as_str()));
        // SSH_ASKPASS is the running exe (current_exe in tests) — present, non-empty.
        assert!(get("SSH_ASKPASS").is_some_and(|p| !p.is_empty()));
        // Secrets never appear in the bundle — only the token + address do.
        assert_eq!(env.len(), 4);
    }

    #[test]
    fn outcome_from_reduces_serve_results() {
        use super::{DeclineReason, Outcome, ServeResult, outcome_from};
        use crate::os::vault::SecretKind;

        // a served secret wins, regardless of order or earlier no-matches.
        assert_eq!(
            outcome_from(
                &[
                    ServeResult::NoMatch {
                        kbd_interactive: true
                    },
                    ServeResult::Served(SecretKind::Passphrase),
                ],
                true,
            ),
            Outcome::Served {
                kind: SecretKind::Passphrase
            }
        );
        // no token-valid request at all -> NotAttempted (a bad token is ignored).
        assert_eq!(outcome_from(&[], false), Outcome::NotAttempted);
        assert_eq!(
            outcome_from(&[ServeResult::BadToken], false),
            Outcome::NotAttempted
        );
        // password armed + a keyboard-interactive prompt seen, nothing served.
        assert_eq!(
            outcome_from(
                &[ServeResult::NoMatch {
                    kbd_interactive: true
                }],
                true
            ),
            Outcome::Declined {
                reason: DeclineReason::KeyboardInteractive
            }
        );
        // a plain no-match with a password armed -> NoMatch (not kbd-interactive).
        assert_eq!(
            outcome_from(
                &[ServeResult::NoMatch {
                    kbd_interactive: false
                }],
                true
            ),
            Outcome::Declined {
                reason: DeclineReason::NoMatch
            }
        );
        // kbd-interactive seen but NO password armed -> plain NoMatch.
        assert_eq!(
            outcome_from(
                &[ServeResult::NoMatch {
                    kbd_interactive: true
                }],
                false
            ),
            Outcome::Declined {
                reason: DeclineReason::NoMatch
            }
        );
    }

    #[test]
    fn arm_connect_serves_password_then_reports_served_outcome() {
        use super::{
            CHANNEL_ENV, Outcome, ResolvedIdentity, TOKEN_ENV, arm_connect, connect_client,
            parse_token_hex, read_reply, write_request,
        };
        use crate::os::vault::{Secret, SecretKind};

        let id = ResolvedIdentity {
            user: "deploy".into(),
            host: "web1".into(),
            host_key_alias: None,
            identity_paths: vec![],
        };
        // Arm with a password; the listener binds + the worker starts before this
        // returns (so a client can connect immediately — the real TOCTOU order).
        let (listener, env) = arm_connect(id, Some(Secret::from("hunter2")), None).unwrap();
        let get = |k: &str| {
            env.iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.to_string_lossy().into_owned())
        };
        let addr = get(CHANNEL_ENV).unwrap();
        let token = parse_token_hex(&get(TOKEN_ENV).unwrap()).unwrap();

        // A helper presents the matching prompt and gets the password.
        let mut c = connect_client(&addr).unwrap();
        write_request(&mut c, &token, "deploy@web1's password: ").unwrap();
        assert_eq!(
            read_reply(&mut c).unwrap().as_deref().map(String::as_str),
            Some("hunter2")
        );
        drop(c);

        // Teardown unblocks the loop, joins, and reports Served{Password}; the
        // worker has dropped (zeroized) the ConnectSecrets by the time join returns.
        let outcome = listener.stop_and_join();
        assert_eq!(
            outcome,
            Outcome::Served {
                kind: SecretKind::Password
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn teardown_is_bounded_when_a_peer_stalls_mid_handshake() {
        use super::{CHANNEL_ENV, Outcome, ResolvedIdentity, arm_connect, connect_client};
        use crate::os::vault::Secret;
        use std::time::{Duration, Instant};

        let id = ResolvedIdentity {
            user: "u".into(),
            host: "h".into(),
            host_key_alias: None,
            identity_paths: vec![],
        };
        let (listener, env) = arm_connect(id, None, Some(Secret::from("pp"))).unwrap();
        let addr = env
            .iter()
            .find(|(n, _)| n == CHANNEL_ENV)
            .map(|(_, v)| v.to_string_lossy().into_owned())
            .unwrap();

        // A peer connects but sends nothing and HOLDS the connection open, parking
        // the worker in read_request (past accept, where the teardown wake can't
        // reach). Give the worker a moment to accept and enter the read.
        let held = connect_client(&addr).unwrap();
        std::thread::sleep(Duration::from_millis(150));

        // Teardown must still return promptly (~SHUTDOWN_GRACE) with TimedOut,
        // detaching the stalled worker rather than blocking the caller forever.
        let start = Instant::now();
        let outcome = listener.stop_and_join();
        let elapsed = start.elapsed();
        assert_eq!(outcome, Outcome::TimedOut);
        assert!(
            elapsed < Duration::from_secs(4),
            "teardown must be bounded near SHUTDOWN_GRACE, took {elapsed:?}"
        );
        drop(held);
    }

    #[test]
    fn classify_passphrase_prompt() {
        let p = "Enter passphrase for key '/home/u/.ssh/id_ed25519': ";
        assert_eq!(
            classify(p),
            Classified::Passphrase {
                key_path: "/home/u/.ssh/id_ed25519".to_string()
            }
        );
    }

    #[test]
    fn classify_password_prompt() {
        assert_eq!(
            classify("deploy@web1's password: "),
            Classified::Password {
                user: "deploy".to_string(),
                host: "web1".to_string()
            }
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
        assert_eq!(
            expand_identity_path("~/.ssh/id_ed25519", &toks()).as_deref(),
            Some("/home/u/.ssh/id_ed25519")
        );
        assert_eq!(
            expand_identity_path("%d/.ssh/k", &toks()).as_deref(),
            Some("/home/u/.ssh/k")
        );
    }

    #[test]
    fn expand_percent_h_k_n_r() {
        assert_eq!(
            expand_identity_path("~/.ssh/id_%h", &toks()).as_deref(),
            Some("/home/u/.ssh/id_web1.example.com")
        );
        assert_eq!(
            expand_identity_path("~/.ssh/id_%k", &toks()).as_deref(),
            Some("/home/u/.ssh/id_web1ka")
        );
        assert_eq!(
            expand_identity_path("~/.ssh/id_%r", &toks()).as_deref(),
            Some("/home/u/.ssh/id_deploy")
        );
    }

    #[test]
    fn unknown_token_is_fail_safe_none() {
        assert_eq!(expand_identity_path("~/.ssh/id_%C", &toks()), None);
        assert_eq!(expand_identity_path("~/.ssh/id_%z", &toks()), None);
    }

    #[test]
    fn identity_from_resolved_config_expands_paths() {
        use super::{OsTokens, resolved_identity};
        use crate::os::resolve::ResolvedConfig;
        let rc = ResolvedConfig {
            hostname: Some("web1".into()),
            user: Some("deploy".into()),
            port: Some("22".into()),
            host_key_alias: None,
            identity_files: vec!["~/.ssh/id_ed25519".into(), "~/.ssh/id_%C".into()],
            ..Default::default()
        };
        let os = OsTokens {
            home: "/home/u".into(),
            local_user: "u".into(),
            uid: "1000".into(),
            localhost: "box".into(),
        };
        let id = resolved_identity(&rc, "web1", &os);
        assert_eq!(id.user, "deploy");
        assert_eq!(id.host, "web1");
        // the expandable path is kept; the %C path is dropped (fail-safe).
        assert_eq!(
            id.identity_paths,
            vec!["/home/u/.ssh/id_ed25519".to_string()]
        );
    }

    #[test]
    fn percent_percent_is_literal() {
        assert_eq!(
            expand_identity_path("~/100%%done", &toks()).as_deref(),
            Some("/home/u/100%done")
        );
    }

    use super::{ConnectSecrets, ResolvedIdentity};
    use crate::os::vault::Secret;

    fn ident() -> ResolvedIdentity {
        ResolvedIdentity {
            user: "deploy".into(),
            host: "web1".into(), // already ASCII-lowercased by ssh -G
            host_key_alias: None,
            identity_paths: vec!["/home/u/.ssh/id_ed25519".into()],
        }
    }

    #[test]
    fn releases_matching_password_once() {
        let mut cs = ConnectSecrets::new(ident(), Some(Secret::from("hunter2")), None);
        // First matching password prompt -> served.
        let r = cs.decide("deploy@web1's password: ");
        assert_eq!(r.as_deref().map(String::as_str), Some("hunter2"));
        // Second password prompt on the same connect -> nothing (single-shot).
        assert!(cs.decide("deploy@web1's password: ").is_none());
    }

    #[test]
    fn withholds_password_for_wrong_host_or_user() {
        let mut cs = ConnectSecrets::new(ident(), Some(Secret::from("hunter2")), None);
        assert!(cs.decide("deploy@evil's password: ").is_none());
        assert!(cs.decide("root@web1's password: ").is_none());
        assert!(
            cs.decide("(deploy@web1) deploy@web1's password: ")
                .is_none()
        );
    }

    #[test]
    fn passphrase_per_path_single_shot() {
        let mut cs = ConnectSecrets::new(
            ResolvedIdentity {
                identity_paths: vec!["/home/u/.ssh/id_a".into(), "/home/u/.ssh/id_b".into()],
                ..ident()
            },
            None,
            Some(Secret::from("pp")),
        );
        // key A: served, then refused on retry of the SAME path.
        assert_eq!(
            cs.decide("Enter passphrase for key '/home/u/.ssh/id_a': ")
                .as_deref()
                .map(String::as_str),
            Some("pp")
        );
        assert!(
            cs.decide("Enter passphrase for key '/home/u/.ssh/id_a': ")
                .is_none()
        );
        // key B: a DIFFERENT expected path is still served (multi-IdentityFile fallback).
        assert_eq!(
            cs.decide("Enter passphrase for key '/home/u/.ssh/id_b': ")
                .as_deref()
                .map(String::as_str),
            Some("pp")
        );
        // an unexpected path is never served.
        assert!(
            cs.decide("Enter passphrase for key '/home/u/.ssh/id_x': ")
                .is_none()
        );
    }

    #[test]
    fn no_secret_armed_returns_none() {
        let mut cs = ConnectSecrets::new(ident(), None, None);
        assert!(cs.decide("deploy@web1's password: ").is_none());
        assert!(
            cs.decide("Enter passphrase for key '/home/u/.ssh/id_ed25519': ")
                .is_none()
        );
    }

    use super::{read_reply, read_request, write_reply, write_request};
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
        assert_eq!(
            read_reply(&mut cur).unwrap().as_deref().map(String::as_str),
            Some("hunter2")
        );

        // A zero-length (no-match) reply.
        let mut buf = Vec::new();
        write_reply(&mut buf, None).unwrap();
        let mut cur = Cursor::new(buf);
        assert!(read_reply(&mut cur).unwrap().is_none());
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

    #[cfg(unix)]
    #[test]
    fn unix_channel_serves_one_request() {
        use super::{
            ConnectSecrets, Listener, ResolvedIdentity, TOKEN_LEN, connect_client, read_reply,
            write_request,
        };
        use crate::os::vault::Secret;
        use std::thread;

        let token = [3u8; TOKEN_LEN];
        let listener = Listener::bind(token).unwrap();
        let addr = listener.address().to_string();

        // Listener side: serve exactly one request from a background thread.
        let handle = thread::spawn(move || {
            let ident = ResolvedIdentity {
                user: "deploy".into(),
                host: "web1".into(),
                host_key_alias: None,
                identity_paths: vec![],
            };
            let mut secrets = ConnectSecrets::new(ident, Some(Secret::from("hunter2")), None);
            listener.serve_one(&mut secrets).unwrap();
        });

        // Client side: present the token + prompt, read the reply.
        let mut conn = connect_client(&addr).unwrap();
        write_request(&mut conn, &token, "deploy@web1's password: ").unwrap();
        let reply = read_reply(&mut conn).unwrap();
        assert_eq!(reply.as_deref().map(String::as_str), Some("hunter2"));

        handle.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn unix_channel_rejects_bad_token() {
        use super::{
            ConnectSecrets, Listener, ResolvedIdentity, TOKEN_LEN, connect_client, read_reply,
            write_request,
        };
        use crate::os::vault::Secret;
        use std::thread;

        let token = [3u8; TOKEN_LEN];
        let listener = Listener::bind(token).unwrap();
        let addr = listener.address().to_string();
        let handle = thread::spawn(move || {
            let ident = ResolvedIdentity {
                user: "deploy".into(),
                host: "web1".into(),
                host_key_alias: None,
                identity_paths: vec![],
            };
            let mut secrets = ConnectSecrets::new(ident, Some(Secret::from("hunter2")), None);
            // serve_one returns BadToken when the token mismatched.
            let served = listener.serve_one(&mut secrets).unwrap();
            assert_eq!(served, super::ServeResult::BadToken);
        });
        let mut conn = connect_client(&addr).unwrap();
        write_request(&mut conn, &[0u8; TOKEN_LEN], "deploy@web1's password: ").unwrap();
        // No reply on bad token -> read_reply sees EOF/error.
        let _ = read_reply(&mut conn);
        handle.join().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_channel_serves_two_sequential_requests() {
        use super::{
            ConnectSecrets, Listener, ResolvedIdentity, TOKEN_LEN, connect_client, read_reply,
            write_request,
        };
        use crate::os::vault::Secret;
        use std::thread;

        let token = [5u8; TOKEN_LEN];
        let listener = Listener::bind(token).unwrap();
        let addr = listener.address().to_string();

        let handle = thread::spawn(move || {
            let ident = ResolvedIdentity {
                user: "deploy".into(),
                host: "web1".into(),
                host_key_alias: None,
                identity_paths: vec!["C:/Users/u/.ssh/id_ed25519".into()],
            };
            let mut secrets = ConnectSecrets::new(
                ident,
                Some(Secret::from("hunter2")),
                Some(Secret::from("pp")),
            );
            // Serve two sequential clients over the SAME persistent instance.
            listener.serve_one(&mut secrets).unwrap();
            listener.serve_one(&mut secrets).unwrap();
        });

        let mut c1 = connect_client(&addr).unwrap();
        write_request(&mut c1, &token, "deploy@web1's password: ").unwrap();
        assert_eq!(
            read_reply(&mut c1).unwrap().as_deref().map(String::as_str),
            Some("hunter2")
        );
        drop(c1);

        let mut c2 = connect_client(&addr).unwrap();
        write_request(
            &mut c2,
            &token,
            "Enter passphrase for key 'C:/Users/u/.ssh/id_ed25519': ",
        )
        .unwrap();
        assert_eq!(
            read_reply(&mut c2).unwrap().as_deref().map(String::as_str),
            Some("pp")
        );

        handle.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn helper_prints_served_secret() {
        use super::{
            CHANNEL_ENV, ConnectSecrets, Listener, ResolvedIdentity, TOKEN_ENV, TOKEN_LEN,
            run_helper,
        };
        use crate::os::vault::Secret;
        use std::thread;

        let token = [4u8; TOKEN_LEN];
        let listener = Listener::bind(token).unwrap();
        let addr = listener.address().to_string();
        let handle = thread::spawn(move || {
            let ident = ResolvedIdentity {
                user: "deploy".into(),
                host: "web1".into(),
                host_key_alias: None,
                identity_paths: vec![],
            };
            let mut secrets = ConnectSecrets::new(ident, Some(Secret::from("hunter2")), None);
            listener.serve_one(&mut secrets).unwrap();
        });

        // run_helper reads CHANNEL/TOKEN from a passed-in Env abstraction so the
        // test need not mutate process-global env.
        let hextok: String = token.iter().map(|b| format!("{b:02x}")).collect();
        let out = run_helper(Some("deploy@web1's password: ".to_string()), |k| match k {
            CHANNEL_ENV => Some(addr.clone()),
            TOKEN_ENV => Some(hextok.clone()),
            _ => None,
        });
        assert_eq!(out.unwrap().as_slice(), b"hunter2\n");
        handle.join().unwrap();
    }

    #[test]
    fn helper_without_channel_env_is_none() {
        use super::run_helper;
        // No SSHM_ASKPASS_CHANNEL -> not in askpass mode -> None (caller falls through).
        let out = run_helper(Some("x".into()), |_| None);
        assert!(out.is_none());
    }
}
