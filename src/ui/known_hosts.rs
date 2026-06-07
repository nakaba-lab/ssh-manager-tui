//! S4 — known_hosts viewer: searchable list with delete.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};

use crate::app::App;

use super::theme;
use super::widgets::{input_line, panel};

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    draw_search(f, app, rows[0]);

    let filtered = app.kh_filtered();
    let items: Vec<ListItem> = filtered
        .iter()
        .filter_map(|&i| app.known_hosts.get(i))
        .map(|e| {
            let marker = e
                .marker
                .as_deref()
                .map(|m| format!("{m} "))
                .unwrap_or_default();
            ListItem::new(Line::from(vec![
                Span::styled(marker, Style::default().fg(theme::ACCENT2)),
                Span::styled(
                    e.host.display(),
                    Style::default()
                        .fg(theme::TEXT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {}", e.key_type), Style::default().fg(theme::DIM)),
            ]))
        })
        .collect();

    let title = format!(
        "Known hosts  [{}/{}]",
        filtered.len(),
        app.known_hosts.len()
    );
    let list = List::new(items)
        .block(panel(&title, true))
        .highlight_style(theme::selection())
        .highlight_symbol(theme::SELECT_SYMBOL);
    f.render_stateful_widget(list, rows[1], &mut app.kh_state);
}

fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let prefix = Span::styled(
        " / ",
        if app.kh_searching {
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::FAINT)
        },
    );
    let body = input_line(&app.kh_search, app.kh_search.len(), app.kh_searching);
    let mut spans = vec![prefix];
    spans.extend(body.spans);
    if app.kh_search.is_empty() && !app.kh_searching {
        spans.push(Span::styled(
            "search host / key type",
            Style::default().fg(theme::FAINT),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
