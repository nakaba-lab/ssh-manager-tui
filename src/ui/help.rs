//! O1 — context-aware help overlay.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::{App, Screen};

use super::theme;
use super::widgets::{centered_pct, modal_block};

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let modal = centered_pct(72, 84, area);
    f.render_widget(Clear, modal);

    let block = modal_block("Help  (Esc / ? to close)", false);

    let base = app.prev_screen.clone().unwrap_or(Screen::List);
    let mut lines = vec![
        section("Global"),
        key("?", "toggle help"),
        key("Esc", "back / cancel one level"),
        key("q / Ctrl-C", "quit"),
    ];

    match base {
        Screen::Edit { .. } => {
            lines.push(section("Edit form"));
            lines.push(key("Tab / Shift-Tab", "next / previous field"));
            lines.push(key("j / k", "next / previous field (navigate)"));
            lines.push(key(
                "Enter",
                "IdentityFile/ProxyJump: open picker; else edit",
            ));
            lines.push(key("i", "edit field manually (text)"));
            lines.push(key("a / d", "add / remove row (lists)"));
            lines.push(key("Ctrl-S", "validate & save"));
            lines.push(key("Esc", "cancel field, then cancel form"));
        }
        Screen::ConnectOverride { .. } => {
            lines.push(section("Connect override (session-only)"));
            lines.push(key("Tab / j / k", "next / previous field"));
            lines.push(key("Enter", "IdentityFile/ProxyJump: pick; else edit"));
            lines.push(key("i", "edit field manually (text)"));
            lines.push(key("Space / Enter", "toggle Verbose (-v)"));
            lines.push(key("a / d", "add / remove forward or extra row"));
            lines.push(key("Ctrl-O", "connect inline with these overrides"));
            lines.push(key("Ctrl-T", "connect in a new tab"));
            lines.push(key("Ctrl-Y", "copy the ssh command"));
            lines.push(key("Esc", "cancel (nothing is written to config)"));
        }
        Screen::KeyManager => {
            lines.push(section("Key manager"));
            lines.push(key("j / k", "move"));
            lines.push(key("g", "generate new key"));
            lines.push(key("y", "copy public key"));
            lines.push(key("s", "set as IdentityFile for host in context"));
            lines.push(key("d", "delete key (private + public)"));
            lines.push(key("r", "rescan"));
        }
        Screen::KnownHosts => {
            lines.push(section("Known hosts"));
            lines.push(key("j / k", "move"));
            lines.push(key("/", "search"));
            lines.push(key("d", "delete entry"));
            lines.push(key("r", "reload"));
        }
        Screen::Vault => {
            lines.push(section("Password vault"));
            lines.push(key("j / k", "move"));
            lines.push(key("a", "add secret"));
            lines.push(key("e / Enter", "edit secret"));
            lines.push(key("y / c", "copy secret to clipboard"));
            lines.push(key("d", "delete secret"));
            lines.push(key("Space", "reveal / mask secrets"));
            lines.push(key("p", "toggle connect-time password auto-fill"));
            lines.push(key("L", "lock vault now (forget master password)"));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  The vault auto-locks after 15 min idle.",
                Style::default().fg(theme::FAINT),
            )));
            lines.push(Line::from(Span::styled(
                "  Secrets are encrypted with your master password in",
                Style::default().fg(theme::FAINT),
            )));
            lines.push(Line::from(Span::styled(
                "  ~/.ssh/sshm-vault.json — never written to the SSH config.",
                Style::default().fg(theme::FAINT),
            )));
        }
        _ => {
            lines.push(section("Host list"));
            lines.push(key("j / k, ↑ / ↓", "move selection"));
            lines.push(key("g / G", "top / bottom"));
            lines.push(key("Tab", "toggle list / detail focus"));
            lines.push(key("/", "search (alias / hostname / user)"));
            lines.push(key("s", "cycle sort (file/recent/name/status)"));
            lines.push(key("Enter", "connect inline (same console)"));
            lines.push(key("t", "connect in new Windows Terminal tab"));
            lines.push(key("F", "open SFTP session (inline)"));
            lines.push(key("b", "SFTP browser (dual-pane: local | remote)"));
            lines.push(key("O", "connect with one-off overrides"));
            lines.push(key("o", "action menu (SFTP session / transfer, …)"));
            lines.push(key("c", "copy ssh command"));
            lines.push(key("e / a", "edit / add host"));
            lines.push(key("d", "delete host"));
            lines.push(key("r / R", "refresh liveness (all / selected)"));
            lines.push(key("K", "key manager"));
            lines.push(key("H", "known hosts"));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Hosts are read from & written to ~/.ssh/config directly.",
        Style::default().fg(theme::FAINT),
    )));

    f.render_widget(Paragraph::new(Text::from(lines)).block(block), modal);
}

fn section(title: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {title}"),
        Style::default()
            .fg(theme::ACCENT)
            .add_modifier(Modifier::BOLD),
    ))
}

fn key(k: &str, label: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("   {k:<18}"),
            Style::default()
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(label.to_string(), Style::default().fg(theme::DIM)),
    ])
}
