//! Password vault: the secret list (base screen) plus the master-password
//! prompt and the add/edit-entry modals. Pure rendering — no domain mutation.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, List, ListItem, Paragraph};

use crate::app::App;

use super::theme;
use super::widgets::{centered, input_line, input_line_borrowed, modal_block, panel};

/// Mask a secret as a row of bullets, capped so very long secrets don't blow out
/// the layout.
fn masked(secret: &str) -> String {
    "•".repeat(secret.chars().count().clamp(1, 12))
}

/// Render a masked secret as an editable input line. `cursor` is a byte offset
/// into `value`; it is mapped onto the (multi-byte) bullet string so the caret
/// lands in the right cell.
fn masked_input(value: &str, cursor: usize, editing: bool) -> Line<'static> {
    let dots = "•".repeat(value.chars().count());
    if !editing {
        // Unfocused fields are rendered with this field's value but a *foreign*
        // cursor, so never slice here — just show the bullets.
        return input_line(&dots, 0, false);
    }
    // Clamp to a real char boundary so a multibyte value never panics the slice.
    let mut cur = cursor.min(value.len());
    while cur > 0 && !value.is_char_boundary(cur) {
        cur -= 1;
    }
    let chars_before = value[..cur].chars().count();
    input_line(&dots, chars_before * '•'.len_utf8(), editing)
}

/// The vault entry list (base screen).
pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(vault) = &app.vault else {
        // Shouldn't normally render locked, but stay defensive.
        let block = panel("Passwords", false);
        let line = Line::from(Span::styled(
            "  Vault is locked.",
            Style::default().fg(theme::DIM),
        ));
        f.render_widget(Paragraph::new(line).block(block), area);
        return;
    };

    if vault.entries.is_empty() {
        let block = panel("Passwords", true);
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No secrets stored yet.",
                Style::default().fg(theme::DIM),
            )),
            Line::from(Span::styled(
                "  Press 'a' to add a password or passphrase.",
                Style::default().fg(theme::FAINT),
            )),
        ];
        f.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
        return;
    }

    let items: Vec<ListItem> = vault
        .entries
        .iter()
        .map(|e| {
            // Reveal borrows the stored (already-scrubbed) secret directly;
            // never copy the plaintext onto the heap — a per-frame `to_string`
            // would be freed un-zeroized on every tick.
            let secret_span = if app.vault_reveal {
                Span::styled(e.secret.as_str(), Style::default().fg(theme::DIM))
            } else {
                Span::styled(masked(&e.secret), Style::default().fg(theme::DIM))
            };
            let mut spans = vec![
                Span::styled(
                    format!("{:<24}", e.host),
                    Style::default()
                        .fg(theme::TEXT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<11}", e.kind.label()),
                    Style::default().fg(theme::ACCENT2),
                ),
                secret_span,
            ];
            if !e.note.is_empty() {
                spans.push(Span::styled(
                    format!("   {}", e.note),
                    Style::default().fg(theme::FAINT),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let title = format!(
        "Passwords{}  ·  pw-autofill {}",
        if app.vault_reveal { "  (revealed)" } else { "" },
        if app.password_autofill_enabled {
            "on"
        } else {
            "off"
        },
    );
    let list = List::new(items)
        .block(panel(&title, true))
        .highlight_style(theme::selection())
        .highlight_symbol(theme::SELECT_SYMBOL);
    f.render_stateful_widget(list, area, &mut app.vault_state);
}

/// Master-password prompt modal (unlock, or create when no vault exists).
pub fn draw_unlock(f: &mut Frame, app: &App, area: Rect) {
    let u = &app.vault_unlock;
    let height = if u.creating { 11 } else { 9 };
    let modal = centered(60, height, area);
    f.render_widget(Clear, modal);

    let title = if u.creating {
        "Create vault"
    } else {
        "Unlock vault"
    };
    let block = modal_block(title, false);

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
    let mut lines = vec![Line::from("")];
    if u.creating {
        lines.push(Line::from(Span::styled(
            "  Set a master password to protect stored secrets.",
            Style::default().fg(theme::FAINT),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  Enter your master password.",
            Style::default().fg(theme::FAINT),
        )));
    }
    lines.push(Line::from(""));

    lines.push(Line::from({
        let mut s = vec![label("Password", u.field == 0)];
        s.extend(masked_input(&u.password, u.cursor, u.field == 0).spans);
        s
    }));
    if u.creating {
        lines.push(Line::from(""));
        lines.push(Line::from({
            let mut s = vec![label("Confirm", u.field == 1)];
            s.extend(masked_input(&u.confirm, u.cursor, u.field == 1).spans);
            s
        }));
    }
    lines.push(Line::from(""));
    let hint = if u.creating {
        "  Tab move · Enter create · Esc cancel"
    } else {
        "  Enter unlock · Esc cancel"
    };
    lines.push(Line::from(Span::styled(
        hint,
        Style::default().fg(theme::FAINT),
    )));

    f.render_widget(Paragraph::new(Text::from(lines)).block(block), modal);
}

/// One-time connect-time **password** consent modal. Shows the resolved
/// `<user@host>` the stored password would be sent to, framed as a consent/typo
/// guard (it is NOT a redirect/MITM defense — the listener's identity binding is).
/// Passphrase auto-fill needs no confirmation, so this only ever gates a password.
pub fn draw_password_confirm(
    f: &mut Frame,
    target: &str,
    kinds: crate::os::vault::MatchedKinds,
    area: Rect,
) {
    let modal = centered(64, 11, area);
    f.render_widget(Clear, modal);
    let block = modal_block("Send stored password?", false);

    let also_passphrase = if kinds.passphrase {
        "  (its passphrase auto-fills without asking)"
    } else {
        ""
    };
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Auto-fill your stored password for",
            Style::default().fg(theme::FAINT),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("      {target}"),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Confirm you meant this host (a typo guard, not a",
            Style::default().fg(theme::FAINT),
        )),
        Line::from(Span::styled(
            format!("  redirect defense).{also_passphrase}"),
            Style::default().fg(theme::FAINT),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Enter send · Esc skip (connect without it)",
            Style::default().fg(theme::FAINT),
        )),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), modal);
}

/// Add/edit entry modal.
pub fn draw_entry(f: &mut Frame, app: &App, area: Rect) {
    let e = &app.vault_entry;
    let modal = centered(64, 13, area);
    f.render_widget(Clear, modal);

    let title = if e.editing.is_some() {
        "Edit secret"
    } else {
        "Add secret"
    };
    let block = modal_block(title, false);

    let label = |s: &str, focused: bool| {
        Span::styled(
            format!("{s:<10}"),
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

    use crate::os::vault::SecretKind;
    let kind_line = Line::from(vec![
        label("Kind", e.field == 1),
        opt(e.kind == SecretKind::Password, "Password    "),
        opt(e.kind == SecretKind::Passphrase, "Passphrase"),
    ]);

    // The secret is masked unless reveal is on.
    let secret_line = Line::from({
        let mut s = vec![label("Secret", e.field == 2)];
        if app.vault_reveal {
            // Borrow the form's (scrubbed-on-drop) secret rather than cloning it
            // into per-frame spans that would leak plaintext onto the heap.
            s.extend(input_line_borrowed(&e.secret, e.cursor, e.field == 2).spans);
        } else {
            s.extend(masked_input(&e.secret, e.cursor, e.field == 2).spans);
        }
        s
    });

    let host_line = Line::from({
        let mut s = vec![label("Host", e.field == 0)];
        s.extend(input_line(&e.host, e.cursor, e.field == 0).spans);
        s
    });
    let note_line = Line::from({
        let mut s = vec![label("Note", e.field == 3)];
        s.extend(input_line(&e.note, e.cursor, e.field == 3).spans);
        s
    });

    let lines = vec![
        Line::from(""),
        host_line,
        Line::from(""),
        kind_line,
        Line::from(""),
        secret_line,
        Line::from(""),
        note_line,
        Line::from(""),
        Line::from(Span::styled(
            "  Tab move · Space toggle kind · Enter save · Esc cancel",
            Style::default().fg(theme::FAINT),
        )),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), modal);
}

#[cfg(test)]
mod tests {
    use super::masked_input;

    // Regression: the shared cursor is passed to UNFOCUSED fields too, so
    // masked_input must never slice a multibyte value at a foreign byte offset
    // (which would panic and crash the TUI). See the password-vault review.
    #[test]
    fn masked_input_never_panics_on_non_char_boundary() {
        let _ = masked_input("éz", 1, false); // unfocused, byte 1 is inside 'é'
        let _ = masked_input("é", 1, true); // focused, non-boundary cursor
        let _ = masked_input("🔐ab", 2, true); // cursor inside a 4-byte emoji
        let _ = masked_input("", 5, true); // cursor past the end of an empty value
        let _ = masked_input("abc", 99, false); // cursor far past the end
    }
}
