//! Connect-time override modal: a session-only form that edits a
//! [`crate::os::connect::ConnectOverrides`] for one connection without touching
//! `~/.ssh/config`. Blank fields inherit the host's saved value (shown as a dim
//! hint); only typed fields override. Rendered as a centered modal over the list.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::{App, FormMode, override_idx};
use crate::config::model::HostView;

use super::theme;
use super::widgets::{centered_pct, input_line, modal_block, section_header};

/// The host's saved value for a single-valued override field, shown as the
/// "inherits …" hint when the override is left blank.
fn inherited(host: &HostView, idx: usize) -> Option<String> {
    match idx {
        override_idx::USER => host.user.clone(),
        override_idx::PORT => host.port.clone(),
        override_idx::IDENTITY => host.identity_files.first().cloned(),
        override_idx::PROXYJUMP => host.proxy_jump.clone(),
        _ => None,
    }
}

pub fn draw(f: &mut Frame, app: &App, host: usize, area: Rect) {
    let of = &app.override_form;
    let form = &of.form;
    let editing = form.mode == FormMode::Editing;
    let host_view = app.hosts.get(host);
    let alias = host_view.map(|h| h.alias()).unwrap_or_default();

    let modal = centered_pct(76, 86, area);
    f.render_widget(Clear, modal);
    let block = modal_block(&format!("Connect override · {alias}"), false);

    let section_for = |idx: usize| -> Option<&'static str> {
        match idx {
            override_idx::USER => Some("Connection (blank = inherit from config)"),
            override_idx::LOCAL_FWD => Some("Forwarding"),
            _ => None,
        }
    };

    let mut lines: Vec<Line> = Vec::new();
    let mut focus_line: usize = 0;
    for (idx, field) in form.fields.iter().enumerate() {
        if let Some(title) = section_for(idx) {
            if idx != 0 {
                lines.push(Line::from(""));
            }
            lines.push(section_header(title));
        }

        let is_focused = idx == form.focused;
        // For single fields (and the verbose toggle) the header line IS the focus
        // target; multi fields set focus_line at the selected row below.
        if is_focused && !field.multi {
            focus_line = lines.len();
        }
        let label_style = if is_focused {
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::DIM)
        };
        let marker = if is_focused { "▸ " } else { "  " };

        // Verbose is a boolean toggle, not a text field.
        if idx == override_idx::VERBOSE {
            let box_ = if of.verbose { "[x]" } else { "[ ]" };
            lines.push(Line::from(vec![
                Span::styled(marker, label_style),
                Span::styled(format!("{:<26}", field.label), label_style),
                Span::styled(
                    box_,
                    Style::default()
                        .fg(if of.verbose { theme::UP } else { theme::DIM })
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            continue;
        }

        if !field.multi {
            let mut spans = vec![
                Span::styled(marker, label_style),
                Span::styled(format!("{:<26}", field.label), label_style),
            ];
            let active = is_focused && editing;
            if field.value.is_empty() && !active {
                // Show what the connection inherits when nothing is typed.
                let hint = host_view
                    .and_then(|h| inherited(h, idx))
                    .map(|v| format!("inherits: {v}"))
                    .unwrap_or_else(|| "—".to_string());
                spans.push(Span::styled(hint, Style::default().fg(theme::FAINT)));
            } else {
                spans.extend(input_line(&field.value, field.cursor, active).spans);
            }
            lines.push(Line::from(spans));
        } else {
            lines.push(Line::from(vec![
                Span::styled(marker, label_style),
                Span::styled(format!("{}:", field.label), label_style),
            ]));
            if field.rows.is_empty() {
                if is_focused {
                    focus_line = lines.len();
                }
                lines.push(Line::from(Span::styled(
                    "      (none — press 'a' to add)",
                    Style::default().fg(theme::FAINT),
                )));
            }
            for (ri, row) in field.rows.iter().enumerate() {
                let row_focused = is_focused && ri == field.row_sel;
                if row_focused {
                    focus_line = lines.len();
                }
                let active = row_focused && editing;
                let bullet_style = if row_focused {
                    Style::default()
                        .fg(theme::ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::FAINT)
                };
                let mut spans = vec![Span::styled(
                    if row_focused { "    › " } else { "      " },
                    bullet_style,
                )];
                if active {
                    spans.extend(input_line(row, field.cursor, true).spans);
                } else {
                    spans.push(Span::styled(row.clone(), Style::default().fg(theme::TEXT)));
                }
                lines.push(Line::from(spans));
            }
        }

        if let Some((_, msg)) = form.errors.iter().find(|(i, _)| *i == idx) {
            lines.push(Line::from(Span::styled(
                format!("      ⚠ {msg}"),
                Style::default().fg(theme::DOWN),
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Session-only — never written to ~/.ssh/config.",
        Style::default().fg(theme::FAINT),
    )));
    lines.push(Line::from(Span::styled(
        "  Tab move · Enter edit/pick · Space toggle",
        Style::default().fg(theme::DIM),
    )));
    lines.push(Line::from(Span::styled(
        "  ^O connect · ^T new-tab · ^Y copy · Esc cancel",
        Style::default().fg(theme::DIM),
    )));

    // Scroll so the focused field stays visible — the modal can overflow a short
    // terminal (e.g. 80×24), which would otherwise clip the focused field or the
    // ^O/^T/^Y action chords below the fold. Mirrors the edit form (ui/edit.rs).
    let inner_h = modal.height.saturating_sub(2) as usize;
    let scroll = if focus_line + 2 > inner_h {
        (focus_line + 2 - inner_h) as u16
    } else {
        0
    };
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .scroll((scroll, 0)),
        modal,
    );
}
