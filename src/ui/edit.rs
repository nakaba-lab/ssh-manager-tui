//! S2 — add/edit host form. Single-valued fields render inline; multi-valued
//! fields (IdentityFile, forwards, extras) render as indented row lists.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::app::{App, FormMode, Screen, form_idx};

use super::theme;
use super::widgets::{input_line, panel, section_header};

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    // The diff-preview modal overlays this form (its base screen is the Edit
    // form), so while it is up `app.screen` is `DiffPreview` — resolve the title
    // from the base screen instead, or an in-progress edit would read "Add host".
    let editing_existing = matches!(app.screen, Screen::Edit { editing: Some(_) })
        || (app.screen == Screen::DiffPreview
            && matches!(app.prev_screen, Some(Screen::Edit { editing: Some(_) })));
    let title = if editing_existing {
        "Edit host"
    } else {
        "Add host"
    };
    let block = panel(title, true);

    let form = &app.form;
    let editing = form.mode == FormMode::Editing;
    let mut lines: Vec<Line> = Vec::new();
    let mut focus_line: usize = 0;

    // The section sub-heading that precedes a given field index (if any).
    // Boundaries follow the contiguous field groups; the field model is untouched.
    let section_for = |idx: usize| -> Option<&'static str> {
        if idx == form_idx::HOST {
            Some("Connection")
        } else if idx == form_idx::IDENTITY {
            Some("Identity & routing")
        } else if idx == form_idx::LOCAL_FWD {
            Some("Forwarding")
        } else if idx == form_idx::EXTRAS {
            Some("Advanced")
        } else if idx == form_idx::TAGS {
            Some("Metadata")
        } else {
            None
        }
    };

    for (idx, field) in form.fields.iter().enumerate() {
        if let Some(title) = section_for(idx) {
            if idx != 0 {
                lines.push(Line::from(""));
            }
            lines.push(section_header(title));
        }

        let is_focused = idx == form.focused;
        if is_focused {
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

        if !field.multi {
            let mut spans = vec![
                Span::styled(marker, label_style),
                Span::styled(format!("{:<26}", field.label), label_style),
            ];
            let active = is_focused && editing;
            spans.extend(input_line(&field.value, field.cursor, active).spans);
            lines.push(Line::from(spans));
        } else {
            lines.push(Line::from(vec![
                Span::styled(marker, label_style),
                Span::styled(format!("{}:", field.label), label_style),
            ]));
            if field.rows.is_empty() {
                lines.push(Line::from(Span::styled(
                    "      (none — press 'a' to add)",
                    Style::default().fg(theme::FAINT),
                )));
            }
            for (ri, row) in field.rows.iter().enumerate() {
                let row_focused = is_focused && ri == field.row_sel;
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

        // Inline validation error for this field.
        if let Some((_, msg)) = form.errors.iter().find(|(i, _)| *i == idx) {
            lines.push(Line::from(Span::styled(
                format!("      ⚠ {msg}"),
                Style::default().fg(theme::DOWN),
            )));
        }
    }

    let inner_h = area.height.saturating_sub(2) as usize;
    let scroll = if focus_line + 2 > inner_h {
        (focus_line + 2 - inner_h) as u16
    } else {
        0
    };

    let para = Paragraph::new(Text::from(lines))
        .block(block)
        .scroll((scroll, 0));
    f.render_widget(para, area);
}
