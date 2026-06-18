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
}
