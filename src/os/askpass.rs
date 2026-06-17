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

#[cfg(test)]
mod tests {
    use super::{Classified, classify};

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
}
