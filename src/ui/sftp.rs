//! Inline SFTP transfer modal: pick a direction plus a local and a remote path,
//! then run a one-shot `sftp -b` transfer inline. Pure rendering — all state
//! lives in [`crate::app::SftpForm`]; submitting is handled in `update.rs`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::{App, SftpBrowser, SftpPane};

use super::theme;
use super::widgets::{
    centered_pct, input_line, modal_block, panel, responsive_split, section_header,
};

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

// ---------------------------------------------------------------------------
// Dual-pane browser (Phase 3)
// ---------------------------------------------------------------------------

/// Render the dual-pane SFTP browser: local files left, remote files right, with
/// a one-line status/help bar below. A full base screen (not an overlay).
pub fn draw_browser(f: &mut Frame, app: &App, area: Rect) {
    let Some(b) = app.sftp_browser.as_ref() else {
        return;
    };

    // Reserve one row at the bottom for the status/help bar.
    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
    let (panes, status_row) = (rows[0], rows[1]);
    let (left, right) = responsive_split(panes, 50, 50);

    draw_local_pane(f, b, left);
    draw_remote_pane(f, b, right);
    draw_status(f, b, status_row);
}

fn draw_local_pane(f: &mut Frame, b: &SftpBrowser, area: Rect) {
    let focused = b.focus == SftpPane::Local;
    let title = format!("Local · {}", b.local_cwd.display());
    let block = panel(&title, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);

    render_entry_list(f, &b.local_entries, b.local_sel, focused, inner, |e| {
        entry_line(&e.name, e.is_dir, false, None)
    });
}

fn draw_remote_pane(f: &mut Frame, b: &SftpBrowser, area: Rect) {
    let focused = b.focus == SftpPane::Remote;
    let cwd = if b.remote_cwd.is_empty() {
        "~".to_string()
    } else {
        b.remote_cwd.clone()
    };
    let spinner = if b.remote_loading { " …" } else { "" };
    let title = format!("Remote · {cwd}{spinner}");
    let block = panel(&title, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if b.remote_entries.is_empty() {
        let msg = if b.remote_loading {
            "loading…"
        } else {
            "(empty)"
        };
        f.render_widget(
            Paragraph::new(Span::styled(msg, Style::default().fg(theme::FAINT))),
            inner,
        );
        return;
    }
    render_entry_list(f, &b.remote_entries, b.remote_sel, focused, inner, |e| {
        entry_line(&e.name, e.is_dir, e.is_link, Some(e.size))
    });
}

/// One directory-entry line: a type glyph, the name, and (remote) a size column.
fn entry_line(name: &str, is_dir: bool, is_link: bool, size: Option<u64>) -> Line<'static> {
    let (glyph, color) = if is_dir {
        ("/", theme::ACCENT)
    } else if is_link {
        ("~", theme::ACCENT2)
    } else {
        (" ", theme::TEXT)
    };
    let mut spans = vec![
        Span::styled(format!("{glyph} "), Style::default().fg(color)),
        Span::styled(name.to_string(), Style::default().fg(color)),
    ];
    if let Some(sz) = size.filter(|_| !is_dir) {
        spans.push(Span::styled(
            format!("  {}", human_size(sz)),
            Style::default().fg(theme::FAINT),
        ));
    }
    Line::from(spans)
}

/// First visible row index so `sel` stays on screen in a `height`-row viewport.
fn list_scroll(sel: usize, height: usize) -> usize {
    if height > 0 && sel >= height {
        sel - height + 1
    } else {
        0
    }
}

/// Render `entries` as a selectable list, building a [`Line`] only for the rows
/// actually on screen (O(viewport), not O(entries)) so a huge listing can't pin the
/// UI thread re-materializing every row each frame (M5). Scrolls so `sel` stays
/// visible and paints the selection bar when the pane is `focused`.
fn render_entry_list<T>(
    f: &mut Frame,
    entries: &[T],
    sel: usize,
    focused: bool,
    area: Rect,
    to_line: impl Fn(&T) -> Line<'static>,
) {
    let height = area.height as usize;
    if height == 0 || entries.is_empty() {
        return;
    }
    let scroll = list_scroll(sel, height);
    let visible: Vec<Line> = entries
        .iter()
        .enumerate()
        .skip(scroll)
        .take(height)
        .map(|(i, e)| {
            let mut line = to_line(e);
            if i == sel && focused {
                line = line.style(theme::selection());
            }
            line
        })
        .collect();
    f.render_widget(Paragraph::new(Text::from(visible)), area);
}

fn draw_status(f: &mut Frame, b: &SftpBrowser, area: Rect) {
    // The key hints live in the global footer; this row carries only the live
    // status (errors / "loading…"), in a warning colour when present.
    let line = if b.status.is_empty() {
        Line::from(Span::styled(
            "  Enter on a file transfers it to the other pane.",
            Style::default().fg(theme::FAINT),
        ))
    } else {
        Line::from(Span::styled(
            format!("  {}", b.status),
            Style::default().fg(theme::WARN),
        ))
    };
    f.render_widget(Paragraph::new(line), area);
}

/// A compact human-readable byte size (e.g. `1.2K`, `3.4M`).
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}{}", UNITS[0])
    } else {
        format!("{size:.1}{}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_scroll_keeps_selection_in_view() {
        // Selection within the first page: no scroll.
        assert_eq!(list_scroll(0, 10), 0);
        assert_eq!(list_scroll(9, 10), 0);
        // Selection past the page: scroll so `sel` is the last visible row.
        assert_eq!(list_scroll(10, 10), 1);
        assert_eq!(list_scroll(25, 10), 16);
        // Degenerate height never panics / underflows.
        assert_eq!(list_scroll(5, 0), 0);
    }

    #[test]
    fn human_size_scales_at_1024_boundaries() {
        assert_eq!(human_size(0), "0B");
        assert_eq!(human_size(1023), "1023B");
        assert_eq!(human_size(1024), "1.0K");
        assert_eq!(human_size(1536), "1.5K");
        assert_eq!(human_size(1024 * 1024), "1.0M");
        assert_eq!(human_size(3 * 1024 * 1024 + 512 * 1024), "3.5M");
        assert_eq!(human_size(1024u64.pow(3)), "1.0G");
        assert_eq!(human_size(1024u64.pow(4)), "1.0T");
        // Beyond the last unit it keeps scaling the top unit, never panics.
        assert_eq!(human_size(5 * 1024u64.pow(4)), "5.0T");
    }
}
