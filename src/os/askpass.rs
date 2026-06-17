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

/// Length of the per-connect authentication token (256 bits).
#[allow(dead_code)]
pub const TOKEN_LEN: usize = 32;

#[cfg(test)]
mod tests {
    #[test]
    fn module_compiles() {
        assert_eq!(super::TOKEN_LEN, 32);
    }
}
