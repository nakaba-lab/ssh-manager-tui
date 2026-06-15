//! Pure rendering. `draw` dispatches by `App.screen`, painting any base screen
//! first and then the active modal overlay on top. Nothing here mutates domain
//! state (only widget scroll state).

pub mod confirm;
pub mod edit;
pub mod help;
pub mod keys;
pub mod known_hosts;
pub mod list;
pub mod theme;
pub mod widgets;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, GenOrigin, Screen};

/// The non-modal screen rendered underneath the current screen (which may be a
/// modal overlay).
fn base_screen(app: &App) -> Screen {
    match &app.screen {
        Screen::Help | Screen::Confirm(_) | Screen::ActionMenu(_) => {
            app.prev_screen.clone().unwrap_or(Screen::List)
        }
        Screen::PickKey { editing } | Screen::PickJump { editing } => {
            Screen::Edit { editing: *editing }
        }
        Screen::GenerateKey { origin } => match origin {
            GenOrigin::KeyManager => Screen::KeyManager,
            GenOrigin::EditForm { editing } => Screen::Edit { editing: *editing },
        },
        other => other.clone(),
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(area);
    let (title_a, body_a, footer_a) = (chunks[0], chunks[1], chunks[2]);

    let base = base_screen(app);

    draw_title(f, app, &base, title_a);
    match &base {
        Screen::Edit { .. } => edit::draw(f, app, body_a),
        Screen::KeyManager => keys::draw(f, app, body_a),
        Screen::KnownHosts => known_hosts::draw(f, app, body_a),
        _ => list::draw(f, app, body_a),
    }
    draw_footer(f, app, &base, footer_a);

    // Modal overlays on top of the base screen.
    match &app.screen {
        Screen::Help => help::draw(f, app, body_a),
        Screen::Confirm(action) => confirm::draw(f, action.clone(), body_a),
        Screen::ActionMenu(idx) => confirm::draw_action_menu(f, app, *idx, body_a),
        Screen::GenerateKey { .. } => keys::draw_wizard(f, app, body_a),
        Screen::PickKey { .. } => keys::draw_picker(f, app, body_a),
        Screen::PickJump { .. } => list::draw_jump_picker(f, app, body_a),
        _ => {}
    }

    draw_toast(f, app, body_a);
}

fn draw_title(f: &mut Frame, app: &App, base: &Screen, area: Rect) {
    let name = match base {
        Screen::List => "Hosts",
        Screen::Edit { editing: Some(_) } => "Edit host",
        Screen::Edit { editing: None } => "Add host",
        Screen::KeyManager => "Keys",
        Screen::KnownHosts => "Known hosts",
        _ => "SSH Manager",
    };
    let count = match base {
        Screen::List => format!("  {}/{} ", app.filtered.len(), app.hosts.len()),
        Screen::KeyManager => format!("  {} keys ", app.keys.len()),
        Screen::KnownHosts => format!("  {} entries ", app.known_hosts.len()),
        _ => String::new(),
    };
    let mut spans = vec![
        Span::styled(
            " sshm",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ›  ", Style::default().fg(theme::FAINT)),
        Span::styled(
            name,
            Style::default()
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(count, Style::default().fg(theme::FAINT)),
    ];
    if app.ssh_path_warning {
        spans.push(Span::styled(
            "  [PATH ssh]",
            Style::default().fg(theme::WARN),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_footer(f: &mut Frame, app: &App, base: &Screen, area: Rect) {
    let hints = match (base, app) {
        (Screen::List, a) if a.searching => {
            widgets::footer_hints(&[("type", "filter"), ("Enter", "keep"), ("Esc", "clear")])
        }
        (Screen::List, a) if a.hosts.is_empty() => widgets::footer_hints(&[
            ("a", "add host"),
            ("K", "keys"),
            ("H", "known-hosts"),
            ("?", "help"),
            ("q", "quit"),
        ]),
        (Screen::List, _) => widgets::footer_hints(&[
            ("j/k", "move"),
            ("/", "search"),
            ("Enter", "connect"),
            ("t", "new-tab"),
            ("s/S", "sftp"),
            ("e", "edit"),
            ("a", "add"),
            ("d", "del"),
            ("K", "keys"),
            ("?", "help"),
        ]),
        (Screen::Edit { .. }, a) if a.form.mode == crate::app::FormMode::Editing => {
            widgets::footer_hints(&[("Enter", "commit"), ("Esc", "cancel field")])
        }
        (Screen::Edit { .. }, a) if a.form.focused == crate::app::form_idx::IDENTITY => {
            widgets::footer_hints(&[
                ("Enter", "pick/gen key"),
                ("i", "edit"),
                ("a/d", "row +/-"),
                ("Ctrl-S", "save"),
                ("Esc", "back"),
            ])
        }
        (Screen::Edit { .. }, a) if a.form.focused == crate::app::form_idx::PROXYJUMP => {
            widgets::footer_hints(&[
                ("Enter", "pick host"),
                ("i", "edit"),
                ("Ctrl-S", "save"),
                ("Esc", "back"),
            ])
        }
        (Screen::Edit { .. }, _) => widgets::footer_hints(&[
            ("Tab", "field"),
            ("Enter", "edit"),
            ("a/d", "row +/-"),
            ("Ctrl-S", "save"),
            ("Esc", "back"),
        ]),
        (Screen::KeyManager, _) => widgets::footer_hints(&[
            ("j/k", "move"),
            ("g", "generate"),
            ("y", "copy pub"),
            ("s", "set-id"),
            ("d", "delete"),
            ("Esc", "back"),
        ]),
        (Screen::KnownHosts, a) if a.kh_searching => {
            widgets::footer_hints(&[("type", "filter"), ("Esc", "clear")])
        }
        (Screen::KnownHosts, _) => widgets::footer_hints(&[
            ("j/k", "move"),
            ("/", "search"),
            ("d", "delete"),
            ("Esc", "back"),
        ]),
        _ => widgets::footer_hints(&[("?", "help"), ("q", "quit")]),
    };
    f.render_widget(Paragraph::new(hints), area);
}

fn draw_toast(f: &mut Frame, app: &App, area: Rect) {
    if app.toast.text.is_empty() {
        return;
    }
    let (marker, style) = if app.toast.is_error {
        (
            "✗",
            Style::default()
                .fg(theme::BG)
                .bg(theme::DOWN)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            "✓",
            Style::default()
                .fg(theme::UP)
                .bg(theme::SEL_BG)
                .add_modifier(Modifier::BOLD),
        )
    };
    let text = format!(" {marker} {} ", app.toast.text);
    let width = (text.chars().count() as u16 + 2).min(area.width);
    let toast_area = Rect {
        x: area.x + area.width.saturating_sub(width),
        y: area.y + area.height.saturating_sub(1),
        width,
        height: 1,
    };
    f.render_widget(Paragraph::new(Span::styled(text, style)), toast_area);
}
