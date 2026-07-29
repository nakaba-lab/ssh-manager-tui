//! Host-key scan modal (#46) — scanning / results / error views.
//!
//! Pure drawing of [`crate::app::KeyScanUi`]. The verification wording (AC8)
//! is shown in EVERY state — keyscan runs over the same channel as the
//! connection, so it is NOT out-of-band verification and the modal must never
//! present it as such.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::{App, KeyScanModal, PinBlocked};
use crate::os::keyscan::PinClass;

use super::theme;
use super::widgets::{centered, modal_block};

/// Footer hints for the scan modal, in `widgets::footer_hints` pair form.
/// Must render within 80 columns (see `keyscan_footer_fits_80_cols_...`).
pub fn keyscan_footer() -> &'static [(&'static str, &'static str)] {
    &[("y", "pin new keys"), ("Esc", "cancel")]
}

/// The verification reminder shown in EVERY modal state (#46 AC8): direct
/// the user to verify the fingerprints against a trusted source (server
/// console / provider docs) before pinning.
pub fn verify_hint() -> &'static str {
    "Verify against a trusted source (server console / provider docs) before pinning."
}

/// Where an approved pin lands — the destination FILE and the host token it is
/// recorded under. Both matter: the file is variable (the host's effective
/// `UserKnownHostsFile`), and the token is not always the scanned target —
/// `HostKeyAlias` records one pin that covers every host sharing that alias, so
/// showing only `host:port` understates how far approving here reaches
/// (#46 round 13).
///
/// Two lines, not one: `Paragraph` clips rather than wraps here, so a single
/// line makes each fact hostage to the other's length — a long lookup key
/// pushed the file name off the right edge entirely (#46 round 14).
///
/// The name comes FIRST so the destination file is the survivor: `fit_body`
/// trims the tail from the front, and the file is the more load-bearing fact —
/// it is what lets the user spot an implausible write target before pressing
/// `y` (#46 round 15).
pub fn pin_destination(lookup_key: &str, target: &std::path::Path) -> [String; 2] {
    [
        format!("Pins are recorded as {lookup_key}"),
        format!("  and written to {}", target.display()),
    ]
}

/// Randomart columns per row (each block is ~19 cols wide; 3 fit in 80).
const ART_COLS: usize = 3;

/// Preferred modal width when the content is narrower than this.
const MIN_MODAL_WIDTH: u16 = 46;

/// Modal size for `content` within `area`, never exceeding it. `centered`
/// clamps the width down, but the height must be clamped here so the body can
/// be trimmed to match (`u16::clamp` would panic outright on a terminal
/// narrower than the preferred width — #46 review).
fn modal_size(content_w: u16, content_h: u16, area: Rect) -> (u16, u16) {
    (
        content_w.max(MIN_MODAL_WIDTH).min(area.width),
        content_h.min(area.height),
    )
}

/// Fit `body` + `tail` into `rows`, dropping `body` from the END first so the
/// trailing verification reminder (#46 AC8) is never what scrolls off — the
/// randomart is the supplementary part, the "verify out-of-band" wording is
/// not. When even the tail does not fit, it is trimmed from the FRONT so the
/// reminder (its last line) is the very last thing lost: returning more lines
/// than `rows` would let `Paragraph` clip the bottom, which is exactly where
/// the reminder sits (#46 final review).
fn fit_body(
    mut body: Vec<Line<'static>>,
    mut tail: Vec<Line<'static>>,
    rows: usize,
) -> Vec<Line<'static>> {
    if tail.len() > rows {
        tail.drain(..tail.len() - rows);
    }
    let budget = rows.saturating_sub(tail.len());
    if body.len() > budget {
        body.truncate(budget);
    }
    body.extend(tail);
    body
}

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let Some(ks) = app.keyscan.as_ref() else {
        return;
    };
    let title = format!("Scan host key: {}", ks.alias);
    let text = Style::default().fg(theme::TEXT);
    let dim = Style::default().fg(theme::DIM);

    let mut lines: Vec<Line> = vec![Line::from("")];
    let mut danger = false;
    let mut blocked_reason: Option<PinBlocked> = None;
    match &ks.modal {
        KeyScanModal::Scanning => {
            lines.push(Line::from(Span::styled(
                format!("scanning {} …", ks.target),
                text,
            )));
        }
        KeyScanModal::Error(msg) => {
            danger = true;
            lines.push(Line::from(Span::styled(
                format!("scan failed: {msg}"),
                Style::default().fg(theme::DOWN),
            )));
        }
        KeyScanModal::Results { rows, blocked } => {
            lines.push(Line::from(Span::styled(
                format!("{} — {} key(s) found", ks.target, rows.len()),
                text,
            )));
            lines.push(Line::from(""));
            blocked_reason = *blocked;
            for row in rows {
                let danger_chip = Style::default()
                    .fg(theme::DOWN)
                    .add_modifier(Modifier::BOLD);
                let (chip, chip_style) = match row.class {
                    PinClass::New => ("[new]", Style::default().fg(theme::UP)),
                    PinClass::AlreadyTrusted => ("[already trusted]", dim),
                    PinClass::Changed => ("[CHANGED]", danger_chip),
                    PinClass::Revoked => ("[REVOKED]", danger_chip),
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{:<20} ", row.key.key_type), text),
                    Span::styled(format!("{} ", row.key.fingerprint), text),
                    Span::styled(chip, chip_style),
                ]));
            }
            // Randomart blocks, side by side (ART_COLS per band).
            for band in rows.chunks(ART_COLS) {
                let art_rows = band
                    .iter()
                    .map(|r| r.key.randomart.len())
                    .max()
                    .unwrap_or(0);
                if art_rows == 0 {
                    continue;
                }
                lines.push(Line::from(""));
                let width = band
                    .iter()
                    .flat_map(|r| r.key.randomart.iter().map(|l| l.chars().count()))
                    .max()
                    .unwrap_or(0);
                for i in 0..art_rows {
                    let joined = band
                        .iter()
                        .map(|r| {
                            let cell = r.key.randomart.get(i).map(String::as_str).unwrap_or("");
                            format!("{cell:<width$}")
                        })
                        .collect::<Vec<_>>()
                        .join("  ");
                    lines.push(Line::from(Span::styled(joined, dim)));
                }
            }
        }
    }
    // The tail is never trimmed: the AC8 reminder and the "pinning is disabled"
    // banner are both must-not-scroll-off content, so neither may sit in the
    // body where `fit_body` could cut it on a short terminal (#46 re-review).
    let mut tail = vec![Line::from("")];
    if let Some(reason) = blocked_reason {
        let (headline, detail) = match reason {
            PinBlocked::Contradicted => (
                "Pinning is DISABLED — these keys contradict a pin you already trust.",
                "Nothing is overwritten here. Inspect Known hosts (H) before trusting this host.",
            ),
            PinBlocked::AlreadyPinned => (
                "Pinning is DISABLED — this host is already pinned.",
                "A scan cannot prove who answered it, so no key is added beside an existing pin.",
            ),
        };
        tail.push(Line::from(Span::styled(
            headline,
            Style::default()
                .fg(theme::DOWN)
                .add_modifier(Modifier::BOLD),
        )));
        tail.push(Line::from(Span::styled(
            detail,
            Style::default().fg(theme::WARN),
        )));
    } else if matches!(ks.modal, KeyScanModal::Results { .. }) {
        for line in pin_destination(&ks.lookup_key, &ks.pin_target) {
            tail.push(Line::from(Span::styled(line, dim)));
        }
    }
    tail.push(Line::from(Span::styled(
        verify_hint(),
        Style::default().fg(theme::WARN),
    )));

    let content_w = lines
        .iter()
        .chain(tail.iter())
        .map(Line::width)
        .max()
        .unwrap_or(0) as u16;
    let (width, height) = modal_size(content_w + 4, (lines.len() + tail.len()) as u16 + 2, area);
    // Two rows go to the block's top/bottom border.
    let lines = fit_body(lines, tail, height.saturating_sub(2) as usize);

    let modal = centered(width, height, area);
    f.render_widget(Clear, modal);
    f.render_widget(
        Paragraph::new(Text::from(lines)).block(modal_block(&title, danger)),
        modal,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::widgets;

    #[test]
    fn keyscan_footer_fits_80_cols_and_offers_pin_and_cancel() {
        // given — the scan modal footer
        let footer = keyscan_footer();
        // when — rendered through the shared footer widget
        let line = widgets::footer_hints(footer);
        // then — fits the 80-column floor and offers both exits
        assert!(
            line.width() <= 80,
            "keyscan footer is {} cols",
            line.width()
        );
        let keys: Vec<&str> = footer.iter().map(|(k, _)| *k).collect();
        assert!(
            keys.contains(&"y"),
            "footer must offer y (pin), got {keys:?}"
        );
        assert!(
            keys.contains(&"Esc"),
            "footer must offer Esc (cancel), got {keys:?}"
        );
    }

    #[test]
    fn modal_size_never_exceeds_a_narrow_terminal() {
        // given — a terminal narrower than the modal's preferred width, which a
        // `u16::clamp(46, area.width)` would panic on (#46 review)
        let area = Rect::new(0, 0, 40, 12);
        // when — sizing content that wants to be wider than the terminal
        let (w, h) = modal_size(80, 30, area);
        // then — clamped down to what actually fits, no panic
        assert_eq!((w, h), (40, 12));

        // given / when — a terminal roomier than the content
        let (w, h) = modal_size(20, 6, Rect::new(0, 0, 100, 40));
        // then — floored at the preferred minimum width
        assert_eq!((w, h), (MIN_MODAL_WIDTH, 6));
    }

    #[test]
    fn fit_body_drops_randomart_before_the_verify_hint() {
        // given — a body far taller than the modal (many randomart rows) and
        // the AC8 reminder pinned to the tail
        let body: Vec<Line<'static>> = (0..40).map(|i| Line::from(format!("art {i}"))).collect();
        let tail = vec![Line::from(""), Line::from(verify_hint())];
        // when — fitted into 10 rows
        let fitted = fit_body(body, tail, 10);
        // then — the reminder survives; the art is what got cut
        assert_eq!(fitted.len(), 10);
        assert_eq!(fitted.last().unwrap().to_string(), verify_hint());
        assert!(
            fitted
                .iter()
                .filter(|l| l.to_string().starts_with("art "))
                .count()
                < 40,
            "randomart should be the part that is trimmed"
        );
    }

    #[test]
    fn fit_body_keeps_the_hint_even_with_no_room_for_the_body() {
        // given — a modal so short only the tail can fit
        let body = vec![Line::from("row")];
        let tail = vec![Line::from(verify_hint())];
        // when
        let fitted = fit_body(body, tail, 1);
        // then — the reminder is what is kept (AC8 is not optional)
        assert_eq!(fitted.len(), 1);
        assert_eq!(fitted[0].to_string(), verify_hint());
    }

    #[test]
    fn pin_destination_names_the_host_token_not_just_the_file() {
        // given — a host whose pins are recorded under a lookup key that is NOT
        // the scanned target: `HostKeyAlias shared-pool` makes one pin cover
        // every host that shares the alias, so showing only the file left the
        // user unable to see how far the approval reaches (#46 round 13)
        let lines = pin_destination(
            "shared-pool",
            std::path::Path::new("/home/u/.ssh/known_hosts"),
        );
        // then — both the host token written and the destination file are shown,
        // and on SEPARATE lines: this widget clips instead of wrapping, so a
        // long lookup key on a shared line pushes the file off the right edge
        // and the user is left with neither fact (#46 round 14)
        assert!(
            lines[0].contains("shared-pool"),
            "first line must name the host token written, got: {lines:?}"
        );
        assert!(
            !lines[0].contains("/home/u/.ssh/known_hosts"),
            "the file must not share a clippable line with the token, got: {lines:?}"
        );
        // and the FILE is last, so the tail's front-trim keeps it: it is what
        // lets the user spot an implausible write target before pressing `y`
        // (#46 round 15)
        assert!(
            lines[1].contains("/home/u/.ssh/known_hosts"),
            "last line must name the file written, got: {lines:?}"
        );
    }

    #[test]
    fn keyscan_verify_hint_points_at_trusted_source() {
        // given / when — the always-visible wording (AC8)
        let hint = verify_hint().to_lowercase();
        // then — it must direct the user to out-of-band verification, not
        // present the same-channel scan as proof of authenticity
        assert!(
            hint.contains("verify"),
            "hint must ask the user to verify, got: {hint}"
        );
        assert!(
            hint.contains("trusted source"),
            "hint must name a trusted source to check against, got: {hint}"
        );
    }
}
