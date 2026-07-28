//! S3 — key manager: list of `~/.ssh/*.pub` keys with detail, and the
//! generate-key wizard (O4) modal.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, List, ListItem, Paragraph, Wrap};

use crate::app::App;
use crate::os::keys::{GenPassphrase, KeyType, PairStatus};

use super::theme;
use super::vault::masked_input;
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
    // Pair verification result — only shown when both halves exist.
    if let Some((text, color)) = pair_hint(k.pair) {
        lines.push(Line::from(vec![
            Span::styled(format!("{:>14}  ", "pair"), Style::default().fg(theme::DIM)),
            Span::styled(text, Style::default().fg(color)),
        ]));
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

/// Human-readable pairing hint for the key detail pane, with its accent color.
/// `None` when only one half exists (nothing to verify). Pure — message
/// selection only, so it stays unit-testable (rendering stays in
/// [`draw_detail`]).
fn pair_hint(pair: PairStatus) -> Option<(&'static str, Color)> {
    match pair {
        PairStatus::Matched => Some(("verified — public key matches private key", theme::UP)),
        PairStatus::Mismatched => Some((
            "MISMATCH — public key is not this private key's pair",
            theme::DOWN,
        )),
        PairStatus::Unverified => Some((
            "unverified — cannot verify without the passphrase (not an error)",
            theme::WARN,
        )),
        PairStatus::NotApplicable => None,
    }
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
    let modal = centered(64, 13, area);
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
    let passphrase_value = Line::from(vec![
        label("Passphrase", w.field == 3),
        opt(w.passphrase == GenPassphrase::NoPassphrase, "none      "),
        opt(w.passphrase == GenPassphrase::Interactive, "interactive"),
    ]);

    let lines = vec![
        Line::from(""),
        type_value,
        Line::from(""),
        file_value,
        Line::from(""),
        comment_value,
        Line::from(""),
        passphrase_value,
        Line::from(""),
        Line::from(Span::styled(
            "  Tab/↑↓ move · Space toggle · Enter generate · Esc cancel",
            Style::default().fg(theme::FAINT),
        )),
        Line::from(Span::styled(
            "  Saved to ~/.ssh/ · interactive: ssh-keygen prompts for it",
            Style::default().fg(theme::FAINT),
        )),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), modal);
}

/// How many affected hosts the sync modal names before eliding. The modal is a
/// fixed-height box, so an unbounded list would wrap past the input row and push
/// it out of the frame — one shared key can easily serve a dozen hosts.
const HOSTS_SHOWN: usize = 4;

/// The affected-host line: the first `max_shown` names, then `… +N more`. Pure,
/// so the elision that keeps the modal's input row on screen is unit-tested.
fn host_summary(hosts: &[String], max_shown: usize) -> String {
    if hosts.len() <= max_shown {
        return hosts.join(", ");
    }
    format!(
        "{}… +{} more",
        hosts[..max_shown]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        hosts.len() - max_shown
    )
}

/// Bulk vault-passphrase update modal (Issue #47), offered after `ssh-keygen -p`
/// succeeds while stored vault `Passphrase` entries still hold the OLD
/// passphrase for the changed key. One typed passphrase updates them all.
pub fn draw_passphrase_sync(f: &mut Frame, app: &App, area: Rect) {
    let form = &app.passphrase_sync;
    let modal = centered(64, 12, area);
    f.render_widget(Clear, modal);

    let block = modal_block("Update vault passphrases", false);

    let faint = |s: &'static str| Line::from(Span::styled(s, Style::default().fg(theme::FAINT)));
    let secret_value = Line::from({
        let mut s = vec![Span::styled(
            format!("{:<12}", "New pass"),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )];
        s.extend(masked_input(&form.secret, form.cursor, true).spans);
        s
    });

    let lines = vec![
        Line::from(""),
        faint("  The key's passphrase changed; these hosts' stored"),
        faint("  passphrases are stale and would auto-fill the old one:"),
        Line::from(Span::styled(
            format!("    {}", host_summary(&form.hosts, HOSTS_SHOWN)),
            Style::default().fg(theme::ACCENT),
        )),
        Line::from(""),
        secret_value,
        Line::from(""),
        faint("  Enter update all · Esc skip (entries stay stale)"),
    ];
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .block(block),
        modal,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// host_summary — ホストが増えても 1 行に収め、入力欄を枠外へ押し出さない（Issue #47）
    #[test]
    fn host_summary_elides_beyond_the_shown_limit() {
        // given
        let few: Vec<String> = vec!["web1".into(), "db".into()];
        let many: Vec<String> = (1..=9).map(|i| format!("web-prod-tokyo-{i:02}")).collect();

        // when / then: 収まるうちは全部見せる
        assert_eq!(host_summary(&few, 4), "web1, db");

        // when / then: 超えたら丸めて残数を示す（折り返しでモーダルが溢れない）
        let summary = host_summary(&many, 4);
        assert!(summary.ends_with("… +5 more"), "summary: {summary}");
        assert!(
            summary.starts_with("web-prod-tokyo-01, web-prod-tokyo-02"),
            "summary: {summary}"
        );
    }

    /// pair_hint — Unverified は「暗号化鍵はパスフレーズ無しで検証できない・エラーではない」旨を説明する（Issue #47）
    #[test]
    fn pair_hint_unverified_explains_encrypted_keys() {
        // given
        let pair = PairStatus::Unverified;

        // when
        let (text, _color) = pair_hint(pair).expect("Unverified must render a hint");

        // then
        assert!(
            text.contains("passphrase"),
            "should mention passphrase-protected (encrypted) keys: {text}"
        );
        assert!(
            text.contains("not an error"),
            "should reassure this state is not an error: {text}"
        );
    }
}
