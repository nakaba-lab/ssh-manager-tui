//! Inline SFTP transfer modal: pick a direction plus a local and a remote path,
//! then run a one-shot `sftp -b` transfer inline. Pure rendering — all state
//! lives in [`crate::app::SftpForm`]; submitting is handled in `update.rs`.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::App;

use super::theme;
use super::widgets::{centered_pct, input_line, modal_block, section_header};

pub fn draw_transfer(f: &mut Frame, app: &App, area: Rect) {
    let form = &app.sftp_form;
    let alias = app
        .hosts
        .get(form.host)
        .map(|h| h.alias())
        .unwrap_or_default();

    let modal = centered_pct(78, 56, area);
    f.render_widget(Clear, modal);
    let block = modal_block(&format!("SFTP transfer · {alias}"), false);

    let label_style = |focused: bool| {
        if focused {
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::DIM)
        }
    };
    let marker = |focused: bool| if focused { "▸ " } else { "  " };

    let mut lines: Vec<Line> = Vec::new();

    // Field 0 — direction toggle.
    lines.push(section_header("Direction"));
    let dir_focused = form.field == 0;
    lines.push(Line::from(vec![
        Span::styled(marker(dir_focused), label_style(dir_focused)),
        Span::styled(format!("{:<8}", "Mode"), label_style(dir_focused)),
        Span::styled(
            form.direction.label(),
            Style::default()
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // Fields 1 & 2 — local / remote path text inputs.
    lines.push(Line::from(""));
    lines.push(section_header("Paths"));
    for (field, label, value, cursor) in [
        (1usize, "Local", &form.local, form.local_cursor),
        (2usize, "Remote", &form.remote, form.remote_cursor),
    ] {
        let focused = form.field == field;
        let mut spans = vec![
            Span::styled(marker(focused), label_style(focused)),
            Span::styled(format!("{label:<8}"), label_style(focused)),
        ];
        spans.extend(input_line(value, cursor, focused).spans);
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Runs `sftp -b` inline — the TUI pauses and sftp shows its progress.",
        Style::default().fg(theme::FAINT),
    )));
    lines.push(Line::from(Span::styled(
        "  A stored passphrase auto-fills; a password is typed at the prompt.",
        Style::default().fg(theme::FAINT),
    )));
    lines.push(Line::from(Span::styled(
        "  Tab move · Space/←→ direction · ^S transfer · Esc cancel",
        Style::default().fg(theme::DIM),
    )));

    f.render_widget(Paragraph::new(Text::from(lines)).block(block), modal);
}
