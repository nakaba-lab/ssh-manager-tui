//! Centralized color theme (Tokyo Night).
//!
//! `ui/` is pure rendering; this module only exposes palette colors and small
//! `Style` helpers — it never touches domain state. Routing every color choice
//! through here keeps the palette consistent across screens and gives a future
//! theme switch a single home.

use ratatui::style::{Color, Modifier, Style};

/// Base background of the palette. Used as a *foreground* when painting text on
/// a colored fill (e.g. the error toast, key-cap badges) for contrast.
pub const BG: Color = Color::Rgb(0x1a, 0x1b, 0x26);
/// Primary text.
pub const TEXT: Color = Color::Rgb(0xc0, 0xca, 0xf5);
/// Secondary text: labels and less-important info.
pub const DIM: Color = Color::Rgb(0x56, 0x5f, 0x89);
/// Tertiary: placeholders, inactive markers, faint separators.
pub const FAINT: Color = Color::Rgb(0x41, 0x48, 0x68);
/// Primary accent: focus, primary actions, app name, selection marker.
pub const ACCENT: Color = Color::Rgb(0x7a, 0xa2, 0xf7);
/// Secondary accent (e.g. the known_hosts cert/marker tag).
pub const ACCENT2: Color = Color::Rgb(0xbb, 0x9a, 0xf7);
/// Subtle dark fill: selected-row background, and the success-toast background.
pub const SEL_BG: Color = Color::Rgb(0x28, 0x34, 0x57);
/// Normal (unfocused) panel border.
pub const BORDER: Color = Color::Rgb(0x3b, 0x42, 0x61);
/// Liveness up / success.
pub const UP: Color = Color::Rgb(0x9e, 0xce, 0x6a);
/// Liveness down / error / destructive action.
pub const DOWN: Color = Color::Rgb(0xf7, 0x76, 0x8e);
/// Warning (e.g. the `[PATH ssh]` banner).
pub const WARN: Color = Color::Rgb(0xe0, 0xaf, 0x68);
/// Liveness checking.
pub const CHECKING: Color = Color::Rgb(0x7d, 0xcf, 0xff);

/// The left-edge marker drawn before a selected list/table row.
pub const SELECT_SYMBOL: &str = "▎ ";

/// Border color for a panel: accent when focused, the normal `BORDER` otherwise.
pub fn border(focused: bool) -> Color {
    if focused { ACCENT } else { BORDER }
}

/// Highlight style for a selected list/table row: subtle background, primary
/// text, bold. (The `▎` left bar comes from [`SELECT_SYMBOL`].)
pub fn selection() -> Style {
    Style::default()
        .bg(SEL_BG)
        .fg(TEXT)
        .add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_roles_are_truecolor() {
        // Guards against accidentally leaving a role as a named ANSI color.
        let roles = [
            BG, TEXT, DIM, FAINT, ACCENT, ACCENT2, SEL_BG, BORDER, UP, DOWN, WARN, CHECKING,
        ];
        for c in roles {
            assert!(
                matches!(c, Color::Rgb(..)),
                "every palette role must be truecolor RGB"
            );
        }
        // Anchor a couple of values so a careless palette edit is caught.
        assert_eq!(ACCENT, Color::Rgb(0x7a, 0xa2, 0xf7));
        assert_eq!(DOWN, Color::Rgb(0xf7, 0x76, 0x8e));
    }

    #[test]
    fn focused_border_uses_accent() {
        assert_eq!(border(true), ACCENT);
        assert_eq!(border(false), BORDER);
    }
}
