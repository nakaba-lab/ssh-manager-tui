//! O2/O5 — generic confirm modal, and O3 — per-host action menu.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::{App, ConfirmAction};

use super::theme;
use super::widgets::{centered, modal_block};

/// The labels shown in the per-host action menu, in selection order.
pub const ACTION_LABELS: [&str; 6] = [
    "Connect (inline)",
    "Connect (new tab)",
    "Connect (overrides)…",
    "Copy ssh command",
    "Edit host",
    "Delete host",
];

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
