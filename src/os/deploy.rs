//! #48 — public-key deployment to a remote `authorized_keys` (the `ssh-copy-id`
//! equivalent Windows never shipped). This module is the **pure half**: it turns a
//! `.pub` file's text into a validated remote `sh` snippet and gives the child's
//! exit code a meaning. It performs no I/O and spawns nothing — the actual round
//! trip runs through the existing inline path in `update.rs` (`suspend_tui` →
//! `run_inline` → `restore_tui`), so this half stays headless-testable and the
//! layering rule holds (see CLAUDE.md layering).
//!
//! Security: a `.pub` file lives under `~/.ssh`, is attacker-influenceable, and its
//! text ends up inside a shell command that runs on the remote host (CWE-88). The
//! body (`<algo> <blob>`) is validated against an allowlist and a failure REFUSES
//! the deployment; a comment that fails the same allowlist is dropped rather than
//! escaped. Nothing here ever escapes — it rejects, exactly like `tokens.rs` and
//! `sftp_quote`.

/// Exit status the remote snippet uses to report "this key was already there".
/// Distinct from 0 (appended) so the outcome survives inline execution, where the
/// child owns stdout and the parent can only observe the exit code.
pub const ALREADY_PRESENT_EXIT: i32 = 3;

/// Longest comment we are willing to splice into the remote command line.
pub const MAX_COMMENT_LEN: usize = 128;

/// Why a `.pub` line cannot be deployed at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployError {
    /// Not `<algo> <blob>[ comment]` — no key to deploy.
    NotAPublicKey,
    /// The algorithm or base64 body carries characters we refuse to send.
    UnsafeBody,
}

/// A validated, ready-to-run deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployPlan {
    /// `<algo> <blob>` — the identity used for the duplicate check.
    pub body: String,
    /// The exact line appended remotely (the body, plus the comment when it survived).
    pub line: String,
    /// True when the `.pub` carried a comment that failed validation and was dropped,
    /// so the confirm modal can say what will actually land on the remote.
    pub comment_dropped: bool,
    /// The remote POSIX `sh` snippet, passed to `ssh` as a single argument.
    pub snippet: String,
}

/// What the child's exit code meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployOutcome {
    /// The key was appended.
    Added,
    /// The key was already in `authorized_keys`; nothing was written.
    AlreadyPresent,
    /// `ssh` itself failed (connection, auth, host-key) — code 255.
    SshFailed,
    /// The remote command failed, e.g. a non-POSIX remote shell (Windows OpenSSH).
    RemoteFailed(i32),
    /// No exit code at all (killed by a signal, or the launch itself failed).
    Interrupted,
}

/// Characters we are willing to send inside the key body (`<algo> <blob>`):
/// base64 plus the punctuation OpenSSH uses in algorithm names (`sk-…@openssh.com`,
/// `ecdsa-sha2-nistp256`). Anything else — quotes, backslashes, backticks, `$`,
/// `;` — means the body is not a key we can safely quote, so we refuse it.
fn body_char_ok(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '@' | '.' | '+' | '/' | '=' | '_')
}

/// Characters allowed in the (optional) comment. Same idea as the body, plus the
/// separators a human comment carries. A comment that fails this is dropped, not
/// escaped — losing the annotation is survivable, mis-escaping is not.
fn comment_char_ok(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, ' ' | '-' | '@' | '.' | '+' | '/' | '=' | '_' | ':' | ',')
}

/// Validate a `.pub` file's text and build the remote snippet.
pub fn plan(pub_text: &str) -> Result<DeployPlan, DeployError> {
    // Only ever the FIRST line: a `.pub` read from a CRLF file, or one with a
    // second key appended, must not fold that second key into what we deploy.
    let first = pub_text.lines().next().unwrap_or("").trim();
    let mut tokens = first.split_whitespace();
    let (Some(algo), Some(blob)) = (tokens.next(), tokens.next()) else {
        return Err(DeployError::NotAPublicKey);
    };
    if !algo.chars().all(body_char_ok) || !blob.chars().all(body_char_ok) {
        return Err(DeployError::UnsafeBody);
    }
    let body = format!("{algo} {blob}");

    // The remainder is the comment. Joining on a single space also normalizes any
    // tabs/runs of spaces, so what the modal shows is what lands on the remote.
    let comment = tokens.collect::<Vec<_>>().join(" ");
    let comment_kept = !comment.is_empty()
        && comment.len() <= MAX_COMMENT_LEN
        && comment.chars().all(comment_char_ok);
    let comment_dropped = !comment.is_empty() && !comment_kept;
    let line = if comment_kept {
        format!("{body} {comment}")
    } else {
        body.clone()
    };

    // One round trip. The duplicate check greps the comment-free body, so a remote
    // entry whose comment differs still counts as present. The `tail -c1` probe
    // adds the missing newline first — appending to a file whose last line has no
    // newline would glue our key onto it and break BOTH entries (a lockout).
    //
    // Quoting: the grep pattern is single-quoted, the appended line double-quoted.
    // Both are safe because the allowlists above exclude `'`, `"`, `$`, `` ` ``
    // and `\`; the two styles also keep the duplicate-check pattern visibly
    // distinct from the line being written.
    let snippet = format!(
        "umask 077; mkdir -p ~/.ssh; touch ~/.ssh/authorized_keys; \
         if grep -qF '{body}' ~/.ssh/authorized_keys; then exit {ALREADY_PRESENT_EXIT}; fi; \
         if [ -s ~/.ssh/authorized_keys ] && [ -n \"$(tail -c1 ~/.ssh/authorized_keys)\" ]; \
         then printf '\\n' >> ~/.ssh/authorized_keys; fi; \
         printf '%s\\n' \"{line}\" >> ~/.ssh/authorized_keys"
    );

    Ok(DeployPlan {
        body,
        line,
        comment_dropped,
        snippet,
    })
}

/// Give the inline child's exit code its meaning.
pub fn classify_exit(code: Option<i32>) -> DeployOutcome {
    match code {
        Some(0) => DeployOutcome::Added,
        Some(ALREADY_PRESENT_EXIT) => DeployOutcome::AlreadyPresent,
        // 255 is ssh's own "could not connect / authenticate".
        Some(255) => DeployOutcome::SshFailed,
        Some(other) => DeployOutcome::RemoteFailed(other),
        None => DeployOutcome::Interrupted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALGO: &str = "ssh-ed25519";
    const BLOB: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIB1LtRcXaGCS5MFvHi1cJcHjuFF5jJyUpTXBrpEEXAMPLE";

    fn body() -> String {
        format!("{ALGO} {BLOB}")
    }

    // --- AC1/AC3: the snippet itself ---

    #[test]
    fn plan_builds_a_guarded_single_round_trip_snippet() {
        // given: an ordinary public key line
        let text = format!("{ALGO} {BLOB} me@laptop");
        // when
        let p = plan(&text).expect("a clean key must plan");
        // then: the classic guarded append, in ONE remote command
        assert!(p.snippet.contains("umask 077"), "{}", p.snippet);
        assert!(p.snippet.contains("mkdir -p ~/.ssh"), "{}", p.snippet);
        assert!(
            p.snippet.contains("~/.ssh/authorized_keys"),
            "{}",
            p.snippet
        );
        assert_eq!(p.line, format!("{} me@laptop", body()));
        assert!(!p.comment_dropped);
    }

    #[test]
    fn plan_greps_for_the_comment_free_body() {
        // The duplicate check must key off `<algo> <blob>` only, so a remote entry
        // whose comment differs (or which carries an options prefix) still counts as
        // present and we never append the same key twice.
        let p = plan(&format!("{ALGO} {BLOB} me@laptop")).unwrap();
        assert_eq!(p.body, body());
        assert!(
            p.snippet.contains(&format!("grep -qF '{}'", body())),
            "duplicate check must grep the body, not the full line: {}",
            p.snippet
        );
        assert!(
            !p.snippet.contains("grep -qF 'ssh-ed25519 ") || !p.snippet.contains("me@laptop'"),
            "the grep pattern must not carry the comment: {}",
            p.snippet
        );
    }

    #[test]
    fn plan_reports_already_present_through_a_distinct_exit_code() {
        // Inline execution gives the child our stdout, so "already there" has to come
        // back as an exit code rather than printed output.
        let p = plan(&body()).unwrap();
        assert!(
            p.snippet.contains(&format!("exit {ALREADY_PRESENT_EXIT}")),
            "the duplicate branch must exit {ALREADY_PRESENT_EXIT}: {}",
            p.snippet
        );
    }

    #[test]
    fn plan_guards_against_a_file_without_a_trailing_newline() {
        // Appending to an authorized_keys whose last line has no newline would glue
        // our key onto it and silently break BOTH entries — a lockout risk.
        let p = plan(&body()).unwrap();
        assert!(
            p.snippet.contains("tail -c1"),
            "must probe the last byte before appending: {}",
            p.snippet
        );
    }

    // --- AC6: injection defence ---

    #[test]
    fn plan_drops_a_comment_carrying_a_single_quote() {
        // The attack: a comment that closes our quote and appends a command.
        let text = format!("{ALGO} {BLOB} evil'; rm -rf ~; echo '");
        let p = plan(&text).expect("a hostile comment must not sink the whole deploy");
        assert!(p.comment_dropped, "the comment must be reported as dropped");
        assert_eq!(p.line, body(), "only the body may be appended");
        assert!(
            !p.snippet.contains("rm -rf"),
            "the injected command must not reach the snippet: {}",
            p.snippet
        );
        assert!(
            !p.line.contains('\'') && !p.body.contains('\''),
            "nothing spliced into the quoted strings may contain a quote"
        );
    }

    #[test]
    fn plan_drops_a_comment_carrying_control_characters() {
        let text = format!("{ALGO} {BLOB} me@laptop\u{7}\u{1b}[2J");
        let p = plan(&text).expect("control chars in a comment are dropped, not fatal");
        assert!(p.comment_dropped);
        assert_eq!(p.line, body());
        assert!(
            !p.snippet.contains('\u{1b}'),
            "no escape sequences may reach the remote command"
        );
    }

    #[test]
    fn plan_drops_an_overlong_comment() {
        let text = format!("{ALGO} {BLOB} {}", "a".repeat(MAX_COMMENT_LEN + 1));
        let p = plan(&text).unwrap();
        assert!(
            p.comment_dropped,
            "an unbounded comment must not be spliced"
        );
        assert_eq!(p.line, body());
    }

    #[test]
    fn plan_keeps_an_ordinary_user_at_host_comment() {
        // The common case must survive: the remote authorized_keys stays auditable.
        let p = plan(&format!("{ALGO} {BLOB} alice@work-laptop.example")).unwrap();
        assert!(!p.comment_dropped);
        assert_eq!(p.line, format!("{} alice@work-laptop.example", body()));
        assert!(p.snippet.contains("alice@work-laptop.example"));
    }

    #[test]
    fn plan_keeps_a_multi_word_comment_of_safe_characters() {
        let p = plan(&format!("{ALGO} {BLOB} alice work laptop")).unwrap();
        assert!(!p.comment_dropped);
        assert_eq!(p.line, format!("{} alice work laptop", body()));
    }

    #[test]
    fn plan_refuses_a_body_carrying_a_quote() {
        // A hostile BODY is not recoverable by dropping anything — refuse outright.
        let text = format!("{ALGO} AAAA'; rm -rf ~; echo '");
        assert_eq!(plan(&text), Err(DeployError::UnsafeBody));
    }

    #[test]
    fn plan_refuses_a_body_carrying_a_backslash_or_backtick() {
        assert_eq!(
            plan(&format!("{ALGO} AAAA`id`")),
            Err(DeployError::UnsafeBody)
        );
        assert_eq!(
            plan(&format!("ssh-\\ed25519 {BLOB}")),
            Err(DeployError::UnsafeBody)
        );
    }

    // --- structural rejection ---

    #[test]
    fn plan_refuses_a_line_without_a_blob() {
        assert_eq!(plan(ALGO), Err(DeployError::NotAPublicKey));
    }

    #[test]
    fn plan_refuses_empty_or_blank_text() {
        assert_eq!(plan(""), Err(DeployError::NotAPublicKey));
        assert_eq!(plan("   \n\n"), Err(DeployError::NotAPublicKey));
    }

    #[test]
    fn plan_reads_only_the_first_line() {
        // A `.pub` read from a CRLF file (or one with trailing junk) must not fold a
        // second line into the comment and ship it to the remote.
        let text = format!("{ALGO} {BLOB} me@laptop\r\nssh-rsa OTHERKEY other@host\n");
        let p = plan(&text).unwrap();
        assert_eq!(p.line, format!("{} me@laptop", body()));
        assert!(
            !p.snippet.contains("OTHERKEY"),
            "a second line must never be deployed: {}",
            p.snippet
        );
    }

    #[test]
    fn plan_accepts_the_sk_and_ecdsa_algorithm_spellings() {
        // `@` and `.` are legal in algorithm names; `-` and digits in the curve ones.
        assert!(plan(&format!("sk-ssh-ed25519@openssh.com {BLOB}")).is_ok());
        assert!(plan(&format!("ecdsa-sha2-nistp256 {BLOB}")).is_ok());
    }

    // --- AC2/AC3/AC7: exit-code meanings ---

    #[test]
    fn classify_exit_maps_zero_to_added() {
        assert_eq!(classify_exit(Some(0)), DeployOutcome::Added);
    }

    #[test]
    fn classify_exit_maps_the_sentinel_to_already_present() {
        assert_eq!(
            classify_exit(Some(ALREADY_PRESENT_EXIT)),
            DeployOutcome::AlreadyPresent
        );
    }

    #[test]
    fn classify_exit_maps_255_to_an_ssh_failure() {
        // 255 is ssh's own "I could not connect / authenticate" code.
        assert_eq!(classify_exit(Some(255)), DeployOutcome::SshFailed);
    }

    #[test]
    fn classify_exit_maps_other_codes_to_a_remote_failure() {
        // A non-POSIX remote shell (Windows OpenSSH default cmd.exe) lands here.
        assert_eq!(classify_exit(Some(1)), DeployOutcome::RemoteFailed(1));
        assert_eq!(classify_exit(Some(127)), DeployOutcome::RemoteFailed(127));
        assert_eq!(classify_exit(Some(9009)), DeployOutcome::RemoteFailed(9009));
    }

    #[test]
    fn classify_exit_maps_a_missing_code_to_interrupted() {
        assert_eq!(classify_exit(None), DeployOutcome::Interrupted);
    }
}
