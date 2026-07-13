//! Before-save diff preview modal (issue #42).
//!
//! Pure rendering of the `Vec<DiffLine>` computed in `update::open_diff_preview`
//! (current on-disk file → what the save will write). Added lines are green,
//! removed lines red, context dim; the body scrolls via [`App::diff_scroll`].
//! This module never mutates domain state — the diff is prepared before the
//! modal opens, so `draw` only paints it.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::App;
use crate::config::diff::{self, DiffLine};

use super::theme;
use super::widgets::{centered_pct, footer_hints, modal_block};

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let modal = centered_pct(84, 84, area);
    f.render_widget(Clear, modal);

    let block = modal_block("Save preview", false);
    let inner = block.inner(modal);
    f.render_widget(block, modal);

    // summary · body (scrollable) · hints
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);

    let (added, removed) = diff::stats(&app.diff_preview);
    let path = app.config.path.display().to_string();
    let summary = Line::from(vec![
        Span::styled(
            format!(" +{added}"),
            Style::default().fg(theme::UP).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  -{removed}"),
            Style::default()
                .fg(theme::DOWN)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("   {path}"), Style::default().fg(theme::FAINT)),
    ]);
    f.render_widget(Paragraph::new(summary), rows[0]);

    let body = if app.diff_preview.is_empty() {
        // Defensive: the modal is never opened on an empty diff, but never paint a
        // blank void if it somehow is.
        vec![Line::from(Span::styled(
            " no changes",
            Style::default().fg(theme::DIM),
        ))]
    } else {
        app.diff_preview.iter().map(diff_line).collect()
    };
    f.render_widget(
        Paragraph::new(Text::from(body)).scroll((app.diff_scroll, 0)),
        rows[1],
    );

    let hints = footer_hints(&[("Enter", "save"), ("Esc", "back"), ("j/k", "scroll")]);
    f.render_widget(Paragraph::new(hints), rows[2]);
}

/// Paint one diff line: `+` green (added), `-` red (removed), space + dim
/// (context). The whole line — sign and text — shares the tag's color so a change
/// reads at a glance.
fn diff_line(dl: &DiffLine) -> Line<'static> {
    let (sign, color, bold) = match dl {
        DiffLine::Add(_) => ('+', theme::UP, true),
        DiffLine::Del(_) => ('-', theme::DOWN, true),
        DiffLine::Context(_) => (' ', theme::DIM, false),
    };
    let mut style = Style::default().fg(color);
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    Line::from(Span::styled(format!("{sign} {}", dl.text()), style))
}
