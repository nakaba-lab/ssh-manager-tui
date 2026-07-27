//! O2/O5 — generic confirm modal, and O3 — per-host action menu.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::{App, ConfirmAction, SftpDirection};

use super::theme;
use super::widgets::{centered, modal_block};

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

pub fn draw(f: &mut Frame, action: ConfirmAction, area: Rect) {
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
        ConfirmAction::Quit => ("Quit", "Quit SSH Manager?".to_string(), false),
    };

    let modal = centered(56, 7, area);
    f.render_widget(Clear, modal);
    let block = modal_block(title, danger);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {message}"),
            Style::default().fg(theme::TEXT),
        )),
        Line::from(""),
        Line::from(vec![
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
        ]),
    ];
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
}
