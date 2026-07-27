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

use crate::app::{App, KeyScanModal};
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

/// Randomart columns per row (each block is ~19 cols wide; 3 fit in 80).
const ART_COLS: usize = 3;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let Some(ks) = app.keyscan.as_ref() else {
        return;
    };
    let title = format!("Scan host key: {}", ks.alias);
    let text = Style::default().fg(theme::TEXT);
    let dim = Style::default().fg(theme::DIM);

    let mut lines: Vec<Line> = vec![Line::from("")];
    let mut danger = false;
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
        KeyScanModal::Results(rows) => {
            lines.push(Line::from(Span::styled(
                format!("{} — {} key(s) found", ks.target, rows.len()),
                text,
            )));
            lines.push(Line::from(""));
            let changed = rows.iter().any(|r| r.class == PinClass::Changed);
            for row in rows {
                let (chip, chip_style) = match row.class {
                    PinClass::New => ("[new]", Style::default().fg(theme::UP)),
                    PinClass::AlreadyTrusted => ("[already trusted]", dim),
                    PinClass::Changed => (
                        "[CHANGED]",
                        Style::default()
                            .fg(theme::DOWN)
                            .add_modifier(Modifier::BOLD),
                    ),
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{:<20} ", row.key.key_type), text),
                    Span::styled(format!("{} ", row.key.fingerprint), text),
                    Span::styled(chip, chip_style),
                ]));
            }
            if changed {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "CHANGED keys are never overwritten here — inspect Known hosts (H) instead.",
                    Style::default().fg(theme::WARN),
                )));
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
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        verify_hint(),
        Style::default().fg(theme::WARN),
    )));

    let content_w = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let modal = centered(
        (content_w + 4).clamp(46, area.width),
        lines.len() as u16 + 2,
        area,
    );
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
