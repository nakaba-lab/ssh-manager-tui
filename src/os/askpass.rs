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

/// Length of the per-connect authentication token (256 bits).
pub const TOKEN_LEN: usize = 32;

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

#[cfg(test)]
mod tests {
    use super::{Classified, IdentityTokens, classify, expand_identity_path};

    #[test]
    fn module_compiles() {
        assert_eq!(super::TOKEN_LEN, 32);
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
}
