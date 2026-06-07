# UI Layout & Density Redesign Implementation Plan (round 2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the host list & key manager responsive (side-by-side when wide, stacked when narrow) and group the detail pane & edit form into labeled sections — all behavior-preserving, pure `ui/` rendering.

**Architecture:** Add a width-driven `responsive_split` helper and a shared `section_header` to `ui/widgets.rs`, then apply them in `list.rs`, `keys.rs`, and `edit.rs`. Layout is chosen from the `area: Rect` each `draw*` already receives — no `App` state, no keybindings, no field-model changes. Builds on the completed round-1 Tokyo Night restyle (`theme`, `panel`, `modal_block`).

**Tech Stack:** Rust 2024, ratatui (`Layout`, `Constraint::Percentage`, `Rect`).

---

## Testing note (read first)

- **`responsive_split` is the one unit-testable unit** — it gets a real failing-test-first cycle (wide → side-by-side; narrow → stacked).
- **Everything else is pure rendering**, verified by `cargo build` (must compile) + a manual visual run. Do NOT invent rendering unit tests.
- **`cargo clippy -- -D warnings` is deferred to the final task.** Intermediate tasks add `pub` helpers before they are all consumed; in a binary crate those emit `dead_code` *warnings* until used. `cargo build` still succeeds. Only Task 5 runs clippy with `-D warnings`, after everything is wired.
- Work continues on the existing branch `feature/ui-visual-redesign` (the round-1 work and both design specs live there).

## File Structure

- **Modify** `src/ui/widgets.rs` — add `WIDE_MIN_WIDTH`, `responsive_split`, `section_header`, and a `responsive_split` unit test.
- **Modify** `src/ui/list.rs` — responsive split in `draw`; section grouping in `draw_detail_pane`.
- **Modify** `src/ui/keys.rs` — responsive split in `draw`.
- **Modify** `src/ui/edit.rs` — section headers in `draw`.
- **Untouched** `src/ui/mod.rs`, `known_hosts.rs`, `confirm.rs`, `help.rs`, `theme.rs`, `app.rs`, `config/`, `os/`.

---

## Task 1: Layout helpers in `widgets.rs`

**Files:**
- Modify: `src/ui/widgets.rs`

- [ ] **Step 1: Write the failing test**

Append this test module to the END of `src/ui/widgets.rs`:

```rust
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
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test responsive_split`
Expected: FAIL to **compile** — `cannot find function responsive_split` / `cannot find value WIDE_MIN_WIDTH` (they don't exist yet). A compile failure is the expected "red" here.

- [ ] **Step 3: Add the helpers**

Insert the following into `src/ui/widgets.rs` immediately AFTER the `use super::theme;` line (before `centered`), so the new public items sit near the top:

```rust
/// Minimum terminal width (columns) at which a two-pane screen lays out
/// side-by-side. Below this, the two panes stack vertically. Tunable.
pub const WIDE_MIN_WIDTH: u16 = 90;

/// Split `area` into two panes. When `area` is at least [`WIDE_MIN_WIDTH`]
/// columns wide the panes sit side-by-side (`side_pct` = the primary pane's
/// width %); otherwise they stack vertically (`stack_pct` = the primary pane's
/// height %). Returns `(primary, secondary)`.
pub fn responsive_split(area: Rect, side_pct: u16, stack_pct: u16) -> (Rect, Rect) {
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
/// the edit form. Indented two columns to sit just inside a panel's padding.
pub fn section_header(title: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {title}"),
        Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD),
    ))
}
```

(`Rect`, `Layout`, `Constraint`, `Line`, `Span`, `Style`, `Modifier`, and `theme` are already imported at the top of `widgets.rs`.)

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test responsive_split`
Expected: PASS — both `responsive_split_is_side_by_side_when_wide` and `responsive_split_stacks_when_narrow` pass.

- [ ] **Step 5: Build**

Run: `cargo build`
Expected: compiles. `dead_code` warnings for `section_header` (and `responsive_split`/`WIDE_MIN_WIDTH` in the non-test build) are EXPECTED — they're consumed in later tasks. Do NOT add `#[allow(dead_code)]`.

- [ ] **Step 6: Commit**

```bash
git add src/ui/widgets.rs
git commit -m "feat(ui): add responsive_split and section_header layout helpers"
```

---

## Task 2: Responsive split in host list & key manager

**Files:**
- Modify: `src/ui/list.rs`
- Modify: `src/ui/keys.rs`

- [ ] **Step 1: Import the helper in `list.rs`**

In `src/ui/list.rs`, change the widgets import line from:

```rust
use super::widgets::{centered, input_line, kv_line, liveness_span, modal_block, panel};
```
to:
```rust
use super::widgets::{
    centered, input_line, kv_line, liveness_span, modal_block, panel, responsive_split,
};
```

- [ ] **Step 2: Use `responsive_split` in `list.rs::draw`**

Replace the ENTIRE `draw` function in `src/ui/list.rs` with:

```rust
pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    if app.hosts.is_empty() {
        draw_empty(f, app, area);
        return;
    }

    let (list_area, detail_area) = responsive_split(area, 58, 60);
    draw_list_pane(f, app, list_area);
    draw_detail_pane(f, app, detail_area);
}
```

- [ ] **Step 3: Import the helper in `keys.rs`**

In `src/ui/keys.rs`, change the widgets import line from:

```rust
use super::widgets::{centered, input_line, modal_block, panel};
```
to:
```rust
use super::widgets::{centered, input_line, modal_block, panel, responsive_split};
```

- [ ] **Step 4: Use `responsive_split` in `keys.rs::draw`**

In `src/ui/keys.rs::draw`, replace this block:

```rust
    let cols =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).split(area);

    let items: Vec<ListItem> = app
```
with:
```rust
    let (list_area, detail_area) = responsive_split(area, 45, 55);

    let items: Vec<ListItem> = app
```

Then, further down in the SAME function, replace:
```rust
    f.render_stateful_widget(list, cols[0], &mut app.keys_state);

    draw_detail(f, app, cols[1]);
```
with:
```rust
    f.render_stateful_widget(list, list_area, &mut app.keys_state);

    draw_detail(f, app, detail_area);
```

- [ ] **Step 5: Build**

Run: `cargo build`
Expected: compiles. (`Layout`/`Constraint` remain imported in both files — still used by `draw_list_pane`/`draw_jump_picker` in `list.rs` and `draw_picker` in `keys.rs`. `responsive_split`'s `dead_code` warning is now gone; `section_header` still warns until Tasks 3–4.)

- [ ] **Step 6: Commit**

```bash
git add src/ui/list.rs src/ui/keys.rs
git commit -m "feat(ui): responsive layout for host list and key manager"
```

---

## Task 3: Section grouping in the detail pane

**Files:**
- Modify: `src/ui/list.rs`

- [ ] **Step 1: Import `section_header`**

In `src/ui/list.rs`, extend the widgets import to include `section_header`:

```rust
use super::widgets::{
    centered, input_line, kv_line, liveness_span, modal_block, panel, responsive_split,
    section_header,
};
```

- [ ] **Step 2: Group the detail lines**

In `src/ui/list.rs::draw_detail_pane`, replace this block — from `let mut lines` through the final `for (k, v) in &h.extras` loop (i.e. everything between the `status` match and the `let para = ...` line):

```rust
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
    lines.push(kv_line(
        "User",
        h.user.clone().unwrap_or_else(|| "—".into()),
    ));
    lines.push(kv_line(
        "Port",
        h.port.clone().unwrap_or_else(|| "—".into()),
    ));
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
```

with:

```rust
    let mut lines: Vec<Line> = Vec::new();

    // Headline: status (ungrouped).
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:>14}  ", "status"),
            Style::default().fg(theme::DIM),
        ),
        status,
    ]));

    // Connection (always shown).
    lines.push(Line::from(""));
    lines.push(section_header("Connection"));
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

    // Identity (only when present).
    if !h.identity_files.is_empty() {
        lines.push(Line::from(""));
        lines.push(section_header("Identity"));
        for id in &h.identity_files {
            lines.push(kv_line("IdentityFile", id.clone()));
        }
    }

    // Forwarding (only when present).
    if !h.local_forwards.is_empty()
        || !h.remote_forwards.is_empty()
        || !h.dynamic_forwards.is_empty()
    {
        lines.push(Line::from(""));
        lines.push(section_header("Forwarding"));
        for fwd in &h.local_forwards {
            lines.push(kv_line("LocalForward", fwd.clone()));
        }
        for fwd in &h.remote_forwards {
            lines.push(kv_line("RemoteForward", fwd.clone()));
        }
        for fwd in &h.dynamic_forwards {
            lines.push(kv_line("DynamicForward", fwd.clone()));
        }
    }

    // Other (only when present).
    if !h.extras.is_empty() {
        lines.push(Line::from(""));
        lines.push(section_header("Other"));
        for (k, v) in &h.extras {
            lines.push(kv_line(k, v.clone()));
        }
    }
```

(The `let para = Paragraph::new(Text::from(lines)).block(block).wrap(...).scroll(...)` tail and the function's head — `focused`/`block`/early-returns/`status` match — are unchanged.)

- [ ] **Step 3: Build**

Run: `cargo build`
Expected: compiles. `section_header`'s `dead_code` warning is now gone in `list.rs` usage but may persist until `edit.rs` also uses it — either way, no errors.

- [ ] **Step 4: Commit**

```bash
git add src/ui/list.rs
git commit -m "feat(ui): group host detail pane into labeled sections"
```

---

## Task 4: Section headers in the edit form

**Files:**
- Modify: `src/ui/edit.rs`

- [ ] **Step 1: Update imports**

In `src/ui/edit.rs`, change:

```rust
use crate::app::{App, FormMode};

use super::theme;
use super::widgets::{input_line, panel};
```
to:
```rust
use crate::app::{App, FormMode, form_idx};

use super::theme;
use super::widgets::{input_line, panel, section_header};
```

- [ ] **Step 2: Insert section headers in the field loop**

Replace the ENTIRE `draw` function in `src/ui/edit.rs` with:

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

    // The section sub-heading that precedes a given field index (if any).
    // Boundaries follow the contiguous field groups; the field model is untouched.
    let section_for = |idx: usize| -> Option<&'static str> {
        if idx == form_idx::HOST {
            Some("Connection")
        } else if idx == form_idx::IDENTITY {
            Some("Identity & routing")
        } else if idx == form_idx::LOCAL_FWD {
            Some("Forwarding")
        } else if idx == form_idx::EXTRAS {
            Some("Advanced")
        } else {
            None
        }
    };

    for (idx, field) in form.fields.iter().enumerate() {
        if let Some(title) = section_for(idx) {
            if idx != 0 {
                lines.push(Line::from(""));
            }
            lines.push(section_header(title));
        }

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

(The only changes vs. the current function: the new `use` line in Step 1, the `section_for` closure, and the header-push block at the top of the loop. `focus_line` is captured AFTER any header is pushed, so scroll-to-focus still points at the field row.)

- [ ] **Step 3: Build**

Run: `cargo build`
Expected: compiles, and should now be WARNING-FREE for the new helpers (`section_header` is consumed by both `list.rs` and `edit.rs`; `responsive_split` by `list.rs`/`keys.rs`).

- [ ] **Step 4: Commit**

```bash
git add src/ui/edit.rs
git commit -m "feat(ui): group edit form fields into labeled sections"
```

---

## Task 5: Final verification

**Files:** none (verification + cleanup commit only if needed)

- [ ] **Step 1: Format**

Run: `cargo fmt`
Then: `cargo fmt --check`
Expected: clean (no diff).

- [ ] **Step 2: Clippy with denied warnings**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no errors. If an unused import slips through (e.g. `Layout`/`Constraint` if a file no longer needs them — they SHOULD still be needed), remove it and re-run.

- [ ] **Step 3: Run all tests**

Run: `cargo test`
Expected: all pass, including `ui::widgets::tests::responsive_split_*` (2 new) and the round-1 `ui::theme::tests` (2).

- [ ] **Step 4: Manual visual check**

Create a throwaway config and launch the TUI (do NOT use the real `~/.ssh/config`):

```bash
cargo run -- --config ./scratch-ssh-config
```

If `./scratch-ssh-config` doesn't exist, create one with ~3 hosts (one with `ProxyJump`, one with `IdentityFile` + a `LocalForward`, one plain). Verify against the spec:

- **Wide terminal (≥ 90 cols):** host list and key manager are side-by-side (list left / detail right), exactly as before.
- **Narrow terminal (< 90 cols):** resize the terminal narrow — the panes stack (list on top, detail below; keys list on top, key detail below). Tab still toggles focus (accent border moves), and detail scrolling still works.
- **Detail pane:** shows `Connection` always; `Identity`/`Forwarding`/`Other` headers appear only when that host has those values; no empty-section headers.
- **Edit form (`a` or `e`):** four section headers (`Connection`, `Identity & routing`, `Forwarding`, `Advanced`) with blank-line separators; Tab/field navigation, validation errors (try saving with an empty Host), and scroll-to-focus all still work.

- [ ] **Step 5: Final commit (only if fmt changed files)**

```bash
git add -A
git commit -m "style(ui): rustfmt cleanup for layout/density redesign"
```

---

## Self-Review (completed by plan author)

**Spec coverage:**
- §2 `responsive_split` + `WIDE_MIN_WIDTH` + `section_header` → Task 1. ✅
- §3 responsive split (host list 58/42↔60/40, key manager 45/55↔55/45) → Task 2. ✅
- §4 detail-pane grouping (Connection always; Identity/Forwarding/Other only when present) → Task 3. ✅
- §5 edit-form sectioning (Connection 0-3 / Identity & routing 4-5 / Forwarding 6-8 / Advanced 9) → Task 4. ✅
- §6 spacing rhythm (blank-line separators; no vertical padding) → realized by the `Line::from("")` separators in Tasks 3-4; `panel` padding untouched. ✅
- §8 verification (responsive_split unit test + build/clippy/fmt/manual) → Tasks 1 & 5. ✅
- §1 non-goals respected: no keybindings/`App` state, field model untouched (`form_idx` referenced read-only), `known_hosts`/`confirm`/`help`/modals/`config`/`os` not in any task. ✅

**Placeholder scan:** No TBD/TODO; every code step has complete, copy-pasteable code. ✅

**Type consistency:** `responsive_split(area: Rect, side_pct: u16, stack_pct: u16) -> (Rect, Rect)`, `WIDE_MIN_WIDTH: u16`, `section_header(&str) -> Line<'static>`, and `form_idx::{HOST,IDENTITY,LOCAL_FWD,EXTRAS}` are referenced identically everywhere. The `responsive_split` return order `(primary, secondary)` maps to `(list_area, detail_area)` in both call sites. ✅

**Edge cases considered:**
- Scroll-to-focus in the edit form: `focus_line` is captured after pushing a section header, so it points at the field row; `inner_h = height - 2` is unchanged (no vertical panel padding added). ✅
- Stacked layout preserves all behavior (focus/scroll/selection) because only the target `Rect`s change, not any state. ✅
