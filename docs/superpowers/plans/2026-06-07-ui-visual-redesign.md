# UI Visual Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restyle the `sshm` TUI to a designed Tokyo Night palette with refined chrome (borders, selection, title bar, footer, toasts), centralizing all color choices in a new `ui/theme.rs`.

**Architecture:** Introduce `ui/theme.rs` exposing the Tokyo Night palette as semantic-role color constants plus small `Style` helpers. Add two composite block builders (`panel`, `modal_block`) to `ui/widgets.rs`. Then sweep every `ui/` file, replacing hardcoded `Color::*` with theme roles and adopting the shared helpers. Pure rendering change — no domain logic, keybindings, or layout ratios change; `config/` and `os/` are untouched.

**Tech Stack:** Rust 2024, ratatui (`Color::Rgb` truecolor, `Block`, `Padding`, `List`/`Table` highlight styling).

---

## Testing note (read first)

This is a pure-rendering change. Per `CLAUDE.md`, the test suite lives in `config/` and `os/`; the `ui/` layer is not unit-tested for drawing. So:

- **`ui/theme.rs` is the one unit-testable unit** — it gets a real failing-test-first cycle (palette roles are distinct truecolor; focused border resolves to accent).
- **All rendering tasks are verified by `cargo build` (must compile) + a manual visual run**, not unit tests. Do **not** invent rendering unit tests.
- **`cargo clippy -- -D warnings` is deferred to the final task.** Intermediate tasks add `pub` theme items before they are all wired up; in a binary crate those emit `dead_code` *warnings* until consumed. `cargo build` still succeeds (warnings, not errors). Only Task 9 runs clippy with `-D warnings`, after everything is wired.

Branch is already `feature/ui-visual-redesign` (the design spec was committed there).

## File Structure

- **Create** `src/ui/theme.rs` — palette color constants by semantic role + style helpers (`border`, `selection`, `SELECT_SYMBOL`). One responsibility: the color theme.
- **Modify** `src/ui/mod.rs` — register `mod theme`; restyle title bar (breadcrumb), toast; footer already routes through `widgets::footer_hints`.
- **Modify** `src/ui/widgets.rs` — add `panel()` and `modal_block()` block builders; rewrite `footer_hints`, `liveness_span`, `kv_line` to use theme.
- **Modify** `src/ui/list.rs`, `src/ui/edit.rs`, `src/ui/keys.rs`, `src/ui/known_hosts.rs`, `src/ui/confirm.rs`, `src/ui/help.rs` — adopt theme + helpers per screen.
- **Untouched** `config/`, `os/`, `app.rs`, `update.rs`, `event_loop.rs`.

Dependency rule preserved: `theme` is internal to `ui/`; nothing in `config/`/`os/` references it.

---

## Task 1: Create the theme module

**Files:**
- Create: `src/ui/theme.rs`
- Modify: `src/ui/mod.rs` (add module declaration)

- [ ] **Step 1: Write `src/ui/theme.rs` with the palette, helpers, and a failing test**

Create `src/ui/theme.rs`:

```rust
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
/// Secondary accent.
pub const ACCENT2: Color = Color::Rgb(0xbb, 0x9a, 0xf7);
/// Selected-row background.
pub const SEL_BG: Color = Color::Rgb(0x28, 0x34, 0x57);
/// Normal (unfocused) panel border.
pub const BORDER: Color = Color::Rgb(0x3b, 0x42, 0x61);
/// liveness up / success.
pub const UP: Color = Color::Rgb(0x9e, 0xce, 0x6a);
/// liveness down / error / destructive action.
pub const DOWN: Color = Color::Rgb(0xf7, 0x76, 0x8e);
/// Warning (e.g. the `[PATH ssh]` banner).
pub const WARN: Color = Color::Rgb(0xe0, 0xaf, 0x68);
/// liveness checking.
pub const CHECKING: Color = Color::Rgb(0x7d, 0xcf, 0xff);

/// The left-edge marker drawn before a selected list/table row.
pub const SELECT_SYMBOL: &str = "▎ ";

/// Border color for a panel: accent when focused, dim border otherwise.
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
```

- [ ] **Step 2: Register the module**

In `src/ui/mod.rs`, the module declarations currently read:

```rust
pub mod confirm;
pub mod edit;
pub mod help;
pub mod keys;
pub mod known_hosts;
pub mod list;
pub mod widgets;
```

Add `pub mod theme;` in alphabetical position so the block becomes:

```rust
pub mod confirm;
pub mod edit;
pub mod help;
pub mod keys;
pub mod known_hosts;
pub mod list;
pub mod theme;
pub mod widgets;
```

- [ ] **Step 3: Run the theme tests — expect PASS**

Run: `cargo test theme`
Expected: the two tests in `ui::theme::tests` pass. (They are written to pass against the code in Step 1; this confirms the module compiles and the palette is wired.)

- [ ] **Step 4: Build — expect success with dead_code warnings**

Run: `cargo build`
Expected: compiles. Warnings about unused `theme::*` items are expected and fine — they get consumed in later tasks.

- [ ] **Step 5: Commit**

```bash
git add src/ui/theme.rs src/ui/mod.rs
git commit -m "feat(ui): add Tokyo Night theme module"
```

---

## Task 2: Shared widget helpers

**Files:**
- Modify: `src/ui/widgets.rs`

- [ ] **Step 1: Update imports**

Replace the top imports of `src/ui/widgets.rs`:

```rust
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::os::liveness::Liveness;
```

with:

```rust
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Padding};

use crate::os::liveness::Liveness;

use super::theme;
```

- [ ] **Step 2: Add the `panel` and `modal_block` builders**

Append to `src/ui/widgets.rs`:

```rust
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
```

- [ ] **Step 3: Recolor `liveness_span`**

Replace the existing `liveness_span` function with:

```rust
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
```

- [ ] **Step 4: Recolor `kv_line`**

Replace the existing `kv_line` function with:

```rust
/// A right-aligned `key  value` detail line (14-wide dim key, primary value).
pub fn kv_line(key: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:>14}  "), Style::default().fg(theme::DIM)),
        Span::styled(value, Style::default().fg(theme::TEXT)),
    ])
}
```

- [ ] **Step 5: Rewrite `footer_hints` (flat, badge-free)**

Replace the existing `footer_hints` function with:

```rust
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
```

Note: `input_line`, `centered`, and `centered_pct` are unchanged (the caret stays `Modifier::REVERSED`, which is palette-agnostic).

- [ ] **Step 6: Build — expect success**

Run: `cargo build`
Expected: compiles (dead_code warnings for not-yet-used theme items still allowed).

- [ ] **Step 7: Commit**

```bash
git add src/ui/widgets.rs
git commit -m "feat(ui): theme-aware shared widgets (panel, modal_block, footer)"
```

---

## Task 3: Title bar and toast (`ui/mod.rs`)

**Files:**
- Modify: `src/ui/mod.rs`

- [ ] **Step 1: Update imports**

The current imports include:

```rust
use ratatui::style::{Color, Modifier, Style};
```

Replace that single line with:

```rust
use ratatui::style::{Modifier, Style};

use super::theme;
```

(`use crate::app::{App, GenOrigin, Screen};` and the other `use` lines stay.)

- [ ] **Step 2: Rewrite `draw_title` to a breadcrumb**

Replace the entire `draw_title` function with:

```rust
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
```

- [ ] **Step 3: Rewrite `draw_toast`**

Replace the entire `draw_toast` function with:

```rust
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
```

`draw_footer` needs no change — it already builds pairs and calls `widgets::footer_hints`, which Task 2 restyled.

- [ ] **Step 4: Build — expect success**

Run: `cargo build`
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add src/ui/mod.rs
git commit -m "feat(ui): breadcrumb title bar and themed toasts"
```

---

## Task 4: Host list screen (`ui/list.rs`)

**Files:**
- Modify: `src/ui/list.rs`

- [ ] **Step 1: Update imports**

Replace the current imports:

```rust
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Row, Table, Wrap,
};

use crate::app::{App, ListFocus};
use crate::os::liveness::Liveness;

use super::widgets::{centered, input_line, kv_line, liveness_span};
```

with:

```rust
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, List, ListItem, Paragraph, Row, Table, Wrap};

use crate::app::{App, ListFocus};
use crate::os::liveness::Liveness;

use super::theme;
use super::widgets::{centered, input_line, kv_line, liveness_span, modal_block, panel};
```

- [ ] **Step 2: Rewrite `draw_empty`**

Replace the entire `draw_empty` function with:

```rust
fn draw_empty(f: &mut Frame, app: &App, area: Rect) {
    let block = panel("No hosts yet", false);
    let accent_key = |k: &str| {
        Span::styled(
            k.to_string(),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    };
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "~/.ssh/config has no Host entries.",
            Style::default().fg(theme::DIM),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Press ", Style::default().fg(theme::DIM)),
            accent_key("a"),
            Span::styled(" to add your first host, ", Style::default().fg(theme::DIM)),
            accent_key("K"),
            Span::styled(" to manage keys, ", Style::default().fg(theme::DIM)),
            accent_key("?"),
            Span::styled(" for help.", Style::default().fg(theme::DIM)),
        ]),
    ];
    if app.include_note {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Note: Include directives present — hosts in included files are not shown.",
            Style::default().fg(theme::WARN),
        )));
    }
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
}
```

- [ ] **Step 3: Rewrite `draw_list_pane`**

Replace the entire `draw_list_pane` function with:

```rust
fn draw_list_pane(f: &mut Frame, app: &mut App, area: Rect) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    draw_search(f, app, rows[0]);

    let focused = app.focus == ListFocus::Hosts;
    let header = Row::new(["", "Alias", "HostName", "User"]).style(
        Style::default()
            .fg(theme::DIM)
            .add_modifier(Modifier::BOLD),
    );

    let table_rows: Vec<Row> = app
        .filtered
        .iter()
        .filter_map(|&i| app.hosts.get(i).map(|h| (i, h)))
        .map(|(i, h)| {
            let state = app.liveness_by_index(i);
            Row::new(vec![
                Line::from(liveness_span(state)),
                Line::from(h.alias().to_string()),
                Line::from(h.host_name.clone().unwrap_or_else(|| "—".into())),
                Line::from(h.user.clone().unwrap_or_else(|| "—".into())),
            ])
        })
        .collect();

    let table = Table::new(
        table_rows,
        [
            Constraint::Length(2),
            Constraint::Percentage(40),
            Constraint::Percentage(38),
            Constraint::Percentage(22),
        ],
    )
    .header(header)
    .block(panel("Hosts", focused))
    .row_highlight_style(theme::selection())
    .highlight_symbol(theme::SELECT_SYMBOL);

    f.render_stateful_widget(table, rows[1], &mut app.list_state);
}
```

- [ ] **Step 4: Rewrite `draw_search`**

Replace the entire `draw_search` function with:

```rust
fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let prefix = Span::styled(
        " / ",
        if app.searching {
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::FAINT)
        },
    );
    let body = input_line(&app.search, app.search.len(), app.searching);
    let mut spans = vec![prefix];
    spans.extend(body.spans);
    if app.search.is_empty() && !app.searching {
        spans.push(Span::styled(
            "search (alias / hostname / user)",
            Style::default().fg(theme::FAINT),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
```

- [ ] **Step 5: Rewrite `draw_jump_picker`**

Replace the entire `draw_jump_picker` function with:

```rust
/// Host picker modal, opened from the edit form's ProxyJump field.
pub fn draw_jump_picker(f: &mut Frame, app: &mut App, area: Rect) {
    let candidates = app.jump_candidates();
    let modal = centered(60, 14, area);
    f.render_widget(Clear, modal);

    let block = modal_block("Pick ProxyJump host", false);

    if candidates.is_empty() {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No other registered hosts to jump through.",
                Style::default().fg(theme::DIM),
            )),
            Line::from(Span::styled(
                "  Add a host first, or type the target manually (Esc).",
                Style::default().fg(theme::FAINT),
            )),
        ];
        f.render_widget(Paragraph::new(Text::from(lines)).block(block), modal);
        return;
    }

    let inner = block.inner(modal);
    f.render_widget(block, modal);
    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);

    let items: Vec<ListItem> = candidates
        .iter()
        .filter_map(|&i| app.hosts.get(i))
        .map(|h| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    h.alias().to_string(),
                    Style::default()
                        .fg(theme::TEXT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}", h.host_name.clone().unwrap_or_else(|| "—".into())),
                    Style::default().fg(theme::DIM),
                ),
                Span::styled(
                    h.user
                        .clone()
                        .map(|u| format!("  ({u})"))
                        .unwrap_or_default(),
                    Style::default().fg(theme::DIM),
                ),
            ]))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(theme::selection())
        .highlight_symbol(theme::SELECT_SYMBOL);
    f.render_stateful_widget(list, rows[0], &mut app.pick_jump_state);

    f.render_widget(
        Paragraph::new(Span::styled(
            " j/k move · Enter select · Esc cancel",
            Style::default().fg(theme::FAINT),
        )),
        rows[1],
    );
}
```

- [ ] **Step 6: Rewrite `draw_detail_pane`**

Replace the entire `draw_detail_pane` function with:

```rust
fn draw_detail_pane(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == ListFocus::Detail;
    let block = panel("Detail", focused);

    let Some(host_idx) = app.selected_host() else {
        f.render_widget(block, area);
        return;
    };
    let Some(h) = app.hosts.get(host_idx) else {
        f.render_widget(block, area);
        return;
    };

    let state = app.liveness_by_index(host_idx);
    let status = match state {
        Liveness::Up => {
            let rtt = app
                .rtt_by_index(host_idx)
                .map(|d| format!(" ({} ms)", d.as_millis()))
                .unwrap_or_default();
            Span::styled(
                format!("up{rtt}"),
                Style::default()
                    .fg(theme::UP)
                    .add_modifier(Modifier::BOLD),
            )
        }
        Liveness::Down => Span::styled("down", Style::default().fg(theme::DOWN)),
        Liveness::Checking => Span::styled("checking…", Style::default().fg(theme::CHECKING)),
        Liveness::Skipped => Span::styled("skipped (jump)", Style::default().fg(theme::FAINT)),
        Liveness::Unknown => Span::styled("unknown", Style::default().fg(theme::FAINT)),
    };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:>14}  ", "status"),
            Style::default().fg(theme::DIM),
        ),
        status,
    ]));
    lines.push(kv_line("alias", h.patterns.join(" ")));
    lines.push(kv_line(
        "HostName",
        h.host_name.clone().unwrap_or_else(|| "—".into()),
    ));
    lines.push(kv_line("User", h.user.clone().unwrap_or_else(|| "—".into())));
    lines.push(kv_line("Port", h.port.clone().unwrap_or_else(|| "—".into())));
    if let Some(j) = &h.proxy_jump {
        lines.push(kv_line("ProxyJump", j.clone()));
    }
    for id in &h.identity_files {
        lines.push(kv_line("IdentityFile", id.clone()));
    }
    for fwd in &h.local_forwards {
        lines.push(kv_line("LocalForward", fwd.clone()));
    }
    for fwd in &h.remote_forwards {
        lines.push(kv_line("RemoteForward", fwd.clone()));
    }
    for fwd in &h.dynamic_forwards {
        lines.push(kv_line("DynamicForward", fwd.clone()));
    }
    for (k, v) in &h.extras {
        lines.push(kv_line(k, v.clone()));
    }

    let para = Paragraph::new(Text::from(lines))
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll, 0));
    f.render_widget(para, area);
}
```

- [ ] **Step 7: Build — expect success**

Run: `cargo build`
Expected: compiles. (`Block`, `BorderType`, `Borders`, `Color` were removed from imports because they are no longer referenced in this file.)

- [ ] **Step 8: Commit**

```bash
git add src/ui/list.rs
git commit -m "feat(ui): restyle host list, detail, search, and jump picker"
```

---

## Task 5: Edit form (`ui/edit.rs`)

**Files:**
- Modify: `src/ui/edit.rs`

- [ ] **Step 1: Update imports**

Replace the current imports:

```rust
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::app::{App, FormMode};

use super::widgets::input_line;
```

with:

```rust
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::app::{App, FormMode};

use super::theme;
use super::widgets::{input_line, panel};
```

- [ ] **Step 2: Rewrite the `draw` body to use `panel` and theme roles**

Replace the entire `draw` function with:

```rust
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let title = match app.screen {
        crate::app::Screen::Edit { editing: Some(_) } => "Edit host",
        _ => "Add host",
    };
    let block = panel(title, true);

    let form = &app.form;
    let editing = form.mode == FormMode::Editing;
    let mut lines: Vec<Line> = Vec::new();
    let mut focus_line: usize = 0;

    for (idx, field) in form.fields.iter().enumerate() {
        let is_focused = idx == form.focused;
        if is_focused {
            focus_line = lines.len();
        }
        let label_style = if is_focused {
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::DIM)
        };
        let marker = if is_focused { "▸ " } else { "  " };

        if !field.multi {
            let mut spans = vec![
                Span::styled(marker, label_style),
                Span::styled(format!("{:<26}", field.label), label_style),
            ];
            let active = is_focused && editing;
            spans.extend(input_line(&field.value, field.cursor, active).spans);
            lines.push(Line::from(spans));
        } else {
            lines.push(Line::from(vec![
                Span::styled(marker, label_style),
                Span::styled(format!("{}:", field.label), label_style),
            ]));
            if field.rows.is_empty() {
                lines.push(Line::from(Span::styled(
                    "      (none — press 'a' to add)",
                    Style::default().fg(theme::FAINT),
                )));
            }
            for (ri, row) in field.rows.iter().enumerate() {
                let row_focused = is_focused && ri == field.row_sel;
                let active = row_focused && editing;
                let bullet_style = if row_focused {
                    Style::default()
                        .fg(theme::ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::FAINT)
                };
                let mut spans = vec![Span::styled(
                    if row_focused { "    › " } else { "      " },
                    bullet_style,
                )];
                if active {
                    spans.extend(input_line(row, field.cursor, true).spans);
                } else {
                    spans.push(Span::styled(row.clone(), Style::default().fg(theme::TEXT)));
                }
                lines.push(Line::from(spans));
            }
        }

        // Inline validation error for this field.
        if let Some((_, msg)) = form.errors.iter().find(|(i, _)| *i == idx) {
            lines.push(Line::from(Span::styled(
                format!("      ⚠ {msg}"),
                Style::default().fg(theme::DOWN),
            )));
        }
    }

    let inner_h = area.height.saturating_sub(2) as usize;
    let scroll = if focus_line + 2 > inner_h {
        (focus_line + 2 - inner_h) as u16
    } else {
        0
    };

    let para = Paragraph::new(Text::from(lines))
        .block(block)
        .scroll((scroll, 0));
    f.render_widget(para, area);
}
```

- [ ] **Step 3: Build — expect success**

Run: `cargo build`
Expected: compiles.

- [ ] **Step 4: Commit**

```bash
git add src/ui/edit.rs
git commit -m "feat(ui): restyle edit form with theme roles"
```

---

## Task 6: Key manager (`ui/keys.rs`)

**Files:**
- Modify: `src/ui/keys.rs`

- [ ] **Step 1: Update imports**

Replace the current imports:

```rust
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::app::App;
use crate::os::keys::KeyType;

use super::widgets::{centered, input_line};
```

with:

```rust
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, List, ListItem, Paragraph, Wrap};

use crate::app::App;
use crate::os::keys::KeyType;

use super::theme;
use super::widgets::{centered, input_line, modal_block, panel};
```

- [ ] **Step 2: Rewrite `draw` (key list + empty state, drop the emoji)**

Replace the entire `draw` function with:

```rust
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

    let cols =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).split(area);

    let items: Vec<ListItem> = app
        .keys
        .iter()
        .map(|k| {
            let (mark, mark_style) = if k.has_private {
                ("● ", Style::default().fg(theme::ACCENT))
            } else {
                ("○ ", Style::default().fg(theme::FAINT))
            };
            ListItem::new(Line::from(vec![
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
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(panel("Keys", true))
        .highlight_style(theme::selection())
        .highlight_symbol(theme::SELECT_SYMBOL);
    f.render_stateful_widget(list, cols[0], &mut app.keys_state);

    draw_detail(f, app, cols[1]);
}
```

- [ ] **Step 3: Rewrite `draw_detail`**

Replace the entire `draw_detail` function with:

```rust
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
    kv("public file", k.path.display().to_string());

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
```

- [ ] **Step 4: Rewrite `draw_picker`**

Replace the entire `draw_picker` function with:

```rust
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
```

- [ ] **Step 5: Rewrite `draw_wizard`**

Replace the entire `draw_wizard` function with:

```rust
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
```

- [ ] **Step 6: Build — expect success**

Run: `cargo build`
Expected: compiles. The `🔒` emoji is gone; private-key presence is now an accent `●` / faint `○`.

- [ ] **Step 7: Commit**

```bash
git add src/ui/keys.rs
git commit -m "feat(ui): restyle key manager, picker, and wizard; drop emoji"
```

---

## Task 7: known_hosts (`ui/known_hosts.rs`)

**Files:**
- Modify: `src/ui/known_hosts.rs`

- [ ] **Step 1: Update imports**

Replace the current imports:

```rust
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, Paragraph};

use crate::app::App;

use super::widgets::input_line;
```

with:

```rust
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};

use crate::app::App;

use super::theme;
use super::widgets::{input_line, panel};
```

- [ ] **Step 2: Rewrite `draw`**

Replace the entire `draw` function with:

```rust
pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    draw_search(f, app, rows[0]);

    let filtered = app.kh_filtered();
    let items: Vec<ListItem> = filtered
        .iter()
        .filter_map(|&i| app.known_hosts.get(i))
        .map(|e| {
            let marker = e
                .marker
                .as_deref()
                .map(|m| format!("{m} "))
                .unwrap_or_default();
            ListItem::new(Line::from(vec![
                Span::styled(marker, Style::default().fg(theme::ACCENT2)),
                Span::styled(
                    e.host.display(),
                    Style::default()
                        .fg(theme::TEXT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}", e.key_type),
                    Style::default().fg(theme::DIM),
                ),
            ]))
        })
        .collect();

    let title = format!("Known hosts  [{}/{}]", filtered.len(), app.known_hosts.len());
    let list = List::new(items)
        .block(panel(&title, true))
        .highlight_style(theme::selection())
        .highlight_symbol(theme::SELECT_SYMBOL);
    f.render_stateful_widget(list, rows[1], &mut app.kh_state);
}
```

- [ ] **Step 3: Rewrite `draw_search`**

Replace the entire `draw_search` function with:

```rust
fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let prefix = Span::styled(
        " / ",
        if app.kh_searching {
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::FAINT)
        },
    );
    let body = input_line(&app.kh_search, app.kh_search.len(), app.kh_searching);
    let mut spans = vec![prefix];
    spans.extend(body.spans);
    if app.kh_search.is_empty() && !app.kh_searching {
        spans.push(Span::styled(
            "search host / key type",
            Style::default().fg(theme::FAINT),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
```

- [ ] **Step 4: Build — expect success**

Run: `cargo build`
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add src/ui/known_hosts.rs
git commit -m "feat(ui): restyle known_hosts list and search"
```

---

## Task 8: Confirm, action menu, and help (`ui/confirm.rs`, `ui/help.rs`)

**Files:**
- Modify: `src/ui/confirm.rs`
- Modify: `src/ui/help.rs`

- [ ] **Step 1: Update `confirm.rs` imports**

Replace the current imports:

```rust
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::app::{App, ConfirmAction};

use super::widgets::centered;
```

with:

```rust
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::{App, ConfirmAction};

use super::theme;
use super::widgets::{centered, modal_block};
```

- [ ] **Step 2: Rewrite `confirm::draw`**

Replace the entire `draw` function with (note: titles are now passed without surrounding spaces, since `modal_block` adds them):

```rust
pub fn draw(f: &mut Frame, action: ConfirmAction, area: Rect) {
    let (title, message, danger) = match action {
        ConfirmAction::DeleteHost(_) => (
            "Delete host",
            "Remove this Host block from ~/.ssh/config?".to_string(),
            true,
        ),
        ConfirmAction::RemoveKey(_) => (
            "Delete key",
            "Delete this key pair (private + public) from disk?".to_string(),
            true,
        ),
        ConfirmAction::RemoveKnownHost { .. } => (
            "Remove known_host",
            "Remove this entry from known_hosts?".to_string(),
            true,
        ),
        ConfirmAction::DiscardEdit => (
            "Discard changes",
            "Discard unsaved changes to this host?".to_string(),
            true,
        ),
        ConfirmAction::Quit => ("Quit", "Quit SSH Manager?".to_string(), false),
    };

    let modal = centered(56, 7, area);
    f.render_widget(Clear, modal);
    let block = modal_block(title, danger);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {message}"),
            Style::default().fg(theme::TEXT),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw("   "),
            Span::styled(
                " y ",
                Style::default()
                    .fg(theme::BG)
                    .bg(if danger { theme::DOWN } else { theme::UP })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  confirm    ", Style::default().fg(theme::DIM)),
            Span::styled(
                " n / Esc ",
                Style::default()
                    .fg(theme::BG)
                    .bg(theme::DIM)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  cancel", Style::default().fg(theme::DIM)),
        ]),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), modal);
}
```

- [ ] **Step 3: Rewrite `confirm::draw_action_menu`**

Replace the entire `draw_action_menu` function with:

```rust
pub fn draw_action_menu(f: &mut Frame, app: &App, host_idx: usize, area: Rect) {
    let alias = app
        .hosts
        .get(host_idx)
        .map(|h| h.alias().to_string())
        .unwrap_or_default();

    let modal = centered(40, (ACTION_LABELS.len() as u16) + 4, area);
    f.render_widget(Clear, modal);
    let block = modal_block(&alias, false);

    let mut lines = vec![Line::from("")];
    for (i, label) in ACTION_LABELS.iter().enumerate() {
        let selected = i == app.menu_sel;
        let (marker, style) = if selected {
            (
                "▎ ",
                Style::default()
                    .bg(theme::SEL_BG)
                    .fg(theme::TEXT)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            ("  ", Style::default().fg(theme::DIM))
        };
        lines.push(Line::from(Span::styled(format!("{marker}{label}"), style)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  j/k move · Enter run · Esc close",
        Style::default().fg(theme::FAINT),
    )));
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), modal);
}
```

(The `ACTION_LABELS` const at the top of the file is unchanged.)

- [ ] **Step 4: Update `help.rs` imports**

Replace the current imports:

```rust
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::app::{App, Screen};

use super::widgets::centered_pct;
```

with:

```rust
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::{App, Screen};

use super::theme;
use super::widgets::{centered_pct, modal_block};
```

- [ ] **Step 5: Rewrite `help::draw` block + the trailing note**

In `help::draw`, replace the block construction:

```rust
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Help  (Esc / ? to close) ");
```

with:

```rust
    let block = modal_block("Help  (Esc / ? to close)", false);
```

Then, near the end of the same function, replace the trailing note:

```rust
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Hosts are read from & written to ~/.ssh/config directly.",
        Style::default().fg(Color::DarkGray),
    )));
```

with:

```rust
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Hosts are read from & written to ~/.ssh/config directly.",
        Style::default().fg(theme::FAINT),
    )));
```

- [ ] **Step 6: Rewrite the `section` and `key` helpers in `help.rs`**

Replace the entire `section` function with:

```rust
fn section(title: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {title}"),
        Style::default()
            .fg(theme::ACCENT)
            .add_modifier(Modifier::BOLD),
    ))
}
```

Replace the entire `key` function with:

```rust
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
```

- [ ] **Step 7: Build — expect success**

Run: `cargo build`
Expected: compiles.

- [ ] **Step 8: Commit**

```bash
git add src/ui/confirm.rs src/ui/help.rs
git commit -m "feat(ui): restyle confirm, action menu, and help overlays"
```

---

## Task 9: Final verification

**Files:** none (verification + cleanup commit only if needed)

- [ ] **Step 1: Format**

Run: `cargo fmt`
Then: `cargo fmt --check`
Expected: clean (no diff).

- [ ] **Step 2: Clippy with denied warnings (now everything is wired)**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no errors. If any `unused import` / `dead_code` fires, remove the offending import or item. Common suspects: a `Color`/`Block`/`BorderType`/`Borders` import left behind in a file whose blocks all moved to `panel`/`modal_block`. Fix and re-run.

Cross-platform note (per `CLAUDE.md`): this change adds no `#[cfg(windows)]`-gated symbols, so the Linux-vs-Windows `dead_code` trap should not apply. Still, run clippy.

- [ ] **Step 3: Run the unit tests**

Run: `cargo test`
Expected: all pass, including `ui::theme::tests`. (No `config/`/`os/` tests were touched.)

- [ ] **Step 4: Manual visual check**

Prepare a throwaway config and launch the TUI (do NOT point at the real `~/.ssh/config`):

```bash
cargo run -- --config ./scratch-ssh-config
```

If `./scratch-ssh-config` does not exist, create one with a couple of hosts first (e.g. two `Host` blocks with `HostName`/`User`, one with `ProxyJump`). Verify each surface against the design spec:

- Title bar reads `sshm  ›  Hosts  N/M` (accent app name, faint separator/count).
- Host table: dim header row, colored liveness glyph, selected row has subtle background + `▎` left bar.
- Detail pane (Tab to focus): focused border turns accent; status line colored (up green / down pink / checking cyan).
- Footer hints: accent keys, dim labels, faint `·` separators — no cyan badges.
- `a` → edit form: focused field label accent + `▸`; validation error (try saving empty) shows `⚠` in red.
- `K` → keys: private-key rows show accent `●`, public-only `○` (no 🔒); detail border dim; picker/wizard use rounded accent borders.
- `H` → known_hosts: themed list + search.
- `?` → help, `o` → action menu, `d` → delete confirm (red rounded border), `q` → quit confirm: all rounded, accent (or red for destructive).
- Trigger a success toast (e.g. save a host) → `✓` green on subtle bg; trigger an error (e.g. save a value containing `"`) → `✗` dark-on-red.

- [ ] **Step 5: Final commit (only if fmt/clippy changed files)**

```bash
git add -A
git commit -m "chore(ui): fmt + clippy cleanup for visual redesign"
```

---

## Self-Review (completed by plan author)

**Spec coverage:**
- §2 theme module → Task 1. ✅
- §3 title bar → Task 3; borders/padding → `panel` (Task 2) applied in Tasks 4–7; selection → `selection()`/`SELECT_SYMBOL` (Task 2) applied in Tasks 4,6,7,8; modals → `modal_block` (Task 2) applied in Tasks 4,6,8; footer → Task 2 (`footer_hints`); toast → Task 3. ✅
- §4 list → Task 4; edit → Task 5; keys (drop emoji) → Task 6; known_hosts → Task 7; confirm/menu/help → Task 8. ✅
- §6 verification (build/clippy/fmt/test/manual) → Task 9. ✅

**Placeholder scan:** No TBD/TODO; every code step contains complete, copy-pasteable code. ✅

**Type consistency:** Helpers are referenced with consistent names everywhere — `theme::border(bool)`, `theme::selection()`, `theme::SELECT_SYMBOL`, `widgets::panel(&str, bool)`, `widgets::modal_block(&str, bool)`, and the color consts `theme::{BG,TEXT,DIM,FAINT,ACCENT,ACCENT2,SEL_BG,BORDER,UP,DOWN,WARN,CHECKING}`. Titles passed to `panel`/`modal_block` are space-free (the builders add the surrounding spaces). ✅

**Known deviation from spec (acceptable):** The selection left bar `▎` is rendered via `highlight_symbol` in the row's foreground (`TEXT`) on the `SEL_BG` background, not in `accent`. ratatui styles the highlight symbol with the row's highlight style, so an independently accent-colored bar would require per-row span injection that the stateful `List`/`Table` widgets don't cleanly support. The spec explicitly left the mechanism open ("意図は『控えめ背景 + accent の左マーカー』"); this is the clean, implementable realization. If a true accent bar is later desired, it's a follow-up.
