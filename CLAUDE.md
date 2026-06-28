# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

`sshm` is a terminal UI (ratatui) for browsing, editing, and connecting to the
SSH hosts in `~/.ssh/config`. It is **Windows-first** but cross-platform. The
defining constraint: it reads and writes the *real* OpenSSH config file, so edits
must be lossless and surgical.

## Commands

```sh
cargo build              # debug build
cargo build --release    # release -> target/release/sshm(.exe)
cargo run                # launch the TUI against ~/.ssh/config
cargo run -- --list      # non-interactive: print hosts and exit
cargo run -- --config PATH   # use an alternate config file (great for manual testing)

cargo test               # all unit tests (config/ and os/ layers)
cargo test roundtrip_crlf    # a single test by name
cargo test config::          # all tests in the config module
cargo clippy --all-targets -- -D warnings   # lint — exactly what CI gates on
cargo fmt                                    # format in place
```

The three gates CI runs on Linux *and* Windows — match them before pushing:
`cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test --all`.

Rust edition 2024; the MSRV is pinned via `rust-version = "1.94"` in `Cargo.toml`.
OpenSSH (`ssh`, `ssh-keygen`) must be on `PATH`; new-tab connect additionally
needs `wt.exe`.

When manually testing edit/save behavior, always point `--config` at a throwaway
file — the app writes to whatever path it is given.

Releases are cut with the user-invoked `/release` runbook
(`.claude/skills/release/SKILL.md`): version bump → quality gates → release build
→ git tag → GitHub release. It has side effects and is **user-only** — never run
it unprompted.

## Architecture

### Layering and the one-way dependency rule

Three layers with a strict, enforced dependency direction:

- **`config/`** — the lossless `~/.ssh/config` parser + surgical writer. **Zero
  ratatui dependency**, fully headless-testable. This is the most important and
  most-tested module.
- **`os/`** — all external-world integration: spawning `ssh`/`ssh-keygen`,
  TCP liveness probing, SSH key discovery + fingerprint-based pairing,
  `known_hosts` parsing/rewriting, clipboard, binary resolution, and the
  encrypted password vault (`os/vault.rs`: Argon2id + XChaCha20-Poly1305 over
  `~/.ssh/sshm-vault.json`, holding per-host login passwords and key passphrases
  — secrets never touch the SSH config, are zeroized on drop, and the file
  header is AEAD-authenticated with range-checked KDF params), plus a small
  **non-secret** preferences file (`os/prefs.rs`: plain JSON at
  `~/.ssh/sshm-prefs.json`, owner-private atomic write — currently just the
  persisted password-autofill opt-in; never holds a secret). **Zero ratatui
  dependency.**
- **`ui/`** — pure rendering only. **Never mutates domain state** (only widget
  scroll/selection state).

`config/` and `os/` know nothing about the UI; the UI and `update.rs` depend on
them. Keep it that way — do not reach into ratatui from `config/` or `os/`.

### Unidirectional state flow (Elm-ish)

`event_loop.rs` runs the loop: **draw → poll for input/tick**.

- All domain mutation lives in **`update.rs`** (`handle_key`) and **`app.rs`**.
  `update.rs` routes every keypress by the active `Screen`.
- **`app.rs`** holds the single `App` struct (all state) and the `Screen` enum
  that drives *both* rendering and input dispatch.
- **`ui/mod.rs`** `draw()` dispatches by `Screen`, painting a base screen then
  any modal overlay on top.

If you add a screen/mode, you touch all three: the `Screen` enum (`app.rs`), a
dispatch arm (`update.rs`), and a draw arm (`ui/mod.rs`). Modal overlays are
implemented via `App::prev_screen` + the `open_overlay`/`close_overlay` helpers.

### UI rendering (theme + responsive layout)

All rendering lives in `ui/` — one module per screen plus two shared foundations:

- **`ui/theme.rs`** — the single source of every color (a Tokyo Night palette)
  and small `Style` helpers (`selection()`, `border()`, `SELECT_SYMBOL`). Route
  every color choice through here; never hardcode a `Color` in a screen module.
- **`ui/widgets.rs`** — shared building blocks: `panel` / `modal_block` (rounded
  borders, accent when focused), `footer_hints`, `kv_line`, `section_header`,
  `input_line`, and `responsive_split`. `responsive_split` lays two panes
  side-by-side at or above `WIDE_MIN_WIDTH` (90 cols) and stacks them vertically
  below it; each call site passes its own side/stack split percentages.

`ui/mod.rs` `draw()` paints a fixed three-row frame (breadcrumb title · body ·
footer hints), then overlays the active modal; `base_screen()` decides which
non-modal screen renders underneath an overlay.

### The lossless round-trip invariant (config layer)

**`render(parse(s)) == s` byte-for-byte for anything the user did not edit.**
This is the core contract; the test suite in `config/mod.rs` guards it.

To achieve it, the document is an ordered `Vec<Item>` where every line preserves
its original indentation, keyword casing, separator (`" "`, `"="`, `" = "`), and
argument text (`model.rs`). Editing never re-renders a whole block — it mutates
the block body at **line granularity** through the surgical setters in
`writer.rs` (`set_single`, `set_multi`, `set_extras`): unchanged lines are left
untouched, only-changed values are rewritten in place, the header is only
rewritten when patterns actually change.

`HostView` (`model.rs`) is the flattened, editable *projection* of a `HostBlock`
used by the form; `from_block` builds it, `apply_view` (`config/mod.rs`) writes
it back surgically. Options not covered by a dedicated form field round-trip
through `HostView::extras` (the B1 regression test guards against dropping them).

ssh_config quoting rule (`tokens.rs`): backslash is **not** an escape character,
so bare Windows paths (`C:\Users\me\.ssh\id`) round-trip untouched. There is no
escape for a literal `"`, so the save path rejects values containing one rather
than corrupt the file.

### The config file is the source of truth for connections

Saved hosts connect via plain `ssh <alias>` (`os/connect.rs`) so OpenSSH itself
reads the very file we wrote — ProxyJump, forwards, IdentityFile etc. apply
automatically. Explicit `ssh` flags (`-i`, `-J`, `-L`...) are emitted *only* for
ad-hoc `ConnectOverrides`, never for saved values.

### Liveness probing

`os/liveness.rs` runs a fixed worker-thread pool draining a shared job queue and
reporting over an `mpsc` channel. The UI thread never blocks: it calls
`App::drain_liveness()` once per tick. Results are keyed by **host index in
`App::hosts`**, which shifts when hosts are added/removed — `rebuild_hosts()`
clears the liveness maps and callers re-probe. Hosts behind a proxy
(`ProxyJump` or `ProxyCommand` — see `HostView::is_proxied`) are `Skipped` (a
direct TCP probe would be meaningless).

### Key discovery & pairing

`os/keys.rs` discovers keys by walking `~/.ssh` recursively (`MAX_DEPTH = 8`) and
classifying files by sniffing **only the first header line** — private key bodies
are never loaded. Public/private halves are paired by fingerprinting each half
independently (`ssh-keygen -l -f`) and comparing the SHA256 *public* fingerprints,
surfaced as `PairStatus` (`Matched` / `Mismatched` / `Unverified` /
`NotApplicable`) — never by reading the private body. An encrypted PEM whose public
half ssh-keygen won't surface without the passphrase is `Unverified`, not an error.

## Windows-first specifics (don't regress these)

- **Binary resolution** (`os/binaries.rs`): prefer `System32\OpenSSH` over the
  bare `ssh` on `PATH`, because the Git/MSYS `ssh` interprets `~/.ssh/config`,
  `-J`, and forwards differently. A `[PATH ssh]` warning shows when the fallback
  is used.
- **Key events**: the Windows console emits both key-down and key-up;
  `event_loop.rs` acts only on `KeyEventKind::Press` to avoid double input.
- **Atomic save** (`writer.rs`): write temp then rename, but Windows rename fails
  if the destination exists, so the target is removed first. `0o600` perms are
  set on unix only.
- **`wt.exe` escaping** (`escape_wt_arg` in `os/connect.rs`): quote on
  whitespace, escape `;`, double embedded `"`, leave backslashes intact.
- **Cross-platform clippy**: CI runs `cargo clippy --all-targets -- -D warnings`
  on Linux *and* Windows. A symbol referenced only from `#[cfg(windows)]` code
  (e.g. `find_wt`,
  `escape_wt_arg`) trips `dead_code` / `unused_imports` on the Linux build,
  which a local Windows-only `clippy` never sees. Gate such helpers to where
  they're used (`#[cfg(windows)]`, or `#[cfg(any(windows, test))]` when only a
  test also exercises them).

## Conventions

- Errors: the config parse/write layer uses a typed `ConfigError` (`error.rs`);
  everywhere else uses `anyhow::Result`. `ConfigError` converts into
  `anyhow::Error` automatically.
- User-facing failures surface as a `Toast` (`App::toast`), not panics. Error
  toasts are sticky (cleared on next keypress); success toasts auto-expire.
- The edit form couples three constants in `app.rs` that must stay in sync:
  `FIELD_LABELS`, the `form_idx` indices, and `MULTI_FIELDS`. Adding a field
  means updating all three (and `form_from_view` / `view_from_form`).
- Tests live inline as `#[cfg(test)]` modules in the `config/` and `os/` files.
  Regression tests are labelled by bug id in comments (e.g. B1, B5) — preserve
  them when refactoring.
- Two different search implementations: the host list is **fuzzy-ranked**
  (`nucleo-matcher`, `App::refilter`), while the known_hosts list is a plain
  case-insensitive **substring** match (`App::kh_filtered`). Don't assume they
  share behavior.
