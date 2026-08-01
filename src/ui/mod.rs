//! Pure rendering. `draw` dispatches by `App.screen`, painting any base screen
//! first and then the active modal overlay on top. Nothing here mutates domain
//! state (only widget scroll state).

pub mod confirm;
pub mod connect_override;
pub mod diff;
pub mod edit;
pub mod help;
pub mod inspect;
pub mod keys;
pub mod keyscan;
pub mod known_hosts;
pub mod list;
pub mod sftp;
pub mod theme;
pub mod vault;
pub mod widgets;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, GenOrigin, PickOrigin, Screen};

/// Host-list footer hints. Kept to the most-used keys plus the `o` action menu
/// and `?` help (which surface new-tab, overrides, sort, keys, passwords, the
/// SFTP actions, …). Must render within 80 columns — see `footers_fit_80_cols`.
const LIST_FOOTER: &[(&str, &str)] = &[
    ("j/k", "move"),
    ("/", "search"),
    ("Enter", "ssh"),
    ("o", "menu"),
    ("a", "add"),
    ("F", "sftp"),
    ("b", "browse"),
    ("?", "help"),
];

/// SFTP browser footer hints (must render within 80 columns).
const SFTP_BROWSER_FOOTER: &[(&str, &str)] = &[
    ("Tab", "pane"),
    ("j/k", "move"),
    ("Enter", "open/xfer"),
    ("F", "sftp"),
    ("Bksp", "up"),
    ("?", "help"),
    ("Esc", "back"),
];

/// Key-manager footer hints (must render within 80 columns).
///
/// This screen has the most actions in the app, so the full hint list does not
/// fit: with deploy (#77) and the agent actions (#49) it reaches 98 cols. Per
/// the policy above, the two most occasional keys moved to the help modal:
///
/// - `s set-id` — the detail pane *already* prints "Press 's' to set as
///   IdentityFile for host X" exactly when it applies, and pressing it without a
///   host context only toasts an error, so the footer slot bought nothing.
/// - `g gen` — the empty-state text and the key-picker footer both advertise it.
const KEY_MANAGER_FOOTER: &[(&str, &str)] = &[
    ("j/k", "move"),
    ("p", "passphr"),
    ("y", "copy"),
    ("D", "deploy"),
    ("d", "del"),
    ("a", "load"),
    ("U", "unload"),
    ("Esc", "back"),
];

/// Vault footer hints (must render within 80 columns). Like [`LIST_FOOTER`] this
/// carries only the most-used keys: the vault's occasional chords (`p` password
/// auto-fill, `m` master password, `u` KDF upgrade) live in the help modal,
/// because listing them here ran the footer past 100 cols — silently clipping
/// `Esc back` on an 80-column console (found reviewing #47).
const VAULT_FOOTER: &[(&str, &str)] = &[
    ("j/k", "move"),
    ("a", "add"),
    ("e", "edit"),
    ("y", "copy"),
    ("d", "del"),
    ("Space", "reveal"),
    ("L", "lock"),
    ("Esc", "back"),
];

/// The footers the 80-column guard checks (see `footers_fit_80_cols`). These are
/// the long ones — the screens whose hint lists actually grow. Shorter footers
/// stay inline in [`draw_footer`]; if one of those gains hints, hoist it to a
/// const and list it here rather than letting it escape the guard (the earlier
/// two-entry version is how the Key-manager footer reached 82 cols unnoticed).
#[cfg(test)]
const ALL_FOOTERS: &[(&str, &[(&str, &str)])] = &[
    ("list", LIST_FOOTER),
    ("sftp browser", SFTP_BROWSER_FOOTER),
    ("key manager", KEY_MANAGER_FOOTER),
    ("vault", VAULT_FOOTER),
];

/// The non-modal screen rendered underneath the current screen (which may be a
/// modal overlay).
fn base_screen(app: &App) -> Screen {
    match &app.screen {
        Screen::Help
        | Screen::Confirm(_)
        | Screen::ActionMenu(_)
        | Screen::VaultUnlock
        | Screen::VaultRekey
        | Screen::ConnectOverride { .. }
        | Screen::SftpTransfer
        | Screen::DiffPreview
        | Screen::PassphraseSync
        | Screen::PasswordConfirm { .. }
        | Screen::KeyScan => app.prev_screen.clone().unwrap_or(Screen::List),
        Screen::PickKey { origin } | Screen::PickJump { origin } => match origin {
            PickOrigin::Edit { editing } => Screen::Edit { editing: *editing },
            // The override modal is itself an overlay over the list, so its
            // pickers render over the list, not a base form screen.
            PickOrigin::Override => Screen::List,
        },
        Screen::GenerateKey { origin } => match origin {
            GenOrigin::KeyManager => Screen::KeyManager,
            GenOrigin::EditForm { editing } => Screen::Edit { editing: *editing },
        },
        Screen::VaultEntry { .. } => Screen::Vault,
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
        Screen::Inspect => inspect::draw(f, app, body_a),
        Screen::Vault => vault::draw(f, app, body_a),
        Screen::SftpBrowser => sftp::draw_browser(f, app, body_a),
        _ => list::draw(f, app, body_a),
    }
    draw_footer(f, app, &base, footer_a);

    // Modal overlays on top of the base screen.
    match &app.screen {
        Screen::Help => help::draw(f, app, body_a),
        Screen::Confirm(action) => confirm::draw(f, app, action.clone(), body_a),
        Screen::ActionMenu(idx) => confirm::draw_action_menu(f, app, *idx, body_a),
        Screen::GenerateKey { .. } => keys::draw_wizard(f, app, body_a),
        Screen::PickKey { .. } => keys::draw_picker(f, app, body_a),
        Screen::PickJump { origin } => {
            // Clone so the immutable borrow of `app.screen` ends before the
            // `&mut app` render call (the picker needs the origin to exclude the
            // right "self" host from the candidate list).
            let origin = origin.clone();
            list::draw_jump_picker(f, app, &origin, body_a)
        }
        Screen::ConnectOverride { host } => connect_override::draw(f, app, *host, body_a),
        Screen::DiffPreview => diff::draw(f, app, body_a),
        Screen::SftpTransfer => sftp::draw_transfer(f, app, body_a),
        Screen::VaultUnlock => vault::draw_unlock(f, app, body_a),
        Screen::VaultRekey => vault::draw_rekey(f, app, body_a),
        Screen::VaultEntry { .. } => vault::draw_entry(f, app, body_a),
        Screen::PassphraseSync => keys::draw_passphrase_sync(f, app, body_a),
        Screen::PasswordConfirm { target, kinds, .. } => {
            vault::draw_password_confirm(f, target, *kinds, body_a)
        }
        Screen::KeyScan => keyscan::draw(f, app, body_a),
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
        Screen::Inspect => "Inspect",
        Screen::Vault => "Passwords",
        Screen::SftpBrowser => "SFTP browser",
        _ => "SSH Manager",
    };
    let count = match base {
        Screen::List => format!(
            "  {}/{} · {} ",
            app.filtered.len(),
            app.hosts.len(),
            app.sort.label()
        ),
        Screen::KeyManager => format!("  {} keys ", app.keys.len()),
        Screen::KnownHosts => format!("  {} entries ", app.known_hosts.len()),
        Screen::Inspect => format!("  {} · ssh -G ", app.inspect_alias),
        Screen::Vault => format!(
            "  {} secrets ",
            app.vault.as_ref().map(|v| v.entries.len()).unwrap_or(0)
        ),
        _ => String::new(),
    };
    // The override modal resolves its base to List (so the list renders behind
    // it), but the breadcrumb should name the modal, not show stale host counts.
    let (name, count) = if matches!(app.screen, Screen::ConnectOverride { .. }) {
        ("Connect override", String::new())
    } else if matches!(app.screen, Screen::KeyScan) {
        ("Scan host key", String::new())
    } else {
        (name, count)
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
    // The `[PATH ssh]` warning is security-relevant (an untrusted Git/MSYS `ssh` is
    // resolved), so it is pushed BEFORE the vault chip — on a narrow terminal the
    // cosmetic chip clips first, never the warning.
    if app.ssh_path_warning {
        spans.push(Span::styled(
            "  [PATH ssh]",
            Style::default().fg(theme::WARN),
        ));
    }
    // Vault status chip (List only): surfaces the otherwise-hidden `P` entry point
    // and a coarse lock/unlock state. The boolean lock state leaks no per-host
    // affiliation, so it is safe to show (unlike a per-host "has secret" cue).
    if matches!(base, Screen::List) {
        spans.push(Span::raw("   "));
        spans.push(Span::styled(
            "P",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ));
        let state = if app.vault.is_some() {
            Span::styled(" vault unlocked", Style::default().fg(theme::UP))
        } else if app.has_vault_file {
            Span::styled(" vault locked", Style::default().fg(theme::DIM))
        } else {
            Span::styled(" vault", Style::default().fg(theme::DIM))
        };
        spans.push(state);
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_footer(f: &mut Frame, app: &App, base: &Screen, area: Rect) {
    // The override modal resolves its base to List, so without this its chrome
    // footer would show stale list hints; surface the modal's real chords instead.
    if matches!(app.screen, Screen::ConnectOverride { .. }) {
        let hints = widgets::footer_hints(&[
            ("Tab", "field"),
            ("Enter", "edit/pick"),
            ("Space", "verbose"),
            ("^O", "connect"),
            ("^T", "new-tab"),
            ("^Y", "copy"),
            ("?", "help"),
            ("Esc", "cancel"),
        ]);
        f.render_widget(Paragraph::new(hints), area);
        return;
    }
    if matches!(app.screen, Screen::SftpTransfer) {
        let hints = widgets::footer_hints(&[
            ("Tab", "field"),
            ("Space/←→", "direction"),
            ("^S", "transfer"),
            ("Esc", "cancel"),
        ]);
        f.render_widget(Paragraph::new(hints), area);
        return;
    }
    // The scan modal resolves its base to the list; surface its own chords.
    if matches!(app.screen, Screen::KeyScan) {
        let hints = widgets::footer_hints(keyscan::keyscan_footer());
        f.render_widget(Paragraph::new(hints), area);
        return;
    }
    // The diff preview resolves its base to the Edit form, so without this its
    // footer would show the form's hints; surface the preview's real chords.
    if matches!(app.screen, Screen::DiffPreview) {
        let hints = widgets::footer_hints(&[
            ("Enter", "save"),
            ("j/k", "scroll"),
            ("Esc", "back to form"),
        ]);
        f.render_widget(Paragraph::new(hints), area);
        return;
    }
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
        (Screen::List, _) => widgets::footer_hints(LIST_FOOTER),
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
        (Screen::KeyManager, _) => widgets::footer_hints(KEY_MANAGER_FOOTER),
        (Screen::KnownHosts, a) if a.kh_searching => {
            widgets::footer_hints(&[("type", "filter"), ("Esc", "clear")])
        }
        (Screen::KnownHosts, _) => widgets::footer_hints(&[
            ("j/k", "move"),
            ("/", "search"),
            ("d", "delete"),
            ("Esc", "back"),
        ]),
        (Screen::Inspect, a) if a.inspect_searching => {
            widgets::footer_hints(&[("type", "filter"), ("Esc", "clear")])
        }
        (Screen::Inspect, _) => {
            widgets::footer_hints(&[("j/k", "move"), ("/", "filter"), ("Esc", "back")])
        }
        (Screen::Vault, _) => widgets::footer_hints(VAULT_FOOTER),
        (Screen::SftpBrowser, _) => widgets::footer_hints(SFTP_BROWSER_FOOTER),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footers_fit_80_cols() {
        // Footers render on a single, non-wrapping line; a >80-col footer silently
        // clips its trailing hints on an 80-column terminal (cmd.exe's default) —
        // the regression this guards against. Every screen's footer is listed here,
        // not just two: the earlier two-constant version let the Key-manager footer
        // grow to 82 cols (and the Vault one to 93) without failing (review #47).
        for (name, hints) in ALL_FOOTERS {
            let width = widgets::footer_hints(hints).width();
            assert!(width <= 80, "{name} footer is {width} cols");
        }
    }
}
