//! #43 — effective-config inspector: a host's `ssh -G` resolution as a
//! searchable, scrollable key/value list.
//!
//! `ssh -G` lowercases keys, normalizes values, and emits compile-time
//! defaults, so this view is an APPROXIMATION of the written config — not a
//! source-of-truth diff. That caveat is surfaced in a dim note line (rather than
//! by highlighting "written vs default", which would misclassify — Issue #43
//! risk #2). Pure rendering; the resolved rows are loaded once in
//! `update::open_inspect`, never recomputed here.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};

use crate::app::App;

use super::theme;
use super::widgets::{input_line, panel};

/// Cap on the aligned key column so a pathologically long key can't push every
/// value off-screen; longer keys simply aren't padded.
const KEY_COL_MAX: usize = 22;

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let rows = Layout::vertical([
        Constraint::Length(1), // search line
        Constraint::Length(1), // honesty note
        Constraint::Min(1),    // resolved key/value list
    ])
    .split(area);
    draw_search(f, app, rows[0]);
    draw_note(f, rows[1]);

    // Collect the filtered rows once (both the key-column width and the list
    // items iterate them). The extra `Vec<&_>` is negligible on a render path.
    let entries: Vec<&(String, String)> = app
        .inspect_filtered()
        .iter()
        .filter_map(|&i| app.inspect_rows.get(i))
        .collect();
    let key_w = entries
        .iter()
        .map(|(k, _)| k.len())
        .max()
        .unwrap_or(0)
        .min(KEY_COL_MAX);

    let items: Vec<ListItem> = entries
        .iter()
        .map(|(k, v)| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{k:<key_w$}  "),
                    Style::default()
                        .fg(theme::ACCENT2)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(v.as_str(), Style::default().fg(theme::TEXT)),
            ]))
        })
        .collect();

    let title = format!(
        "Effective config · {}  [{}/{}]",
        app.inspect_alias,
        entries.len(),
        app.inspect_rows.len()
    );
    let list = List::new(items)
        .block(panel(&title, true))
        .highlight_style(theme::selection())
        .highlight_symbol(theme::SELECT_SYMBOL);
    f.render_stateful_widget(list, rows[2], &mut app.inspect_state);
}

fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let prefix = Span::styled(
        " / ",
        if app.inspect_searching {
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::FAINT)
        },
    );
    let body = input_line(
        &app.inspect_search,
        app.inspect_search.len(),
        app.inspect_searching,
    );
    let mut spans = vec![prefix];
    spans.extend(body.spans);
    if app.inspect_search.is_empty() && !app.inspect_searching {
        spans.push(Span::styled(
            "filter key / value",
            Style::default().fg(theme::FAINT),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_note(f: &mut Frame, area: Rect) {
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " ssh -G normalizes keys/values and includes defaults — an approximation, not a diff.",
            Style::default().fg(theme::FAINT),
        ))),
        area,
    );
}
