//! S3 — key manager: list of `~/.ssh/*.pub` keys with detail, and the
//! generate-key wizard (O4) modal.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, List, ListItem, Paragraph, Wrap};

use crate::app::App;
use crate::os::agent::{AgentSnapshot, AgentStatus, KeyAgentState, ServiceState};
use crate::os::keys::{KeyType, PairStatus};

use super::theme;
use super::widgets::{
    centered, input_line, kv_line, kv_line_colored, modal_block, panel, responsive_split,
    section_header,
};

/// Advice lines for the current agent/service pairing. Empty when there is
/// nothing useful to say.
fn service_advice(snapshot: &AgentSnapshot) -> Vec<&'static str> {
    match (&snapshot.status, snapshot.service) {
        // Stock Windows ships ssh-agent *disabled*, and `sc query` reports a
        // disabled service as plain STOPPED — indistinguishable from stopped.
        // Advising only `Start-Service` would therefore fail outright ("Cannot
        // start service ssh-agent") for the single most common case, so both
        // steps are given.
        (_, Some(ServiceState::Stopped)) => vec![
            "As Administrator: Set-Service ssh-agent -StartupType Automatic",
            "then: Start-Service ssh-agent",
        ],
        // A running service we cannot reach almost always means sshm and the
        // agent are in different security contexts — the classic symptom of
        // launching one of them elevated.
        (AgentStatus::NotRunning, Some(ServiceState::Running)) => {
            vec![
                "Service is running but unreachable — is sshm elevated and the agent not (or vice versa)?",
            ]
        }
        _ => Vec::new(),
    }
}

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
            let mut spans = vec![
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
            ];
            if k.pair == PairStatus::Mismatched {
                spans.push(Span::styled(
                    "  mismatch",
                    Style::default()
                        .fg(theme::DOWN)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            // Agent membership badge (#49), built like the `mismatch` badge
            // above: plain text in a theme colour, never a glyph — the list
            // width maths stays predictable and no terminal has to have the
            // font for it.
            if app.key_agent_state(k) == KeyAgentState::Loaded {
                spans.push(Span::styled(
                    "  agent",
                    Style::default()
                        .fg(theme::ACCENT)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            ListItem::new(Line::from(spans))
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
    let mut kv = |key: &str, v: String| lines.push(kv_line(key, v));
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
    // Pair verification result — only shown when both halves exist.
    if let Some((text, color)) = match k.pair {
        PairStatus::Matched => Some(("verified — public key matches private key", theme::UP)),
        PairStatus::Mismatched => Some((
            "MISMATCH — public key is not this private key's pair",
            theme::DOWN,
        )),
        PairStatus::Unverified => Some((
            "unverified — could not fingerprint both halves",
            theme::WARN,
        )),
        PairStatus::NotApplicable => None,
    } {
        lines.push(kv_line_colored("pair", text.to_string(), color));
    }

    // --- ssh-agent block (#49) ---
    // Kept as its own section rather than folded into the key/value list above:
    // `status` and `service` describe the agent, not this key, and mixing the
    // two scopes in one column reads as if the agent were a property of the key.
    lines.push(Line::from(""));
    lines.push(section_header("ssh-agent"));

    let (status_text, status_color) = match &app.agent.status {
        AgentStatus::Probing => ("checking…".to_string(), theme::CHECKING),
        AgentStatus::Running(fps) if fps.is_empty() => {
            ("running (no keys)".to_string(), theme::WARN)
        }
        AgentStatus::Running(fps) => (format!("running ({} keys)", fps.len()), theme::UP),
        AgentStatus::NotRunning => ("not running".to_string(), theme::DOWN),
        // Deliberately not "no ssh-add": any exit code outside 0/1/2 lands here
        // too (OpenSSH's fatal() exits 255), so naming a missing binary would
        // send the user down the wrong path in those cases.
        AgentStatus::Unavailable => ("unavailable".to_string(), theme::DOWN),
    };
    lines.push(kv_line_colored("status", status_text, status_color));

    // Absent off Windows, where there is no ssh-agent service to report on.
    if let Some(service) = app.agent.service {
        let (text, color) = match service {
            ServiceState::Running => ("running", theme::UP),
            ServiceState::Stopped => ("stopped or disabled", theme::DOWN),
            ServiceState::Paused => ("paused", theme::WARN),
            ServiceState::Transitioning => ("starting/stopping…", theme::CHECKING),
            ServiceState::Unknown => ("unknown", theme::DIM),
        };
        lines.push(kv_line_colored("service", text.to_string(), color));
    }

    let (key_text, key_color) = match app.key_agent_state(k) {
        KeyAgentState::Loaded => ("loaded", theme::UP),
        KeyAgentState::NotLoaded => ("not loaded", theme::DIM),
        // The key's fingerprint could not be read, so we genuinely do not know —
        // saying "not loaded" here would be a confident lie.
        KeyAgentState::Unknown => ("unknown (no fingerprint)", theme::WARN),
        KeyAgentState::NoAgent => ("—", theme::DIM),
    };
    lines.push(kv_line_colored("this key", key_text.to_string(), key_color));

    // Actionable advice only. Starting the service needs elevation, so we print
    // the command rather than silently failing to run it ourselves.
    for advice in service_advice(&app.agent) {
        lines.push(Line::from(Span::styled(
            format!("  {advice}"),
            Style::default().fg(theme::FAINT),
        )));
    }

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
