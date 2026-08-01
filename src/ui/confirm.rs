//! O2/O5 — generic confirm modal, and O3 — per-host action menu.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::{App, ConfirmAction, SftpDirection};

use super::theme;
use super::widgets::{centered, kv_line, modal_block};

/// The labels shown in the per-host action menu, in selection order.
pub const ACTION_LABELS: [&str; 9] = [
    "Connect (inline)",
    "Connect (new tab)",
    "SFTP (inline)",
    "SFTP transfer…",
    "Connect (overrides)…",
    "Copy ssh command",
    "Scan host key",
    "Edit host",
    "Delete host",
];

/// Symbolic indices into [`ACTION_LABELS`], shared with the dispatch in
/// `update::handle_action_menu` so the label order and the action mapping can
/// never drift apart (the bare integers were a silent-reorder hazard — moving a
/// row would otherwise send e.g. "Delete host" to a connect branch).
pub mod action_idx {
    pub const CONNECT_INLINE: usize = 0;
    pub const CONNECT_NEW_TAB: usize = 1;
    pub const SFTP_INLINE: usize = 2;
    pub const SFTP_TRANSFER: usize = 3;
    pub const CONNECT_OVERRIDES: usize = 4;
    pub const COPY_COMMAND: usize = 5;
    pub const SCAN_HOST_KEY: usize = 6;
    pub const EDIT: usize = 7;
    pub const DELETE: usize = 8;
}

/// Width of the deploy modal. The key/value rows use [`kv_line`]'s 16-column label
/// gutter, so a full SHA256 fingerprint does not fit and is elided (see [`elide`]).
const DEPLOY_MODAL_WIDTH: u16 = 60;

/// Shorten `s` to `max` columns, keeping the head and the last few characters so
/// a fingerprint stays comparable at a glance.
fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(4)).collect();
    let tail: String = s
        .chars()
        .skip(s.chars().count().saturating_sub(3))
        .collect();
    format!("{head}…{tail}")
}

/// Body of the deploy confirmation (#48). Deployment rewrites the REMOTE host's
/// credentials and undoing it means editing `authorized_keys` by hand, so the
/// modal names the host, the key, its fingerprint and the comment that will land
/// — mistaking one key for another must not survive this screen. A comment the
/// allowlist rejected is shown as dropped rather than silently omitted, so the
/// screen matches what the remote actually receives.
fn deploy_lines(app: &App) -> Vec<Line<'static>> {
    let host = app
        .key_host_ctx
        .and_then(|i| app.hosts.get(i))
        .map(|h| match &h.host_name {
            Some(n) => format!("{} ({n})", h.alias()),
            None => h.alias().to_string(),
        })
        .unwrap_or_else(|| "?".to_string());
    let (name, fingerprint) = app
        .keys_state
        .selected()
        .and_then(|i| app.keys.get(i))
        .map(|k| {
            let p = k.pub_path.as_ref().unwrap_or(&k.path);
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| p.display().to_string());
            (name, k.fingerprint.clone())
        })
        .unwrap_or_else(|| ("?".into(), "?".into()));
    let comment = match app.pending_deploy.as_ref() {
        Some(p) if p.comment_dropped => "— (dropped: unsafe characters)".to_string(),
        Some(p) => p
            .line
            .strip_prefix(&p.body)
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .unwrap_or("—")
            .to_string(),
        None => "—".to_string(),
    };

    // 16 columns go to kv_line's label gutter, 2 more to the modal border.
    let value_w = DEPLOY_MODAL_WIDTH as usize - 18;
    vec![
        Line::from(Span::styled(
            "  Append this key to the remote ~/.ssh/authorized_keys?",
            Style::default().fg(theme::TEXT),
        )),
        Line::from(""),
        kv_line("Host", elide(&host, value_w)),
        kv_line("Key", elide(&name, value_w)),
        kv_line("Fingerprint", elide(&fingerprint, value_w)),
        kv_line("Comment", elide(&comment, value_w)),
    ]
}

pub fn draw(f: &mut Frame, app: &App, action: ConfirmAction, area: Rect) {
    // The deploy confirmation has its own key/value layout (see `deploy_lines`),
    // so it is built here rather than as a one-line message.
    if matches!(action, ConfirmAction::DeployKey) {
        draw_modal(
            f,
            "Deploy public key",
            deploy_lines(app),
            false,
            DEPLOY_MODAL_WIDTH,
            area,
        );
        return;
    }
    let (title, message, danger) = match action {
        ConfirmAction::DeleteHost(_) => (
            "Delete host",
            "Remove this Host block from ~/.ssh/config?".to_string(),
            true,
        ),
        ConfirmAction::RemoveKey(_) => (
            "Delete key",
            "Delete this key pair (private + public) from disk?".to_string(),
            true,
        ),
        ConfirmAction::RemoveKnownHost { .. } => (
            "Remove known_host",
            "Remove this entry from known_hosts?".to_string(),
            true,
        ),
        ConfirmAction::DiscardEdit => (
            "Discard changes",
            "Discard unsaved changes to this host?".to_string(),
            true,
        ),
        ConfirmAction::DeleteVaultEntry(_) => (
            "Delete secret",
            "Remove this stored secret from the vault?".to_string(),
            true,
        ),
        ConfirmAction::OverwriteTransfer { direction, name } => {
            let side = match direction {
                SftpDirection::Get => "local",
                SftpDirection::Put => "remote",
            };
            // Truncate a long (possibly server-supplied) name to fit the modal width.
            let shown: String = name.chars().take(36).collect();
            let ellipsis = if name.chars().count() > 36 { "…" } else { "" };
            (
                "Overwrite file",
                format!("A {side} file '{shown}{ellipsis}' already exists. Overwrite it?"),
                true,
            )
        }
        // Handled above: it needs `app` for the host/key detail lines.
        ConfirmAction::DeployKey => unreachable!("DeployKey is drawn by draw_modal"),
        ConfirmAction::Quit => ("Quit", "Quit SSH Manager?".to_string(), false),
    };

    let body = vec![Line::from(Span::styled(
        format!("  {message}"),
        Style::default().fg(theme::TEXT),
    ))];
    draw_modal(f, title, body, danger, 56, area);
}

/// Render a confirm modal: the body lines above the y/n hint row. Shared so the
/// one-line confirmations and the key/value deploy modal (#48) can never drift in
/// framing, colours, or key hints.
fn draw_modal(
    f: &mut Frame,
    title: &str,
    body: Vec<Line<'static>>,
    danger: bool,
    width: u16,
    area: Rect,
) {
    let modal = centered(width, 6 + body.len() as u16, area);
    f.render_widget(Clear, modal);
    let block = modal_block(title, danger);

    let mut lines = vec![Line::from("")];
    lines.extend(body);
    lines.push(Line::from(""));
    lines.extend([Line::from(vec![
        Span::raw("   "),
        Span::styled(
            " y / Enter ",
            Style::default()
                .fg(theme::BG)
                .bg(if danger { theme::DOWN } else { theme::UP })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  confirm    ", Style::default().fg(theme::DIM)),
        Span::styled(
            " n / Esc ",
            Style::default()
                .fg(theme::BG)
                .bg(theme::DIM)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  cancel", Style::default().fg(theme::DIM)),
    ])]);
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), modal);
}

pub fn draw_action_menu(f: &mut Frame, app: &App, host_idx: usize, area: Rect) {
    let alias = app
        .hosts
        .get(host_idx)
        .map(|h| h.alias().to_string())
        .unwrap_or_default();

    let modal = centered(40, (ACTION_LABELS.len() as u16) + 4, area);
    f.render_widget(Clear, modal);
    let block = modal_block(&alias, false);

    let mut lines = vec![Line::from("")];
    for (i, label) in ACTION_LABELS.iter().enumerate() {
        let selected = i == app.menu_sel;
        let (marker, style) = if selected {
            (
                "▎ ",
                Style::default()
                    .bg(theme::SEL_BG)
                    .fg(theme::TEXT)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            ("  ", Style::default().fg(theme::DIM))
        };
        lines.push(Line::from(Span::styled(format!("{marker}{label}"), style)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  j/k move · Enter run · Esc close",
        Style::default().fg(theme::FAINT),
    )));
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), modal);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_idx_aligns_with_labels() {
        // The dispatch keys off these indices; assert each names its own label so
        // a future reorder of ACTION_LABELS without updating action_idx (or the
        // match arms) fails here rather than silently misrouting a menu action.
        assert_eq!(ACTION_LABELS.len(), 9);
        assert_eq!(
            ACTION_LABELS[action_idx::CONNECT_INLINE],
            "Connect (inline)"
        );
        assert_eq!(
            ACTION_LABELS[action_idx::CONNECT_NEW_TAB],
            "Connect (new tab)"
        );
        assert_eq!(ACTION_LABELS[action_idx::SFTP_INLINE], "SFTP (inline)");
        assert_eq!(ACTION_LABELS[action_idx::SFTP_TRANSFER], "SFTP transfer…");
        assert_eq!(
            ACTION_LABELS[action_idx::CONNECT_OVERRIDES],
            "Connect (overrides)…"
        );
        assert_eq!(ACTION_LABELS[action_idx::COPY_COMMAND], "Copy ssh command");
        assert_eq!(ACTION_LABELS[action_idx::SCAN_HOST_KEY], "Scan host key");
        assert_eq!(ACTION_LABELS[action_idx::EDIT], "Edit host");
        assert_eq!(ACTION_LABELS[action_idx::DELETE], "Delete host");
    }

    #[test]
    fn action_labels_include_scan_host_key() {
        // given — the per-host action menu (#46 adds the explicit scan entry)
        // when / then — the launch path for the host-key pre-scan exists
        assert!(
            ACTION_LABELS.contains(&"Scan host key"),
            "ActionMenu must offer 'Scan host key' (#46), got {ACTION_LABELS:?}"
        );
    }

    // --- #48 deploy confirmation: the last human gate before a remote write ---

    /// Flatten a rendered line back to plain text for assertions.
    fn text_of(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn app_with_key(pub_text: &str) -> (App, std::path::PathBuf) {
        use std::io::Write;
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("sshm-cfm-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config");
        std::fs::File::create(&cfg)
            .unwrap()
            .write_all(b"Host web-prod\n    HostName 10.0.0.1\n")
            .unwrap();
        let pub_path = dir.join("id_ed25519.pub");
        std::fs::File::create(&pub_path)
            .unwrap()
            .write_all(pub_text.as_bytes())
            .unwrap();

        let mut app = App::new(cfg).unwrap();
        app.keys = vec![crate::os::keys::KeyInfo {
            path: dir.join("id_ed25519"),
            pub_path: Some(pub_path),
            bits: 256,
            fingerprint: "SHA256:EXAMPLEfingerprintEXAMPLEfingerprintEXAMPLE".into(),
            comment: "me@laptop".into(),
            key_type: "ED25519".into(),
            has_private: true,
            pair: crate::os::keys::PairStatus::Matched,
        }];
        app.keys_state.select(Some(0));
        app.key_host_ctx = Some(0);
        app.pending_deploy = Some(crate::os::deploy::plan(pub_text).expect("fixture key plans"));
        (app, dir)
    }

    const CLEAN_PUB: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIB1LtRcXaGCS5MFvHi1cJcHjuFF5jJyUpTXBrpEEXAMPLE me@laptop";

    #[test]
    fn deploy_lines_name_the_host_key_fingerprint_and_comment() {
        // AC1: the modal is the last gate before rewriting a REMOTE authorized_keys,
        // so all four identifying facts must be on screen.
        let (app, dir) = app_with_key(CLEAN_PUB);
        let rendered: Vec<String> = deploy_lines(&app).iter().map(text_of).collect();
        let joined = rendered.join("\n");
        assert!(joined.contains("web-prod"), "{joined}");
        assert!(joined.contains("10.0.0.1"), "host name is shown: {joined}");
        assert!(joined.contains("id_ed25519.pub"), "{joined}");
        assert!(joined.contains("SHA256:"), "{joined}");
        assert!(
            joined.contains("me@laptop"),
            "the comment that lands: {joined}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deploy_lines_say_so_when_the_comment_is_dropped() {
        // The screen must match what the remote receives — a comment the allowlist
        // rejected is reported, never silently omitted.
        let (app, dir) = app_with_key(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIB1LtRcXaGCS5MFvHi1cJcHjuFF5jJyUpTXBrpEEXAMPLE evil'; id; echo '",
        );
        let joined: String = deploy_lines(&app)
            .iter()
            .map(text_of)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("dropped"), "{joined}");
        assert!(
            !joined.contains("id;"),
            "the hostile comment must not be echoed: {joined}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deploy_lines_fit_the_modal_width() {
        // kv_line pads the label to 16 columns; a value that overflows would push
        // the modal border off and corrupt the frame.
        let (app, dir) = app_with_key(CLEAN_PUB);
        for line in deploy_lines(&app) {
            let w = text_of(&line).chars().count();
            assert!(
                w <= DEPLOY_MODAL_WIDTH as usize - 2,
                "line overflows the modal: {w} cols in {:?}",
                text_of(&line)
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn elide_keeps_the_head_and_tail() {
        assert_eq!(elide("short", 10), "short");
        assert_eq!(elide("abcdefghij", 10), "abcdefghij");
        // Longer than the budget: head + … + last three, within the budget.
        let out = elide("abcdefghijklmno", 10);
        assert_eq!(out.chars().count(), 10);
        assert!(out.starts_with("abcdef"), "{out}");
        assert!(out.ends_with("mno"), "{out}");
    }
}
