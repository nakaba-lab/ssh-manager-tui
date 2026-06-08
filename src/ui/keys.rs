//! S3 — key manager: list of `~/.ssh/*.pub` keys with detail, and the
//! generate-key wizard (O4) modal.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, List, ListItem, Paragraph, Wrap};

use crate::app::App;
use crate::os::keys::KeyType;

use super::theme;
use super::widgets::{centered, input_line, modal_block, panel, responsive_split};

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    if app.keys.is_empty() {
        let block = panel("Keys", false);
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No keys found in ~/.ssh.",
                Style::default().fg(theme::DIM),
            )),
            Line::from(Span::styled(
                "  Press 'g' to generate one.",
                Style::default().fg(theme::FAINT),
            )),
        ];
        f.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
        return;
    }

    // wide: list 45% width; stacked (narrow): list 55% height
    let (list_area, detail_area) = responsive_split(area, 45, 55);

    let items: Vec<ListItem> = app
        .keys
        .iter()
        .map(|k| {
            let (mark, mark_style) = if k.has_private {
                ("● ", Style::default().fg(theme::ACCENT))
            } else {
                ("○ ", Style::default().fg(theme::FAINT))
            };
            ListItem::new(Line::from(vec![
                Span::styled(mark, mark_style),
                Span::styled(
                    k.name(),
                    Style::default()
                        .fg(theme::TEXT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {} {}", k.key_type, k.bits),
                    Style::default().fg(theme::DIM),
                ),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(panel("Keys", true))
        .highlight_style(theme::selection())
        .highlight_symbol(theme::SELECT_SYMBOL);
    f.render_stateful_widget(list, list_area, &mut app.keys_state);

    draw_detail(f, app, detail_area);
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let block = panel("Key detail", false);

    let Some(k) = app.keys_state.selected().and_then(|i| app.keys.get(i)) else {
        f.render_widget(block, area);
        return;
    };

    let mut lines: Vec<Line> = Vec::new();
    let mut kv = |key: &str, v: String| {
        lines.push(Line::from(vec![
            Span::styled(format!("{key:>14}  "), Style::default().fg(theme::DIM)),
            Span::styled(v, Style::default().fg(theme::TEXT)),
        ]));
    };
    kv("name", k.name());
    kv("type", k.key_type.clone());
    kv("bits", k.bits.to_string());
    kv("fingerprint", k.fingerprint.clone());
    kv(
        "comment",
        if k.comment.is_empty() {
            "—".into()
        } else {
            k.comment.clone()
        },
    );
    kv(
        "private key",
        if k.has_private {
            "present".into()
        } else {
            "MISSING".into()
        },
    );
    kv(
        "public file",
        match &k.pub_path {
            Some(p) => p.display().to_string(),
            None => "MISSING".into(),
        },
    );

    if let Some(ctx) = app.key_host_ctx.and_then(|i| app.hosts.get(i)) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                "  Press 's' to set as IdentityFile for host '{}'.",
                ctx.alias()
            ),
            Style::default().fg(theme::WARN),
        )));
    }

    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// Key picker modal, opened from the edit form's IdentityFile field.
pub fn draw_picker(f: &mut Frame, app: &mut App, area: Rect) {
    let modal = centered(60, 14, area);
    f.render_widget(Clear, modal);

    let block = modal_block("Pick IdentityFile", false);

    if app.keys.is_empty() {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No keys in ~/.ssh.",
                Style::default().fg(theme::DIM),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Press 'g' to generate one, or Esc to cancel.",
                Style::default().fg(theme::FAINT),
            )),
        ];
        f.render_widget(Paragraph::new(Text::from(lines)).block(block), modal);
        return;
    }

    let inner = block.inner(modal);
    f.render_widget(block, modal);

    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);

    let items: Vec<ListItem> = app
        .keys
        .iter()
        .map(|k| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    k.name(),
                    Style::default()
                        .fg(theme::TEXT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {} {}  {}", k.key_type, k.bits, k.fingerprint),
                    Style::default().fg(theme::DIM),
                ),
            ]))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(theme::selection())
        .highlight_symbol(theme::SELECT_SYMBOL);
    f.render_stateful_widget(list, rows[0], &mut app.pick_key_state);

    f.render_widget(
        Paragraph::new(Span::styled(
            " j/k move · Enter select · g generate · Esc cancel",
            Style::default().fg(theme::FAINT),
        )),
        rows[1],
    );
}

/// Generate-key wizard modal (O4).
pub fn draw_wizard(f: &mut Frame, app: &App, area: Rect) {
    let w = app.gen_wizard.clone();
    let modal = centered(64, 11, area);
    f.render_widget(Clear, modal);

    let block = modal_block("Generate key", false);

    let label = |s: &str, focused: bool| {
        Span::styled(
            format!("{s:<12}"),
            if focused {
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::DIM)
            },
        )
    };
    let opt = |sel: bool, text: &str| {
        Span::styled(
            format!("{} {}", if sel { "(•)" } else { "( )" }, text),
            Style::default().fg(if sel { theme::ACCENT } else { theme::DIM }),
        )
    };
    let type_value = Line::from(vec![
        label("Type", w.field == 0),
        opt(w.key_type == KeyType::Ed25519, "ed25519   "),
        opt(w.key_type == KeyType::Rsa4096, "rsa-4096"),
    ]);
    let file_value = Line::from({
        let mut s = vec![label("Filename", w.field == 1)];
        s.extend(input_line(&w.filename, w.filename_cursor, w.field == 1).spans);
        s
    });
    let comment_value = Line::from({
        let mut s = vec![label("Comment", w.field == 2)];
        s.extend(input_line(&w.comment, w.comment_cursor, w.field == 2).spans);
        s
    });

    let lines = vec![
        Line::from(""),
        type_value,
        Line::from(""),
        file_value,
        Line::from(""),
        comment_value,
        Line::from(""),
        Line::from(Span::styled(
            "  Tab/↑↓ move · Space toggle type · Enter generate · Esc cancel",
            Style::default().fg(theme::FAINT),
        )),
        Line::from(Span::styled(
            "  Saved to ~/.ssh/ · no passphrase (v1)",
            Style::default().fg(theme::FAINT),
        )),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), modal);
}
