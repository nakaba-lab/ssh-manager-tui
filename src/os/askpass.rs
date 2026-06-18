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

// Phase 1 builds this module standalone; its items are wired into the connect
// path in Phase 3. Until then the binary (non-test) build sees them as unused.
// TODO(phase3): remove this once the connect path references this module.
#![allow(dead_code)]

use std::collections::HashSet;
use std::io::{self, Read, Write};

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
#[allow(unused_imports)]
pub use chan::{Listener, connect_client};

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
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_TYPE_BYTE, PIPE_WAIT,
        WaitNamedPipeW,
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
                    PIPE_TYPE_BYTE | PIPE_WAIT,
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
        /// serve at most one secret. Returns Ok(true) for a token-valid client
        /// (secret or deliberate no-match served), Ok(false) on token mismatch.
        /// The same handle is reused for the next client after DisconnectNamedPipe.
        pub fn serve_one(&self, secrets: &mut ConnectSecrets) -> io::Result<bool> {
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
            let result = (|| -> io::Result<bool> {
                let (token, prompt) = read_request(&mut io)?;
                if !ct_eq(&token, &self.token) {
                    return Ok(false); // drop without reply
                }
                let secret = secrets.decide(&prompt);
                // Refuse a secret that cannot survive the line channel.
                let to_send = match secret {
                    Some(s) if secret_is_one_line(&s) => Some(s),
                    _ => None,
                };
                write_reply(&mut io, to_send.as_deref())?;
                Ok(true)
            })();

            // Relinquish the borrowed File without closing the handle.
            std::mem::forget(io);

            // If we replied, block until the client has drained the reply before
            // disconnecting — DisconnectNamedPipe discards un-read pipe data, so
            // a premature disconnect makes the client's read see ERROR_PIPE_NOT_CONNECTED.
            // SAFETY: same persistent handle.
            if matches!(result, Ok(true)) {
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
}

#[cfg(windows)]
#[allow(unused_imports)]
pub use chan::{Listener, connect_client};

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
                identity_paths: vec!["/home/u/.ssh/id_a".into(), "/home/u/.ssh/id_b".into()],
                ..ident()
            },
            None,
            Some(Secret::from("pp")),
        );
        // key A: served, then refused on retry of the SAME path.
        assert_eq!(
            cs.decide("Enter passphrase for key '/home/u/.ssh/id_a': ")
                .as_deref(),
            Some("pp")
        );
        assert_eq!(
            cs.decide("Enter passphrase for key '/home/u/.ssh/id_a': "),
            None
        );
        // key B: a DIFFERENT expected path is still served (multi-IdentityFile fallback).
        assert_eq!(
            cs.decide("Enter passphrase for key '/home/u/.ssh/id_b': ")
                .as_deref(),
            Some("pp")
        );
        // an unexpected path is never served.
        assert_eq!(
            cs.decide("Enter passphrase for key '/home/u/.ssh/id_x': "),
            None
        );
    }

    #[test]
    fn no_secret_armed_returns_none() {
        let mut cs = ConnectSecrets::new(ident(), None, None);
        assert_eq!(cs.decide("deploy@web1's password: "), None);
        assert_eq!(
            cs.decide("Enter passphrase for key '/home/u/.ssh/id_ed25519': "),
            None
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
        assert_eq!(reply.as_deref(), Some("hunter2"));

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
        assert_eq!(read_reply(&mut c1).unwrap().as_deref(), Some("hunter2"));
        drop(c1);

        let mut c2 = connect_client(&addr).unwrap();
        write_request(
            &mut c2,
            &token,
            "Enter passphrase for key 'C:/Users/u/.ssh/id_ed25519': ",
        )
        .unwrap();
        assert_eq!(read_reply(&mut c2).unwrap().as_deref(), Some("pp"));

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
}
