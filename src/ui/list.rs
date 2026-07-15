//! S1 — host list: search box, table with liveness column, and a detail pane.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, List, ListItem, Paragraph, Row, Table, Wrap};

use crate::app::{App, ListFocus, PickOrigin};
use crate::config::model::HostView;
use crate::os::history;
use crate::os::liveness::Liveness;

use super::theme;
use super::widgets::{
    centered, input_line, kv_line, liveness_span, modal_block, panel, responsive_split,
    section_header,
};

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    if app.hosts.is_empty() {
        draw_empty(f, app, area);
        return;
    }

    // wide: list 58% width; stacked (narrow): list 60% height
    let (list_area, detail_area) = responsive_split(area, 58, 60);
    draw_list_pane(f, app, list_area);
    draw_detail_pane(f, app, detail_area);
}

fn draw_empty(f: &mut Frame, app: &App, area: Rect) {
    let block = panel("No hosts yet", false);
    let accent_key = |k: &str| {
        Span::styled(
            k.to_string(),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    };
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "~/.ssh/config has no Host entries.",
            Style::default().fg(theme::DIM),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Press ", Style::default().fg(theme::DIM)),
            accent_key("a"),
            Span::styled(" to add your first host, ", Style::default().fg(theme::DIM)),
            accent_key("K"),
            Span::styled(" to manage keys, ", Style::default().fg(theme::DIM)),
            accent_key("?"),
            Span::styled(" for help.", Style::default().fg(theme::DIM)),
        ]),
    ];
    if app.include_note {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Note: Include directives present — hosts in included files are not shown.",
            Style::default().fg(theme::WARN),
        )));
    }
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
}

fn draw_list_pane(f: &mut Frame, app: &mut App, area: Rect) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    draw_search(f, app, rows[0]);

    let focused = app.focus == ListFocus::Hosts;
    let header = Row::new(["", "Alias", "HostName", "User"])
        .style(Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD));

    let table_rows: Vec<Row> = app
        .filtered
        .iter()
        .filter_map(|&i| app.hosts.get(i).map(|h| (i, h)))
        .map(|(i, h)| {
            let state = app.liveness_by_index(i);
            let mut alias_spans = vec![
                secret_indicator_span(app, h),
                Span::raw(h.alias().to_string()),
            ];
            // #45: tags as inline `#chip`s right of the alias, in the accent color.
            if !h.tags.is_empty() {
                alias_spans.push(Span::styled(
                    format!("  {}", h.tags_display()),
                    Style::default().fg(theme::ACCENT),
                ));
            }
            Row::new(vec![
                Line::from(liveness_span(state)),
                Line::from(alias_spans),
                Line::from(h.host_name.clone().unwrap_or_else(|| "—".into())),
                Line::from(h.user.clone().unwrap_or_else(|| "—".into())),
            ])
        })
        .collect();

    let table = Table::new(
        table_rows,
        [
            Constraint::Length(2),
            Constraint::Percentage(40),
            Constraint::Percentage(38),
            Constraint::Percentage(22),
        ],
    )
    .header(header)
    .block(panel("Hosts", focused))
    .row_highlight_style(theme::selection())
    .highlight_symbol(theme::SELECT_SYMBOL);

    f.render_stateful_widget(table, rows[1], &mut app.list_state);
}

/// The 2-cell stored-secret indicator prefix for a host row: a glyph (password
/// `p`, passphrase `k`, both `*`) coloured active (the host has a `known_hosts`
/// pin) or muted (a candidate not yet trusted), or two blanks when the host has no
/// stored secret or the vault is locked. Always two cells so the alias stays
/// aligned. Mirrors connect dispatch exactly — both consult `vault_secret_kinds`.
fn secret_indicator_span(app: &App, host: &HostView) -> Span<'static> {
    let Some(kinds) = app.vault_secret_kinds(host) else {
        return Span::raw("  ");
    };
    let glyph = match (kinds.password, kinds.passphrase) {
        (true, true) => theme::SECRET_BOTH,
        (true, false) => theme::SECRET_PASSWORD,
        (false, true) => theme::SECRET_PASSPHRASE,
        (false, false) => return Span::raw("  "), // unreachable: any() held
    };
    let color = if app.host_known_hint(host) {
        theme::ACCENT2
    } else {
        theme::FAINT
    };
    Span::styled(format!("{glyph} "), Style::default().fg(color))
}

fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let prefix = Span::styled(
        " / ",
        if app.searching {
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::FAINT)
        },
    );
    let body = input_line(&app.search, app.search.len(), app.searching);
    let mut spans = vec![prefix];
    spans.extend(body.spans);
    if app.search.is_empty() && !app.searching {
        spans.push(Span::styled(
            "search (alias / hostname / user / tag)",
            Style::default().fg(theme::FAINT),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Host picker modal, opened from the edit form's ProxyJump field.
pub fn draw_jump_picker(f: &mut Frame, app: &mut App, origin: &PickOrigin, area: Rect) {
    let candidates = app.jump_candidates(app.pick_jump_self_alias(origin));
    let modal = centered(60, 14, area);
    f.render_widget(Clear, modal);

    let block = modal_block("Pick ProxyJump host", false);

    if candidates.is_empty() {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No other registered hosts to jump through.",
                Style::default().fg(theme::DIM),
            )),
            Line::from(Span::styled(
                "  Add a host first, or type the target manually (Esc).",
                Style::default().fg(theme::FAINT),
            )),
        ];
        f.render_widget(Paragraph::new(Text::from(lines)).block(block), modal);
        return;
    }

    let inner = block.inner(modal);
    f.render_widget(block, modal);
    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);

    let items: Vec<ListItem> = candidates
        .iter()
        .filter_map(|&i| app.hosts.get(i))
        .map(|h| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    h.alias().to_string(),
                    Style::default()
                        .fg(theme::TEXT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}", h.host_name.clone().unwrap_or_else(|| "—".into())),
                    Style::default().fg(theme::DIM),
                ),
                Span::styled(
                    h.user
                        .clone()
                        .map(|u| format!("  ({u})"))
                        .unwrap_or_default(),
                    Style::default().fg(theme::DIM),
                ),
            ]))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(theme::selection())
        .highlight_symbol(theme::SELECT_SYMBOL);
    f.render_stateful_widget(list, rows[0], &mut app.pick_jump_state);

    f.render_widget(
        Paragraph::new(Span::styled(
            " j/k move · Enter select · Esc cancel",
            Style::default().fg(theme::FAINT),
        )),
        rows[1],
    );
}

fn draw_detail_pane(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == ListFocus::Detail;
    let block = panel("Detail", focused);

    let Some(host_idx) = app.selected_host() else {
        f.render_widget(block, area);
        return;
    };
    let Some(h) = app.hosts.get(host_idx) else {
        f.render_widget(block, area);
        return;
    };

    let state = app.liveness_by_index(host_idx);
    let status = match state {
        Liveness::Up => {
            let rtt = app
                .rtt_by_index(host_idx)
                .map(|d| format!(" ({} ms)", d.as_millis()))
                .unwrap_or_default();
            Span::styled(
                format!("up{rtt}"),
                Style::default().fg(theme::UP).add_modifier(Modifier::BOLD),
            )
        }
        Liveness::Down => Span::styled("down", Style::default().fg(theme::DOWN)),
        Liveness::Checking => Span::styled("checking…", Style::default().fg(theme::CHECKING)),
        Liveness::Skipped => Span::styled("skipped (proxy)", Style::default().fg(theme::FAINT)),
        Liveness::Unknown => Span::styled("unknown", Style::default().fg(theme::FAINT)),
    };

    let mut lines: Vec<Line> = Vec::new();

    // Headline: status (ungrouped).
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:>14}  ", "status"),
            Style::default().fg(theme::DIM),
        ),
        status,
    ]));

    // Connection (always shown).
    lines.push(Line::from(""));
    lines.push(section_header("Connection"));
    lines.push(kv_line("alias", h.patterns.join(" ")));
    lines.push(kv_line(
        "HostName",
        h.host_name.clone().unwrap_or_else(|| "—".into()),
    ));
    lines.push(kv_line(
        "User",
        h.user.clone().unwrap_or_else(|| "—".into()),
    ));
    lines.push(kv_line(
        "Port",
        h.port.clone().unwrap_or_else(|| "—".into()),
    ));
    let last_conn = match app.history.last(h.alias()) {
        Some(t) => history::relative_label(t, history::now_unix()),
        None => "never".into(),
    };
    lines.push(kv_line("Last connected", last_conn));
    if let Some(j) = &h.proxy_jump {
        lines.push(kv_line("ProxyJump", j.clone()));
    }
    // Auto-fill (only when the vault is unlocked and has a stored secret here).
    if let Some(kinds) = app.vault_secret_kinds(h) {
        let mut parts = Vec::new();
        if kinds.password {
            parts.push("password");
        }
        if kinds.passphrase {
            parts.push("passphrase");
        }
        let suffix = if app.host_known_hint(h) {
            ""
        } else {
            " (accept host key first)"
        };
        lines.push(kv_line(
            "Auto-fill",
            format!("{}{suffix}", parts.join(" + ")),
        ));
    }

    // Metadata: tags & description (#45; only when present).
    if !h.tags.is_empty() || h.description.is_some() {
        lines.push(Line::from(""));
        lines.push(section_header("Metadata"));
        if !h.tags.is_empty() {
            lines.push(kv_line("Tags", h.tags_display()));
        }
        if let Some(d) = &h.description {
            lines.push(kv_line("Description", d.clone()));
        }
    }

    // Identity (only when present).
    if !h.identity_files.is_empty() {
        lines.push(Line::from(""));
        lines.push(section_header("Identity"));
        for id in &h.identity_files {
            lines.push(kv_line("IdentityFile", id.clone()));
        }
    }

    // Forwarding (only when present).
    if !h.local_forwards.is_empty()
        || !h.remote_forwards.is_empty()
        || !h.dynamic_forwards.is_empty()
    {
        lines.push(Line::from(""));
        lines.push(section_header("Forwarding"));
        for fwd in &h.local_forwards {
            lines.push(kv_line("LocalForward", fwd.clone()));
        }
        for fwd in &h.remote_forwards {
            lines.push(kv_line("RemoteForward", fwd.clone()));
        }
        for fwd in &h.dynamic_forwards {
            lines.push(kv_line("DynamicForward", fwd.clone()));
        }
    }

    // Other (only when present).
    if !h.extras.is_empty() {
        lines.push(Line::from(""));
        lines.push(section_header("Other"));
        for (k, v) in &h.extras {
            lines.push(kv_line(k, v.clone()));
        }
    }

    let para = Paragraph::new(Text::from(lines))
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll, 0));
    f.render_widget(para, area);
}
