//! Small shared rendering helpers used across screens.

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Padding};

use crate::os::liveness::Liveness;

use super::theme;

/// Minimum terminal width (columns) at which a two-pane screen lays out
/// side-by-side. Below this, the two panes stack vertically. Tunable.
pub const WIDE_MIN_WIDTH: u16 = 90;

/// Split `area` into two panes. When `area` is at least [`WIDE_MIN_WIDTH`]
/// columns wide the panes sit side-by-side (`side_pct` = the primary pane's
/// width %); otherwise they stack vertically (`stack_pct` = the primary pane's
/// height %). Returns `(primary, secondary)`.
pub fn responsive_split(area: Rect, side_pct: u16, stack_pct: u16) -> (Rect, Rect) {
    debug_assert!(side_pct <= 100, "side_pct {side_pct} > 100");
    debug_assert!(stack_pct <= 100, "stack_pct {stack_pct} > 100");
    if area.width >= WIDE_MIN_WIDTH {
        let cols = Layout::horizontal([
            Constraint::Percentage(side_pct),
            Constraint::Percentage(100 - side_pct),
        ])
        .split(area);
        (cols[0], cols[1])
    } else {
        let rows = Layout::vertical([
            Constraint::Percentage(stack_pct),
            Constraint::Percentage(100 - stack_pct),
        ])
        .split(area);
        (rows[0], rows[1])
    }
}

/// A dim, bold section sub-heading used to group fields in the detail pane and
/// the edit form. Indented two columns so it sits slightly in from the panel's
/// content edge (the panel adds one padding column; this adds one more).
pub fn section_header(title: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {title}"),
        Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD),
    ))
}

/// A centred rectangle `width` x `height` (absolute cells) within `area`.
pub fn centered(width: u16, height: u16, area: Rect) -> Rect {
    let [h] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [v] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(h);
    v
}

/// A centred rectangle sized as a percentage of `area`.
pub fn centered_pct(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let [h] = Layout::horizontal([Constraint::Percentage(pct_x)])
        .flex(Flex::Center)
        .areas(area);
    let [v] = Layout::vertical([Constraint::Percentage(pct_y)])
        .flex(Flex::Center)
        .areas(h);
    v
}

/// A styled glyph for a liveness state.
pub fn liveness_span(state: Liveness) -> Span<'static> {
    let (color, modifier) = match state {
        Liveness::Up => (theme::UP, Modifier::BOLD),
        Liveness::Down => (theme::DOWN, Modifier::empty()),
        Liveness::Checking => (theme::CHECKING, Modifier::empty()),
        Liveness::Skipped => (theme::FAINT, Modifier::empty()),
        Liveness::Unknown => (theme::FAINT, Modifier::empty()),
    };
    Span::styled(
        state.glyph(),
        Style::default().fg(color).add_modifier(modifier),
    )
}

/// Render an editable text value as a `Line`, drawing a reverse-video caret at
/// the (byte) cursor position when `editing`. Value text uses `theme::TEXT` so
/// it renders consistently regardless of the terminal's default foreground.
pub fn input_line(value: &str, cursor: usize, editing: bool) -> Line<'static> {
    let text = Style::default().fg(theme::TEXT);
    if !editing {
        return Line::from(Span::styled(value.to_string(), text));
    }
    // Clamp to a real char boundary so a stray cursor never panics the split.
    let mut cursor = cursor.min(value.len());
    while cursor > 0 && !value.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let (before, after) = value.split_at(cursor);
    let (cur_ch, rest) = match after.chars().next() {
        Some(c) => (c.to_string(), after[c.len_utf8()..].to_string()),
        None => (" ".to_string(), String::new()),
    };
    Line::from(vec![
        Span::styled(before.to_string(), text),
        Span::styled(cur_ch, text.add_modifier(Modifier::REVERSED)),
        Span::styled(rest, text),
    ])
}

/// Like [`input_line`] but **borrows** `value` instead of cloning it into owned
/// spans. Use for a sensitive value (a revealed secret): the plaintext stays in
/// the caller's already-scrubbed buffer rather than being copied onto the heap
/// each frame where the per-frame copy would be freed un-zeroized.
pub fn input_line_borrowed(value: &str, cursor: usize, editing: bool) -> Line<'_> {
    let text = Style::default().fg(theme::TEXT);
    if !editing {
        return Line::from(Span::styled(value, text));
    }
    // Clamp to a real char boundary so a stray cursor never panics the split.
    let mut cursor = cursor.min(value.len());
    while cursor > 0 && !value.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let (before, after) = value.split_at(cursor);
    let (cur_ch, rest) = match after.chars().next() {
        Some(c) => after.split_at(c.len_utf8()),
        None => (" ", ""),
    };
    Line::from(vec![
        Span::styled(before, text),
        Span::styled(cur_ch, text.add_modifier(Modifier::REVERSED)),
        Span::styled(rest, text),
    ])
}

/// A right-aligned `key  value` detail line (14-wide dim key, primary value).
pub fn kv_line(key: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:>14}  "), Style::default().fg(theme::DIM)),
        Span::styled(value, Style::default().fg(theme::TEXT)),
    ])
}

/// Build a footer hint line from `(key, label)` pairs: accent keys, dim labels,
/// faint `·` separators.
pub fn footer_hints(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, (key, label)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(theme::FAINT)));
        }
        spans.push(Span::styled(
            key.to_string(),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {label}"),
            Style::default().fg(theme::DIM),
        ));
    }
    Line::from(spans)
}

/// A rounded content panel: dim border (accent when focused), accent/dim bold
/// title, and 1-cell horizontal padding for breathing room. `title` is given
/// without surrounding spaces; this adds them.
pub fn panel(title: &str, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::border(focused)))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(if focused { theme::ACCENT } else { theme::DIM })
                .add_modifier(Modifier::BOLD),
        ))
}

/// A rounded modal/overlay block: accent border + title, or `down` (red) when
/// the modal confirms a destructive action. No padding — modal bodies manage
/// their own spacing.
pub fn modal_block(title: &str, danger: bool) -> Block<'static> {
    let color = if danger { theme::DOWN } else { theme::ACCENT };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responsive_split_is_side_by_side_when_wide() {
        let area = Rect::new(0, 0, WIDE_MIN_WIDTH, 20);
        let (a, b) = responsive_split(area, 60, 50);
        // Same height as the area, widths tile it, secondary sits to the right.
        assert_eq!(a.height, area.height);
        assert_eq!(b.height, area.height);
        assert_eq!(a.y, area.y);
        assert_eq!(b.y, area.y);
        assert_eq!(a.width + b.width, area.width);
        assert!(b.x > a.x);
    }

    #[test]
    fn responsive_split_stacks_when_narrow() {
        let area = Rect::new(0, 0, WIDE_MIN_WIDTH - 1, 20);
        let (a, b) = responsive_split(area, 60, 50);
        // Same width as the area, heights tile it, secondary sits below.
        assert_eq!(a.width, area.width);
        assert_eq!(b.width, area.width);
        assert_eq!(a.x, area.x);
        assert_eq!(b.x, area.x);
        assert_eq!(a.height + b.height, area.height);
        assert!(b.y > a.y);
    }
}
