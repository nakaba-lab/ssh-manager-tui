<div align="center">

# sshm

**A fast terminal UI for browsing, editing, and connecting to the SSH hosts in your `~/.ssh/config`.**

Windows-first, cross-platform, and careful: it reads **and writes the real config file**,
so every edit is surgical and lossless — your comments, blank lines, indentation, and
keyword casing survive untouched.

[![CI](https://github.com/nakaba-lab/ssh-manager-tui/actions/workflows/ci.yml/badge.svg)](https://github.com/nakaba-lab/ssh-manager-tui/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/nakaba-lab/ssh-manager-tui?sort=semver)](https://github.com/nakaba-lab/ssh-manager-tui/releases)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-lightgrey.svg)](#platform-support)

</div>

---

`sshm` parses `~/.ssh/config` into an ordered, byte-faithful document and only ever
rewrites the lines you actually change. Connections run through plain `ssh <alias>`,
so OpenSSH itself reads the very file you edited — `ProxyJump`, forwards, and
`IdentityFile` all apply automatically. No lock-in, no parallel database, no surprises
for the other tools that read your config.

## What it looks like

```text
 sshm  ›  Hosts  5/5

╭─ Hosts ───────────────────────────────────────────────────╮
│     Alias       HostName              User                │
│ ▎ ● web-prod    10.0.1.20             deploy              │
│   ● db-1        db1.internal          admin               │
│   ○ staging     staging.example.com   ubuntu              │
│   — bastion     —                     —                   │
│   · old-box     192.168.1.5           root                │
│                                                           │
╰───────────────────────────────────────────────────────────╯
╭─ Detail ──────────────────────────────────────────────────╮
│         status  ● up (24 ms)                              │
│                                                           │
│   Connection                                              │
│          alias  web-prod                                  │
│       HostName  10.0.1.20                                 │
│           User  deploy                                    │
│           Port  22                                        │
│      ProxyJump  bastion                                   │
│                                                           │
│   Identity                                                │
│   IdentityFile  ~/.ssh/id_ed25519                         │
╰───────────────────────────────────────────────────────────╯
 j/k move · / search · Enter connect · t new-tab · e edit · a add · d del · K keys · ? help
```

A breadcrumb title bar, a searchable host table with a live reachability column, and a
grouped detail pane — styled with a built-in **Tokyo Night** theme and a responsive
layout that stacks the panes on narrow terminals.

## Features

- **Host list & connect** — a fuzzy-searchable list of every `Host` entry with a live
  reachability dot. Connect **inline** (suspends the TUI, runs `ssh`, restores it on
  exit) or open the session in a **new Windows Terminal tab**. Copy the equivalent
  `ssh` command to the clipboard with one key.
- **Lossless config editing** — add, edit, and delete `Host` blocks through a form:
  `HostName`, `User`, `Port`, `IdentityFile`, `ProxyJump`, `LocalForward` /
  `RemoteForward` / `DynamicForward`, plus arbitrary extra options. Everything is
  written back **surgically** — only changed lines are rewritten.
- **ProxyJump & port forwarding** — edit them per host with pickers (choose a jump host
  from your other entries); they apply automatically on connect because they live in
  the file.
- **Key manager** — list `~/.ssh/*.pub` with fingerprints, generate new keys
  (Ed25519 or RSA-4096 via `ssh-keygen`), copy a public key, set a key as a host's
  `IdentityFile`, or delete a key pair.
- **known_hosts viewer** — browse and search `known_hosts` (including hashed entries)
  and remove a stale entry safely.
- **Live reachability** — a background worker pool TCP-probes hosts without ever
  blocking the UI; round-trip time is shown in the detail pane.

## Installation

### Package managers

**Cargo** — any platform with a Rust toolchain (**1.94+**, edition 2024):

```sh
cargo install sshm-tui          # installs the `sshm` binary into ~/.cargo/bin
```

> Published on crates.io as **`sshm-tui`** (the bare `sshm` name was already taken),
> but the installed command is still **`sshm`**.

**Scoop** — Windows:

```powershell
scoop bucket add nakaba-lab https://github.com/nakaba-lab/scoop-bucket
scoop install sshm-tui
```

**winget** — Windows:

```powershell
winget install NakabaLab.sshm
```

### Download a prebuilt binary

Each [**Release**](https://github.com/nakaba-lab/ssh-manager-tui/releases) attaches
archives for Windows (`.zip`), Linux, and macOS (`.tar.gz`), each with a matching
`.sha256` checksum. Download the one for your platform, extract `sshm`/`sshm.exe`, and
put it on your `PATH`.

> The published Windows binary is currently **self-signed**, so SmartScreen may show a
> "Windows protected your PC" warning. Choose **More info → Run anyway**, or build from
> source if you prefer.

### From source

```sh
# install straight from git into ~/.cargo/bin (binary: sshm)
cargo install --git https://github.com/nakaba-lab/ssh-manager-tui

# …or clone and build
git clone https://github.com/nakaba-lab/ssh-manager-tui
cd ssh-manager-tui
cargo build --release      # binary at target/release/sshm(.exe)
```

### Runtime requirements

- **OpenSSH** (`ssh`, `ssh-keygen`) on `PATH`. On Windows, `sshm` prefers the
  `System32\OpenSSH` build and shows a `[PATH ssh]` warning in the title bar if it has
  to fall back to a different `ssh` (e.g. the Git/MSYS one, which interprets the config
  differently).
- **Windows Terminal** (`wt.exe`) — only needed for the "connect in a new tab" action.

## Usage

```text
sshm                 launch the interactive TUI against ~/.ssh/config
sshm --list          print configured hosts and exit (non-interactive)
sshm --config PATH   use an alternate config file (great for safe testing)
sshm --version       print version and exit
sshm --help          print help and exit
```

`-c`, `-l`, `-V`, `-h` are accepted as short aliases.

## Keybindings

### Host list

| Key | Action |
|-----|--------|
| `j` / `k`, `↓` / `↑` | move selection (scroll the detail pane when it has focus) |
| `Ctrl-d` / `Ctrl-u` | jump 5 rows down / up |
| `g` / `G` | first / last host |
| `Tab` | toggle focus between the list and the detail pane |
| `/` | search (fuzzy: alias / hostname / user) |
| `Enter` | **connect inline** (suspends the TUI, runs `ssh`, restores it) |
| `t` | **connect in a new Windows Terminal tab** |
| `o` | open the per-host action menu |
| `c` | copy the `ssh` command to the clipboard |
| `e` / `a` | edit / add a host |
| `d` | delete the host (with confirm) |
| `r` / `R` | refresh liveness for all / the selected host |
| `K` | key manager · `H` known_hosts viewer |
| `?` | help · `q` / `Ctrl-C` quit (`Esc` clears an active search filter first, else quits) |

While searching: type to filter, `Enter` keep the filter, `Esc` clear it, `↑`/`↓` move.

### Edit / add form

| Key | Action |
|-----|--------|
| `Tab` / `Shift-Tab`, `j` / `k` | next / previous field |
| `←` / `→` | select a row (in the IdentityFile / forward / extras lists) |
| `Enter` | edit the field — on **IdentityFile** opens the key picker, on **ProxyJump** opens the host picker |
| `i` | always edit the field value inline (even on IdentityFile / ProxyJump) |
| `a` / `d` | add / remove a row in a list field |
| `Ctrl-S` | validate & save |
| `Esc` | cancel the field, then leave the form (prompts if there are unsaved changes) |

While editing a field: `←`/`→` move the cursor, `Home`/`End` jump, `Backspace`/`Delete`
remove, `Enter` commit, `Esc` revert the field.

### Key manager

| Key | Action |
|-----|--------|
| `j` / `k`, `↓` / `↑`, `Home` / `End` | move |
| `g` | generate a key (Ed25519 / RSA-4096 wizard) |
| `y` | copy the public key |
| `s` | set as the `IdentityFile` of the host you opened from (`K`) |
| `d` | delete the key pair (with confirm) · `r` rescan · `Esc` back |

### known_hosts

| Key | Action |
|-----|--------|
| `j` / `k`, `↓` / `↑` | move · `g` / `G` top / bottom |
| `/` | search (substring: host / key type) |
| `d` | remove the entry (with confirm) · `r` reload · `Esc` back |

### Modals

The **confirm** dialog takes `y` / `Enter` to confirm and `n` / `Esc` to cancel. The
**action menu** and pickers use `j` / `k` to move, `Enter` to choose, and `Esc` to close.

## Liveness indicators

The dot in the host list (and the `status` line in the detail pane) shows reachability:

| Glyph | Meaning |
|:-----:|---------|
| `●` | **up** — TCP connect succeeded (detail pane shows the round-trip time) |
| `○` | **down** — TCP connect failed (refused, timed out, or the host name didn't resolve) |
| `…` | **checking** — a probe is in flight |
| `—` | **skipped** — behind a `ProxyJump` / `ProxyCommand`, or no resolvable target (a wildcard-only alias with no `HostName`), so a direct probe is impossible |
| `·` | **unknown** — not probed yet (before the first sweep) |

Probes are direct TCP connects to the host's `HostName` — or the alias itself when no
`HostName` is set — on its `Port` (default 22), run on a small background worker pool
with a 1.5s timeout, so the UI never blocks. Press `r` to re-probe everything or `R`
for just the selected host.

## Safety & the lossless guarantee

The core contract is **`render(parse(file)) == file`, byte-for-byte, for anything you
don't edit.** The config document is an ordered list of lines that each preserve their
original indentation, keyword casing, and separators; editing mutates only the lines
that changed and only rewrites a `Host` header when its patterns actually change.
Options without a dedicated form field round-trip through an "extra options" list rather
than being dropped.

On top of that:

- **A backup on first save** — the previous file is copied to `config.bak` the first
  time you save in a session.
- **Atomic writes** — saves go to a temp file and then rename over the target (on Unix
  with `0o600` perms).
- **Private keys are never read or displayed** — only key *paths* are passed to `ssh`
  via `-i`.
- **Validation before write** — a missing alias, a non-numeric port, or a value
  containing a `"` (which ssh_config cannot escape) is rejected rather than silently
  corrupting the file.
- **known_hosts edits are content-addressed** — an entry is removed by matching its
  verbatim line (a `.old` backup is written), so a stale index can't delete the wrong
  key after an external change.

## Notes & limitations

- Hosts pulled in via `Include` directives are **not** listed or edited — only
  `~/.ssh/config` itself is (a note is shown when `Include` directives are present).
- Generated keys currently have **no passphrase**, and the generator refuses to
  overwrite an existing file.
- Connections to saved hosts use plain `ssh <alias>`; explicit flags (`-i`, `-J`,
  `-L`, …) are only ever emitted for ad-hoc overrides, never for saved values.

## Platform support

`sshm` is **Windows-first** but fully cross-platform; CI runs the formatter, Clippy
(`-D warnings`), and the test suite on both **Linux** and **Windows**. Windows-specific
care includes:

- preferring `System32\OpenSSH` over a `PATH` `ssh` that would interpret the config
  differently,
- acting only on key **press** events (the Windows console also emits key-up),
- a Windows-safe atomic rename (remove-then-rename), and
- correct `wt.exe` argument escaping for the new-tab connect.

## Architecture

Three layers with a strict, one-way dependency direction:

- **`config/`** — the lossless `~/.ssh/config` parser and surgical writer. Zero UI
  dependency; this is the most heavily tested module.
- **`os/`** — all outside-world integration: spawning `ssh` / `ssh-keygen`, TCP
  liveness probing, `known_hosts` parsing, clipboard, binary resolution.
- **`ui/`** — pure rendering only; never mutates domain state.

State flow is Elm-ish: `event_loop.rs` draws then polls, `update.rs` routes every
keypress by the active `Screen`, and `app.rs` holds the single `App` struct. See
[`CLAUDE.md`](CLAUDE.md) for the full design notes.

## Development

```sh
cargo build                     # debug build
cargo run                       # launch against ~/.ssh/config
cargo run -- --config ./test    # run against a throwaway config (recommended)

cargo test                      # config/ and os/ unit + round-trip tests
cargo test roundtrip_crlf       # a single test by name
cargo clippy -- -D warnings
cargo fmt
```

The test suite is headless and pure — no test spawns `ssh`, `ssh-keygen`, or `wt`. When
manually testing save behavior, always point `--config` at a throwaway file: the app
writes to whatever path it is given.

## Support

`sshm` is free and open source, built in spare time. If it saves you time, you
can optionally sponsor its development via
[**GitHub Sponsors**](https://github.com/sponsors/nakata5577) — entirely
optional and genuinely appreciated. Starring the repo helps just as much. ⭐

## Contributing

Issues and pull requests are welcome. Before opening a PR, please run `cargo fmt`,
`cargo clippy -- -D warnings`, and `cargo test`. When touching the `config/` layer,
keep the lossless round-trip invariant intact and preserve the regression tests
(labelled by bug id in comments).

## License

Licensed under either of

- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  http://www.apache.org/licenses/LICENSE-2.0)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual
licensed as above, without any additional terms or conditions.

## Acknowledgements

Built with [ratatui](https://ratatui.rs) (terminal UI),
[nucleo-matcher](https://github.com/helix-editor/nucleo) (fuzzy search),
[arboard](https://github.com/1Password/arboard) (clipboard), and
[dirs](https://github.com/dirs-dev/dirs-rs) (home resolution).
