//! Input dispatch. All domain mutation happens here (and in `app.rs`); the UI
//! layer never mutates. `handle_key` routes by the active screen and threads the
//! terminal through for inline-ssh suspend/restore.

use std::io::stdout;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use zeroize::{Zeroize, Zeroizing};

use crate::app::{
    App, ConfirmAction, ConnectMode, FormMode, GenOrigin, ListFocus, Screen, VaultEntryForm,
    VaultUnlock, form_from_view, form_idx, is_multi, view_from_form,
};
use crate::config::model::HostView;
use crate::os::askpass::{DeclineReason, Outcome, arm_connect, os_tokens, resolved_identity};
use crate::os::connect::{
    ConnectOverrides, build_ssh_args, command_line, connect_new_tab, describe_exit,
    describe_exit_code, run_ssh_inline,
};
use crate::os::keys::{generate_key, read_public_key};
use crate::os::known_hosts::remove_entry;
use crate::os::resolve::{
    ResolvedConfig, has_match_exec, is_host_known, resolve_config, tofu_lookup_key,
};
use crate::os::ssh_dir;
use crate::os::vault::{
    self, MatchedKinds, Secret, SecretKind, Vault, VaultEntry, match_vault_kinds,
};
use crate::ui::confirm::ACTION_LABELS;

pub fn handle_key(app: &mut App, key: KeyEvent, terminal: &mut DefaultTerminal) -> Result<()> {
    // Refresh the idle clock so an active session never auto-locks the vault (#14).
    app.last_activity = std::time::Instant::now();
    // Any keypress dismisses a sticky error toast.
    if app.toast.is_error {
        app.clear_toast();
    }

    match app.screen.clone() {
        Screen::List => handle_list(app, key, terminal)?,
        Screen::Edit { editing } => handle_edit(app, key, editing)?,
        Screen::KeyManager => handle_keys(app, key)?,
        Screen::KnownHosts => handle_known_hosts(app, key),
        Screen::Help => handle_help(app, key),
        Screen::Confirm(action) => handle_confirm(app, key, action)?,
        Screen::ActionMenu(idx) => handle_action_menu(app, key, idx, terminal)?,
        Screen::GenerateKey { origin } => handle_gen_wizard(app, key, origin),
        Screen::PickKey { editing } => handle_pick_key(app, key, editing),
        Screen::PickJump { editing } => handle_pick_jump(app, key, editing),
        Screen::Vault => handle_vault(app, key),
        Screen::VaultUnlock => handle_vault_unlock(app, key),
        Screen::VaultEntry { editing } => handle_vault_entry(app, key, editing),
        Screen::PasswordConfirm {
            alias, mode, rc, ..
        } => handle_password_confirm(app, key, alias, mode, rc, terminal)?,
    }
    Ok(())
}

/// The one-time password-confirm modal: Enter/`y` confirms (re-enter the connect
/// arming the password), Esc/`n` declines (re-enter withholding it — passphrase
/// still arms). The cached `rc` from the first pass is handed back so the re-entry
/// reuses it instead of running `ssh -G` again. Closing the overlay first returns
/// to the base screen (List) so the inline connect suspends/restores over it.
fn handle_password_confirm(
    app: &mut App,
    key: KeyEvent,
    alias: String,
    mode: ConnectMode,
    rc: Box<ResolvedConfig>,
    terminal: &mut DefaultTerminal,
) -> Result<()> {
    match key.code {
        KeyCode::Enter | KeyCode::Char('y') => {
            close_overlay(app);
            connect_by_alias(
                app,
                terminal,
                &alias,
                mode,
                PasswordChoice::Confirmed,
                Some(*rc),
            )?;
        }
        KeyCode::Esc | KeyCode::Char('n') => {
            close_overlay(app);
            connect_by_alias(
                app,
                terminal,
                &alias,
                mode,
                PasswordChoice::Withheld,
                Some(*rc),
            )?;
        }
        _ => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Text-editing helpers (UTF-8 aware).
// ---------------------------------------------------------------------------

fn prev_boundary(s: &str, i: usize) -> usize {
    let mut j = i.saturating_sub(1);
    while j > 0 && !s.is_char_boundary(j) {
        j -= 1;
    }
    j
}

fn next_boundary(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    let mut j = i + 1;
    while j < s.len() && !s.is_char_boundary(j) {
        j += 1;
    }
    j
}

fn insert_char(s: &mut String, cursor: &mut usize, c: char) {
    s.insert(*cursor, c);
    *cursor += c.len_utf8();
}

fn backspace(s: &mut String, cursor: &mut usize) {
    if *cursor > 0 {
        let p = prev_boundary(s, *cursor);
        s.replace_range(p..*cursor, "");
        *cursor = p;
    }
}

fn delete_forward(s: &mut String, cursor: &mut usize) {
    if *cursor < s.len() {
        let n = next_boundary(s, *cursor);
        s.replace_range(*cursor..n, "");
    }
}

// --- Secret-field editing -------------------------------------------------
//
// The plain `insert_char` / `backspace` / `delete_forward` mutate a `String`
// in place. For a *secret* field (master password, passphrase) that leaks
// plaintext on the heap: `String::insert` reallocates when it outgrows its
// capacity and frees the old buffer **without** scrubbing it, and `replace_range`
// only shifts bytes left, leaving the deleted tail readable past `len`. The
// variants below grow/shrink through a fresh allocation and zeroize the old
// buffer before it is dropped, so no intermediate copy of the secret survives.

/// Insert `c` at `cursor`, growing via a zeroizing reallocation so the old
/// backing buffer is wiped (not just freed) when the value outgrows its
/// capacity. Use only for secret fields.
fn insert_char_secret(s: &mut String, cursor: &mut usize, c: char) {
    if s.len() + c.len_utf8() > s.capacity() {
        let mut grown = String::with_capacity(((s.len() + c.len_utf8()) * 2).max(64));
        grown.push_str(s);
        s.zeroize();
        *s = grown;
    }
    s.insert(*cursor, c);
    *cursor += c.len_utf8();
}

/// Remove `range` from a secret `String` by rebuilding into a fresh buffer and
/// zeroizing the old one — so the removed bytes never linger past `len` on the
/// heap the way an in-place `replace_range` would leave them.
fn remove_range_secret(s: &mut String, range: std::ops::Range<usize>) {
    let mut rebuilt = String::with_capacity(s.capacity());
    rebuilt.push_str(&s[..range.start]);
    rebuilt.push_str(&s[range.end..]);
    s.zeroize();
    *s = rebuilt;
}

/// `backspace` for a secret field — see [`remove_range_secret`].
fn backspace_secret(s: &mut String, cursor: &mut usize) {
    if *cursor > 0 {
        let p = prev_boundary(s, *cursor);
        remove_range_secret(s, p..*cursor);
        *cursor = p;
    }
}

/// `delete_forward` for a secret field — see [`remove_range_secret`].
fn delete_forward_secret(s: &mut String, cursor: &mut usize) {
    if *cursor < s.len() {
        let n = next_boundary(s, *cursor);
        remove_range_secret(s, *cursor..n);
    }
}

// ---------------------------------------------------------------------------
// S1 — host list
// ---------------------------------------------------------------------------

fn handle_list(app: &mut App, key: KeyEvent, terminal: &mut DefaultTerminal) -> Result<()> {
    if app.searching {
        handle_list_search(app, key);
        return Ok(());
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('c') if ctrl => open_confirm(app, ConfirmAction::Quit),
        KeyCode::Char('q') => open_confirm(app, ConfirmAction::Quit),
        KeyCode::Char('?') => open_overlay(app, Screen::Help),
        KeyCode::Esc => {
            if !app.search.is_empty() {
                app.search.clear();
                app.refilter();
            } else {
                open_confirm(app, ConfirmAction::Quit);
            }
        }
        KeyCode::Tab => {
            app.focus = match app.focus {
                ListFocus::Hosts => ListFocus::Detail,
                ListFocus::Detail => ListFocus::Hosts,
            };
        }
        KeyCode::Char('/') => {
            app.searching = true;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if app.focus == ListFocus::Detail {
                app.detail_scroll = app.detail_scroll.saturating_add(1);
            } else {
                move_selection(app, 1);
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.focus == ListFocus::Detail {
                app.detail_scroll = app.detail_scroll.saturating_sub(1);
            } else {
                move_selection(app, -1);
            }
        }
        KeyCode::Char('d') if ctrl => move_selection(app, 5),
        KeyCode::Char('u') if ctrl => move_selection(app, -5),
        KeyCode::Char('g') => select_index(app, 0),
        KeyCode::Char('G') => select_index(app, app.filtered.len().saturating_sub(1)),
        KeyCode::Enter => connect_selected(app, terminal, ConnectMode::Inline)?,
        KeyCode::Char('t') => connect_selected(app, terminal, ConnectMode::NewWtTab)?,
        KeyCode::Char('o') => {
            if let Some(h) = app.selected_host() {
                app.menu_sel = 0;
                open_overlay(app, Screen::ActionMenu(h));
            }
        }
        KeyCode::Char('c') => copy_command(app),
        KeyCode::Char('e') => open_edit(app),
        KeyCode::Char('a') => open_add(app),
        KeyCode::Char('d') => {
            if let Some(h) = app.selected_host() {
                let item = app.host_items[h];
                open_confirm(app, ConfirmAction::DeleteHost(item));
            }
        }
        KeyCode::Char('r') => app.refresh_all_liveness(),
        KeyCode::Char('R') => {
            if let Some(h) = app.selected_host() {
                app.refresh_one_liveness(h);
            }
        }
        KeyCode::Char('K') => {
            app.key_host_ctx = app.selected_host();
            app.screen = Screen::KeyManager;
        }
        KeyCode::Char('H') => {
            // Reload so the indices used for deletion are fresh at open time.
            app.reload_known_hosts();
            app.screen = Screen::KnownHosts;
        }
        KeyCode::Char('P') => open_vault(app),
        _ => {}
    }
    Ok(())
}

fn handle_list_search(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Char('c') if ctrl => {
            app.searching = false;
            open_confirm(app, ConfirmAction::Quit);
        }
        KeyCode::Esc => {
            app.searching = false;
            app.search.clear();
            app.refilter();
        }
        KeyCode::Enter => {
            app.searching = false;
        }
        KeyCode::Backspace => {
            app.search.pop();
            app.refilter();
        }
        KeyCode::Down => move_selection(app, 1),
        KeyCode::Up => move_selection(app, -1),
        // Only unmodified characters are typed into the filter.
        KeyCode::Char(c) if !ctrl && !alt => {
            app.search.push(c);
            app.refilter();
        }
        _ => {}
    }
}

fn move_selection(app: &mut App, delta: i32) {
    if app.filtered.is_empty() {
        return;
    }
    let len = app.filtered.len() as i32;
    let cur = app.list_state.selected().unwrap_or(0) as i32;
    let next = (cur + delta).clamp(0, len - 1);
    app.list_state.select(Some(next as usize));
    app.detail_scroll = 0;
}

fn select_index(app: &mut App, idx: usize) {
    if app.filtered.is_empty() {
        return;
    }
    app.list_state.select(Some(idx.min(app.filtered.len() - 1)));
    app.detail_scroll = 0;
}

// ---------------------------------------------------------------------------
// Connecting
// ---------------------------------------------------------------------------

/// What connect dispatch should do for a host, once the candidacy + gate inputs
/// are known. Pure decision (computed by [`connect_plan`]); the caller executes
/// it. v1 NOTE: a locked vault yields `candidacy == None` upstream (see
/// `App::vault_secret_kinds`), so there is deliberately no unlock-on-connect
/// branch — the user unlocks first to arm auto-fill. The spec's
/// locked→unlock→auto-fill is deferred because candidacy cannot be known while
/// locked without prompting for the master password on *every* connect (even
/// keyless hosts).
#[derive(Debug, PartialEq, Eq)]
enum ConnectPlan {
    /// Connect normally, no auto-fill. `Some(msg)` = emit this non-error toast
    /// (the TOFU not-yet-trusted nudge); `None` = silent (no candidacy match, a
    /// proxied host, a failed resolve, or a Match-exec degrade).
    Normal(Option<String>),
    /// Show the one-time password-confirm modal first, then connect.
    DeferPasswordConfirm(MatchedKinds),
    /// Arm these kinds and connect with auto-fill.
    Arm(MatchedKinds),
}

/// The pure connect-dispatch gate sequence (spec "Connect-time flow" steps 1–5):
/// candidacy → Match-exec degrade → resolve → proxy skip → TOFU known-host gate →
/// password-confirm gate → arm. Every miss degrades to a normal connect (fails
/// safe — never arms). `candidacy` is the opt-in-masked `vault_secret_kinds`
/// result (so password is already dropped when disabled / vault locked).
fn connect_plan(
    candidacy: Option<MatchedKinds>,
    host_has_match_exec: bool,
    resolved: bool,
    is_proxied: bool,
    is_known: bool,
    password_confirmed: bool,
    alias: &str,
) -> ConnectPlan {
    let Some(kinds) = candidacy else {
        return ConnectPlan::Normal(None); // no match (or vault locked) — silent
    };
    if host_has_match_exec || !resolved || is_proxied {
        return ConnectPlan::Normal(None); // degrade silently (no env)
    }
    if !is_known {
        // BLANKET gate, load-bearing for BOTH kinds — not just the server-facing
        // password. Arming sets `SSH_ASKPASS_REQUIRE=force`, which routes the
        // host-key "Are you sure you want to continue connecting" prompt to our
        // helper too; the helper classifies it as `Other` and replies empty, so an
        // unknown host would fail with "Host key verification failed" before any
        // secret prompt (validated by the Step 0 spike). So we must NOT arm an
        // unknown host even for a (local) passphrase: leave the first connect
        // un-armed so the user accepts the key on the TTY, then auto-fill on reconnect.
        return ConnectPlan::Normal(Some(format!(
            "host key not yet trusted for {alias} — accept it at the prompt, then reconnect to auto-fill the stored secret"
        )));
    }
    if kinds.password && !password_confirmed {
        return ConnectPlan::DeferPasswordConfirm(kinds);
    }
    ConnectPlan::Arm(kinds)
}

/// Apply the per-connect password decision to the opt-in-masked candidacy: a
/// `Withheld` decline drops the Password kind (so a password-only host degrades to
/// `None` and a both-kinds host keeps only the passphrase); `Ask`/`Confirmed` pass
/// through. Pure, so the release-gating mask is unit-tested directly.
fn apply_password_choice(
    kinds: Option<MatchedKinds>,
    choice: PasswordChoice,
) -> Option<MatchedKinds> {
    let mut k = kinds?;
    if choice == PasswordChoice::Withheld {
        k.password = false;
    }
    k.any().then_some(k)
}

/// Whether a stored **password** may arm this attempt: `Confirmed` always (the
/// modal was accepted), `Withheld` never, `Ask` only if the resolved target is
/// already in the session-consent set. Pure; the actual insert on `Confirmed`
/// (which persists the consent) is the caller's side effect.
fn password_confirmed(
    choice: PasswordChoice,
    target: Option<&str>,
    confirmed: &std::collections::HashSet<String>,
) -> bool {
    match choice {
        PasswordChoice::Confirmed => true,
        PasswordChoice::Withheld => false,
        PasswordChoice::Ask => target.is_some_and(|t| confirmed.contains(t)),
    }
}

/// Whether connect dispatch may release a stored **password** this attempt — the
/// state carried across the one-time password-confirm modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PasswordChoice {
    /// First dispatch: defer to the confirm modal when a password is a candidate
    /// and the resolved target is not already session-confirmed.
    Ask,
    /// The modal was accepted (or the target is already confirmed) — arm it.
    Confirmed,
    /// The modal was declined — withhold the password; passphrase still arms.
    Withheld,
}

/// `t` (new-tab) auto-fill stays OFF until the `wt.exe -w 0` env-inheritance spike
/// passes (Phase 3 plan, Step 0). While false a new-tab connect skips the whole
/// resolve+arm machinery and connects plainly with no askpass env.
const NEW_TAB_AUTOFILL: bool = false;

fn connect_selected(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    mode: ConnectMode,
) -> Result<()> {
    let Some(idx) = app.selected_host() else {
        return Ok(());
    };
    let alias = app.hosts[idx].alias().to_string();
    connect_by_alias(app, terminal, &alias, mode, PasswordChoice::Ask, None)
}

/// The re-entrant connect executor (spec "Connect-time flow"): resolve the alias →
/// gate (candidacy → Match-exec → `ssh -G` → proxy → TOFU) → decide via
/// [`connect_plan`] → execute. Inline owns its askpass listener in a scope-guard;
/// every gate miss degrades to a plain connect (fails safe — never arms). `choice`
/// threads the password-confirm decision back in after the modal re-enters here.
fn connect_by_alias(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    alias: &str,
    mode: ConnectMode,
    choice: PasswordChoice,
    cached_rc: Option<ResolvedConfig>,
) -> Result<()> {
    // The host can vanish if the config changed under a deferred confirm — degrade
    // to a message rather than panic.
    let Some(host) = app.hosts.iter().find(|h| h.alias() == alias).cloned() else {
        app.toast(format!("host '{alias}' is no longer in the config"), true);
        return Ok(());
    };
    let args = build_ssh_args(&host, &ConnectOverrides::default());

    // v1 auto-fills the inline path only; a mode that won't auto-fill skips the
    // candidacy + resolve machinery entirely (no `ssh -G`, no env).
    let autofill_mode = match mode {
        ConnectMode::Inline => true,
        ConnectMode::NewWtTab => NEW_TAB_AUTOFILL,
    };
    if !autofill_mode {
        return connect_plain(app, terminal, &host, &args, mode);
    }

    // One-time discoverability nudge for a stored password while auto-fill is off
    // (computed from the *raw*, unmasked match so the opt-in mask can't hide it).
    maybe_password_discoverability(app, &host);

    // Candidacy (opt-in-masked). A decline additionally drops the Password kind.
    let mut candidacy = apply_password_choice(app.vault_secret_kinds(&host), choice);
    // (#6) OpenSSH < 8.5 lacks the `(user@host)` keyboard-interactive prefix that
    // classify() depends on, so never arm the server-facing *password* secret on
    // such a client. Passphrase auto-fill is local-only and stays enabled.
    if let Some(k) = candidacy.as_mut()
        && k.password
        && !crate::os::askpass::ssh_kbdint_prefix_supported()
    {
        k.password = false;
        app.toast(
            "password auto-fill off: OpenSSH < 8.5 can't isolate keyboard-interactive",
            false,
        );
        if !k.any() {
            candidacy = None;
        }
    }
    // No candidate secret → a plain connect (skip `ssh -G` for the common case).
    if candidacy.is_none() {
        return connect_plain(app, terminal, &host, &args, mode);
    }

    // Reuse the first-pass resolution if the modal handed it back, so the
    // Confirmed/Withheld re-entry does not run `ssh -G` (and any `Match exec`
    // predicate) a second time. Otherwise resolve now — but a `Match exec`
    // anywhere in the config would make `ssh -G` execute a predicate, so skip the
    // resolve entirely and degrade.
    let (host_has_match_exec, rc) = match cached_rc {
        Some(rc) => (false, Some(rc)),
        None => {
            let mex = has_match_exec(&app.config.render());
            let rc = (!mex).then(|| resolve_config(alias).ok()).flatten();
            (mex, rc)
        }
    };
    let resolved = rc.is_some();
    let is_proxied = host.is_proxied()
        || rc
            .as_ref()
            .is_some_and(|rc| rc.proxy_jump.is_some() || rc.proxy_command.is_some());
    let is_known = match rc.as_ref() {
        Some(rc) => {
            tofu_lookup_key(rc).is_some_and(|key| is_host_known(&key, &known_hosts_files(rc)))
        }
        None => false,
    };

    // Resolve the confirm target + fold in the session set / explicit choice. A
    // Confirmed choice persists the consent so a later Ask connect to the same
    // resolved target skips the modal.
    let target = rc.as_ref().map(resolved_target);
    if choice == PasswordChoice::Confirmed
        && let Some(t) = &target
    {
        app.confirmed_password_targets.insert(t.clone());
    }
    let password_confirmed =
        password_confirmed(choice, target.as_deref(), &app.confirmed_password_targets);

    match connect_plan(
        candidacy,
        host_has_match_exec,
        resolved,
        is_proxied,
        is_known,
        password_confirmed,
        alias,
    ) {
        ConnectPlan::Normal(msg) => {
            if let Some(m) = msg {
                app.toast(m, false);
            }
            connect_plain(app, terminal, &host, &args, mode)
        }
        ConnectPlan::DeferPasswordConfirm(kinds) => {
            // Show the one-time consent modal; its Enter/Esc re-enters here with
            // Confirmed/Withheld, reusing the cached `rc` so we don't resolve twice.
            // Only reachable from `Ask`, and DeferPasswordConfirm implies resolved.
            let rc = rc.expect("DeferPasswordConfirm implies a resolved config");
            let target = target.unwrap_or_else(|| alias.to_string());
            open_overlay(
                app,
                Screen::PasswordConfirm {
                    alias: alias.to_string(),
                    mode,
                    kinds,
                    target,
                    rc: Box::new(rc),
                },
            );
            Ok(())
        }
        ConnectPlan::Arm(kinds) => {
            // `Arm` implies `resolved`, so `rc` is Some; new-tab returned early.
            let rc = rc.expect("Arm implies a resolved config");
            arm_and_connect_inline(app, terminal, &host, &args, alias, &rc, kinds)
        }
    }
}

/// Connect with no auto-fill: the non-candidate path and every gate-miss degrade.
fn connect_plain(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    host: &HostView,
    args: &[String],
    mode: ConnectMode,
) -> Result<()> {
    match mode {
        ConnectMode::Inline => {
            suspend_tui(terminal)?;
            let status = run_ssh_inline(args, &[]);
            restore_tui(terminal)?;
            report_plain_exit(app, status);
        }
        ConnectMode::NewWtTab => match connect_new_tab(host.alias(), args, &[]) {
            Ok(()) => app.toast(format!("opened new tab: ssh {}", host.alias()), false),
            Err(e) => app.toast(format!("{e}"), true),
        },
    }
    Ok(())
}

/// Arm the askpass listener, run `ssh` inline with the force env, tear the listener
/// down (zeroizing secrets) and surface the combined exit+outcome toast. The
/// listener is a scope-guard: its `Drop` stops+joins+zeroizes even on an early
/// return or panic before `stop_and_join`.
fn arm_and_connect_inline(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    host: &HostView,
    args: &[String],
    alias: &str,
    rc: &ResolvedConfig,
    kinds: MatchedKinds,
) -> Result<()> {
    let (password, passphrase) = gather_secrets(app, host, kinds);
    // Nothing actually resolved to a servable secret → just connect plainly.
    if password.is_none() && passphrase.is_none() {
        return connect_plain(app, terminal, host, args, ConnectMode::Inline);
    }
    let identity = resolved_identity(rc, alias, &os_tokens());

    match arm_connect(identity, password, passphrase) {
        Ok((listener, env)) => {
            suspend_tui(terminal)?;
            let status = run_ssh_inline(args, &env);
            restore_tui(terminal)?;
            let outcome = listener.stop_and_join();
            let toast = match status {
                Ok(s) => connect_toast(alias, s.code(), &outcome),
                Err(e) => Some((format!("failed to launch ssh: {e}"), true)),
            };
            if let Some((msg, is_err)) = toast {
                app.toast(msg, is_err);
            }
        }
        Err(e) => {
            // Arming failed (e.g. listener bind) → degrade to a plain connect.
            app.toast(
                format!("auto-fill unavailable ({e}); connecting without it"),
                false,
            );
            connect_plain(app, terminal, host, args, ConnectMode::Inline)?;
        }
    }
    Ok(())
}

/// Pull the armed secret material out of the unlocked vault for `host`, limited to
/// the `kinds` the plan armed and matched by the SAME candidacy logic as
/// [`match_vault_kinds`] (non-glob patterns, exact host). Returns owned `Secret`s
/// (they zeroize on drop); the listener still enforces the identity binding before
/// any of it is released.
fn gather_secrets(
    app: &App,
    host: &HostView,
    kinds: MatchedKinds,
) -> (Option<Secret>, Option<Secret>) {
    let Some(vault) = app.vault.as_ref() else {
        return (None, None);
    };
    let (mut password, mut passphrase) = (None, None);
    for pat in &host.patterns {
        if pat.contains(['*', '?', '!']) {
            continue;
        }
        for e in vault.secrets_for_host(pat) {
            match e.kind {
                SecretKind::Password if kinds.password && password.is_none() => {
                    password = Some(e.secret.clone());
                }
                SecretKind::Passphrase if kinds.passphrase && passphrase.is_none() => {
                    passphrase = Some(e.secret.clone());
                }
                _ => {}
            }
        }
    }
    (password, passphrase)
}

/// The resolved `<user@host>` the password-confirm consent is keyed on — the same
/// host token OpenSSH puts in the prompt (HostKeyAlias verbatim, else the resolved
/// hostname).
fn resolved_target(rc: &ResolvedConfig) -> String {
    let user = rc.user.as_deref().unwrap_or("");
    let host = rc
        .host_key_alias
        .as_deref()
        .or(rc.hostname.as_deref())
        .unwrap_or("");
    format!("{user}@{host}")
}

/// The known_hosts files `ssh -G` reported for a resolved host (user + global).
fn known_hosts_files(rc: &ResolvedConfig) -> Vec<String> {
    let mut files = rc.user_known_hosts_files.clone();
    files.extend(rc.global_known_hosts_files.iter().cloned());
    files
}

/// One-time-per-session nudge: a host has a stored password but connect-time
/// password auto-fill is off, so the user might not realize they could use it.
fn maybe_password_discoverability(app: &mut App, host: &HostView) {
    if app.password_autofill_enabled || app.password_hint_shown {
        return;
    }
    let Some(vault) = app.vault.as_ref() else {
        return;
    };
    let has_password =
        match_vault_kinds(&host.patterns, &vault.entries).is_some_and(|k| k.password);
    if has_password {
        app.password_hint_shown = true;
        app.toast(
            "this host has a stored password — press P then p to enable password auto-fill",
            false,
        );
    }
}

/// Surface a plain (no auto-fill) inline ssh exit as a toast.
fn report_plain_exit(app: &mut App, status: std::io::Result<std::process::ExitStatus>) {
    match status {
        Ok(s) => {
            if let Some((msg, is_err)) = describe_exit(&s) {
                app.toast(msg, is_err);
            }
        }
        Err(e) => app.toast(format!("failed to launch ssh: {e}"), true),
    }
}

/// The connect-time auto-fill outcome toast for `alias`: combine the ssh exit
/// `code` with the listener `outcome`, exhaustively over the reachable outcomes so
/// a failed connect is diagnosable. `None` = no toast (a clean exit 0 with nothing
/// notable, e.g. key auth that never prompted).
fn connect_toast(alias: &str, code: Option<i32>, outcome: &Outcome) -> Option<(String, bool)> {
    match outcome {
        Outcome::Served { kind } => {
            let k = kind.label().to_ascii_lowercase();
            match code {
                Some(0) => Some((format!("auto-filled {k} · connected"), false)),
                Some(255) => Some((
                    format!("auto-filled {k}, but ssh authentication/connection failed"),
                    true,
                )),
                _ => describe_exit_code(code),
            }
        }
        Outcome::Declined {
            reason: DeclineReason::KeyboardInteractive,
        } => Some((
            "server used keyboard-interactive — stored password withheld; type it manually".into(),
            false,
        )),
        // A stored secret was a candidate but never released (withheld at the
        // confirm modal, or no prompt matched the resolved identity), and auth
        // failed — under `force` ssh gives no manual prompt this connect.
        Outcome::Declined {
            reason: DeclineReason::NoMatch,
        } if code == Some(255) => Some((
            format!(
                "auth failed for {alias} — the stored secret was withheld or didn't match; press P to copy it, or reconnect to auto-fill"
            ),
            true,
        )),
        // The stored secret was never even requested (a key or other factor was
        // tried first and failed).
        Outcome::NotAttempted if code == Some(255) => Some((
            format!(
                "auth failed for {alias} — the stored secret was never requested (key/other factor failed)"
            ),
            true,
        )),
        Outcome::Declined { .. } | Outcome::TimedOut | Outcome::NotAttempted => {
            describe_exit_code(code)
        }
    }
}

fn suspend_tui(terminal: &mut DefaultTerminal) -> Result<()> {
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen, Show)?;
    let _ = terminal;
    Ok(())
}

fn restore_tui(terminal: &mut DefaultTerminal) -> Result<()> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    terminal.clear()?;
    Ok(())
}

fn copy_command(app: &mut App) {
    let Some(idx) = app.selected_host() else {
        return;
    };
    let host = app.hosts[idx].clone();
    let cmd = command_line(&host, &ConnectOverrides::default());
    match copy_to_clipboard(&cmd) {
        Ok(()) => app.toast("ssh command copied", false),
        Err(e) => app.toast(format!("clipboard error: {e}"), true),
    }
}

fn copy_to_clipboard(text: &str) -> Result<()> {
    let mut cb = arboard::Clipboard::new()?;
    // Pass the borrowed text through (Cow::Borrowed) so we don't make an extra
    // owned, un-zeroized String copy of a vault secret on our own heap — arboard
    // borrows it straight into the OS clipboard buffer.
    cb.set_text(std::borrow::Cow::Borrowed(text))?;
    Ok(())
}

/// How long a copied vault secret lingers before auto-clear. Kept short because
/// the moment a secret enters the OS clipboard it is outside our zeroized memory.
const CLIPBOARD_CLEAR_AFTER: Duration = Duration::from_secs(12);

/// Called each tick: once the auto-clear deadline passes, wipe the clipboard
/// (only if it still holds the copied secret). Every step is best-effort.
pub fn tick_clipboard(app: &mut App) {
    if let Some(at) = app.clipboard_clear_at
        && Instant::now() >= at
    {
        force_clear_clipboard(app);
    }
}

/// Wipe the clipboard now if it still holds the secret we copied, and forget the
/// pending clear. Used at the tick deadline and on quit (which stops the tick).
pub fn force_clear_clipboard(app: &mut App) {
    let key = app.clipboard_hash_key;
    let target = app.clipboard_hash;
    // Still holds our secret, or we couldn't read it back → try to wipe it.
    let ours = clipboard_hash(key).map(|h| h == target).unwrap_or(true);
    if ours && clear_clipboard().is_err() {
        // Transient failure (clipboard momentarily locked): keep the deadline so
        // the next tick retries, rather than giving up with the secret still set.
        return;
    }
    app.clipboard_clear_at = None;
    app.clipboard_hash = 0;
}

fn clear_clipboard() -> Result<()> {
    arboard::Clipboard::new()?.set_text(String::new())?;
    Ok(())
}

/// Keyed hash of the current clipboard contents, without retaining the plaintext.
fn clipboard_hash(key: u64) -> Option<u64> {
    let txt = Zeroizing::new(arboard::Clipboard::new().ok()?.get_text().ok()?);
    Some(hash_secret(key, &txt))
}

/// A per-session **keyed** hash, so the value held in memory is not a
/// stand-alone, precomputable digest of the secret on its own.
fn hash_secret(key: u64, s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    s.hash(&mut h);
    h.finish()
}

// ---------------------------------------------------------------------------
// S2 — edit / add form
// ---------------------------------------------------------------------------

fn open_edit(app: &mut App) {
    let Some(idx) = app.selected_host() else {
        return;
    };
    let view = app.hosts[idx].clone();
    app.form = form_from_view(&view);
    let item = app.host_items[idx];
    app.screen = Screen::Edit {
        editing: Some(item),
    };
}

fn open_add(app: &mut App) {
    app.form = form_from_view(&HostView::default());
    app.screen = Screen::Edit { editing: None };
}

fn handle_edit(app: &mut App, key: KeyEvent, editing: Option<usize>) -> Result<()> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && matches!(key.code, KeyCode::Char('s')) {
        if app.form.mode == FormMode::Editing {
            app.form.mode = FormMode::Navigate;
        }
        save_form(app, editing);
        return Ok(());
    }

    if app.form.mode == FormMode::Editing {
        handle_edit_typing(app, key);
    } else {
        handle_edit_navigate(app, key, editing);
    }
    Ok(())
}

/// `&mut` access to the text currently being edited (single value or active
/// row). Defensively guarantees a valid row index for multi fields.
fn active_text(app: &mut App) -> (&mut String, &mut usize) {
    let f = &mut app.form.fields[app.form.focused];
    if f.multi {
        if f.rows.is_empty() {
            f.rows.push(String::new());
        }
        if f.row_sel >= f.rows.len() {
            f.row_sel = f.rows.len() - 1;
        }
        (&mut f.rows[f.row_sel], &mut f.cursor)
    } else {
        (&mut f.value, &mut f.cursor)
    }
}

fn handle_edit_typing(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => app.form.mode = FormMode::Navigate,
        KeyCode::Esc => {
            // Revert the field to its pre-edit value.
            let backup = app.form.edit_backup.clone();
            let (s, cursor) = active_text(app);
            *s = backup;
            *cursor = s.len();
            app.form.mode = FormMode::Navigate;
        }
        KeyCode::Left => {
            let (s, cursor) = active_text(app);
            *cursor = prev_boundary(s, *cursor);
        }
        KeyCode::Right => {
            let (s, cursor) = active_text(app);
            *cursor = next_boundary(s, *cursor);
        }
        KeyCode::Home => {
            let (_, cursor) = active_text(app);
            *cursor = 0;
        }
        KeyCode::End => {
            let (s, cursor) = active_text(app);
            *cursor = s.len();
        }
        KeyCode::Backspace => {
            let (s, cursor) = active_text(app);
            backspace(s, cursor);
        }
        KeyCode::Delete => {
            let (s, cursor) = active_text(app);
            delete_forward(s, cursor);
        }
        KeyCode::Char(c) => {
            let (s, cursor) = active_text(app);
            insert_char(s, cursor, c);
        }
        _ => {}
    }
}

fn handle_edit_navigate(app: &mut App, key: KeyEvent, editing: Option<usize>) {
    let nfields = app.form.fields.len();
    match key.code {
        KeyCode::Esc => {
            if form_is_dirty(app) {
                open_confirm(app, ConfirmAction::DiscardEdit);
            } else {
                app.screen = Screen::List;
            }
        }
        KeyCode::Tab | KeyCode::Down | KeyCode::Char('j') => {
            app.form.focused = (app.form.focused + 1) % nfields;
        }
        KeyCode::BackTab | KeyCode::Up | KeyCode::Char('k') => {
            app.form.focused = (app.form.focused + nfields - 1) % nfields;
        }
        KeyCode::Left => {
            let f = &mut app.form.fields[app.form.focused];
            if f.multi {
                f.row_sel = f.row_sel.saturating_sub(1);
            }
        }
        KeyCode::Right => {
            let f = &mut app.form.fields[app.form.focused];
            if f.multi && !f.rows.is_empty() {
                f.row_sel = (f.row_sel + 1).min(f.rows.len() - 1);
            }
        }
        // On the picker fields, Enter opens the picker; `i` edits manually.
        KeyCode::Enter if app.form.focused == form_idx::IDENTITY => {
            app.reload_keys();
            app.pick_key_state
                .select((!app.keys.is_empty()).then_some(0));
            app.screen = Screen::PickKey { editing };
        }
        KeyCode::Enter if app.form.focused == form_idx::PROXYJUMP => {
            let has = !app.jump_candidates().is_empty();
            app.pick_jump_state.select(has.then_some(0));
            app.screen = Screen::PickJump { editing };
        }
        KeyCode::Enter | KeyCode::Char('i') => begin_edit(app),
        KeyCode::Char('a') => {
            let f = &mut app.form.fields[app.form.focused];
            if f.multi {
                f.rows.push(String::new());
                f.row_sel = f.rows.len() - 1;
                begin_edit(app);
            } else {
                begin_edit(app);
            }
        }
        KeyCode::Char('d') => {
            let f = &mut app.form.fields[app.form.focused];
            if f.multi && !f.rows.is_empty() {
                f.rows.remove(f.row_sel);
                if f.row_sel > 0 && f.row_sel >= f.rows.len() {
                    f.row_sel -= 1;
                }
            }
        }
        _ => {
            let _ = editing;
        }
    }
}

fn begin_edit(app: &mut App) {
    let focused = app.form.focused;
    if is_multi(focused) && app.form.fields[focused].rows.is_empty() {
        app.form.fields[focused].rows.push(String::new());
        app.form.fields[focused].row_sel = 0;
    }
    let (s, cursor) = active_text(app);
    let end = s.len();
    let backup = s.clone();
    *cursor = end;
    app.form.edit_backup = backup;
    app.form.mode = FormMode::Editing;
}

fn form_is_dirty(app: &App) -> bool {
    view_from_form(&app.form) != app.form.original
}

fn save_form(app: &mut App, editing: Option<usize>) {
    let view = view_from_form(&app.form);

    // Validation.
    let mut errors: Vec<(usize, String)> = Vec::new();
    if view.patterns.is_empty() {
        errors.push((form_idx::HOST, "Host alias is required".into()));
    }
    if let Some(p) = &view.port
        && p.parse::<u16>().is_err()
    {
        errors.push((form_idx::PORT, "Port must be a number 1–65535".into()));
    }
    // ssh_config has no escape for a literal double-quote, so reject it rather
    // than silently corrupt the value on write/round-trip.
    for (i, f) in app.form.fields.iter().enumerate() {
        if f.value.contains('"') || f.rows.iter().any(|r| r.contains('"')) {
            errors.push((i, "value cannot contain a double-quote (\")".into()));
        }
    }
    app.form.errors = errors;
    if !app.form.errors.is_empty() {
        app.toast("fix the highlighted fields", true);
        return;
    }

    let result = match editing {
        Some(item) => app.config.apply_view(item, &view).map(|_| "host saved"),
        None => app.config.add_host(&view).map(|_| "host added"),
    };
    let msg = match result {
        Ok(m) => m,
        Err(e) => {
            app.toast(format!("{e}"), true);
            return;
        }
    };
    if let Err(e) = app.config.save() {
        app.toast(format!("save failed: {e}"), true);
        return;
    }
    app.rebuild_hosts();
    app.refresh_all_liveness();
    app.screen = Screen::List;
    app.toast(msg, false);
}

// ---------------------------------------------------------------------------
// S3 — key manager
// ---------------------------------------------------------------------------

fn handle_keys(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.screen = Screen::List,
        KeyCode::Char('?') => open_overlay(app, Screen::Help),
        KeyCode::Char('j') | KeyCode::Down => move_list(&mut app.keys_state, app.keys.len(), 1),
        KeyCode::Char('k') | KeyCode::Up => move_list(&mut app.keys_state, app.keys.len(), -1),
        KeyCode::Home => app.keys_state.select((!app.keys.is_empty()).then_some(0)),
        KeyCode::End => app.keys_state.select(app.keys.len().checked_sub(1)),
        KeyCode::Char('g') => {
            app.gen_wizard = crate::app::GenWizard::default();
            app.screen = Screen::GenerateKey {
                origin: GenOrigin::KeyManager,
            };
        }
        KeyCode::Char('y') => copy_public_key(app),
        KeyCode::Char('s') => set_identity_for_host(app),
        KeyCode::Char('d') => {
            if let Some(sel) = app.keys_state.selected() {
                open_confirm(app, ConfirmAction::RemoveKey(sel));
            }
        }
        KeyCode::Char('r') => {
            app.reload_keys();
            app.toast("keys reloaded", false);
        }
        _ => {}
    }
    Ok(())
}

fn copy_public_key(app: &mut App) {
    let Some(k) = app.keys_state.selected().and_then(|i| app.keys.get(i)) else {
        return;
    };
    match read_public_key(k)
        .map_err(anyhow::Error::from)
        .and_then(|t| copy_to_clipboard(&t))
    {
        Ok(()) => app.toast("public key copied", false),
        Err(e) => app.toast(format!("{e}"), true),
    }
}

fn set_identity_for_host(app: &mut App) {
    let Some(host_idx) = app.key_host_ctx else {
        app.toast("open Keys from a host (K) to set its IdentityFile", true);
        return;
    };
    let Some(k) = app.keys_state.selected().and_then(|i| app.keys.get(i)) else {
        return;
    };
    if !k.has_private {
        app.toast("no private key file for this entry", true);
        return;
    }
    let id_path = tildify(&k.private_path());
    let Some(&item) = app.host_items.get(host_idx) else {
        return;
    };
    let mut view = app.hosts[host_idx].clone();
    view.identity_files = vec![id_path.clone()];
    if let Err(e) = app.config.apply_view(item, &view) {
        app.toast(format!("{e}"), true);
        return;
    }
    if let Err(e) = app.config.save() {
        app.toast(format!("save failed: {e}"), true);
        return;
    }
    app.rebuild_hosts();
    app.refresh_all_liveness();
    app.screen = Screen::List;
    app.toast(format!("IdentityFile set to {id_path}"), false);
}

/// Convert a path under the home directory into a `~/...` form for the config.
fn tildify(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display()).replace('\\', "/");
    }
    path.display().to_string()
}

fn handle_gen_wizard(app: &mut App, key: KeyEvent, origin: GenOrigin) {
    let w = &mut app.gen_wizard;
    match key.code {
        KeyCode::Esc => {
            app.screen = gen_return_screen(&origin);
            return;
        }
        KeyCode::Tab | KeyCode::Down => {
            w.field = (w.field + 1) % 3;
            return;
        }
        KeyCode::BackTab | KeyCode::Up => {
            w.field = (w.field + 2) % 3;
            return;
        }
        KeyCode::Enter => {
            run_generate(app, origin);
            return;
        }
        _ => {}
    }

    match w.field {
        0 => {
            if matches!(
                key.code,
                KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right
            ) {
                w.key_type = match w.key_type {
                    crate::os::keys::KeyType::Ed25519 => crate::os::keys::KeyType::Rsa4096,
                    crate::os::keys::KeyType::Rsa4096 => crate::os::keys::KeyType::Ed25519,
                };
                // Offer a matching default filename if still on a default.
                if w.filename == "id_ed25519" || w.filename == "id_rsa" {
                    w.filename = match w.key_type {
                        crate::os::keys::KeyType::Ed25519 => "id_ed25519".into(),
                        crate::os::keys::KeyType::Rsa4096 => "id_rsa".into(),
                    };
                    w.filename_cursor = w.filename.len();
                }
            }
        }
        1 => edit_wizard_field(&mut w.filename, &mut w.filename_cursor, key),
        2 => edit_wizard_field(&mut w.comment, &mut w.comment_cursor, key),
        _ => {}
    }
}

fn edit_wizard_field(s: &mut String, cursor: &mut usize, key: KeyEvent) {
    match key.code {
        KeyCode::Left => *cursor = prev_boundary(s, *cursor),
        KeyCode::Right => *cursor = next_boundary(s, *cursor),
        KeyCode::Home => *cursor = 0,
        KeyCode::End => *cursor = s.len(),
        KeyCode::Backspace => backspace(s, cursor),
        KeyCode::Delete => delete_forward(s, cursor),
        KeyCode::Char(c) => insert_char(s, cursor, c),
        _ => {}
    }
}

/// Screen the generate-key wizard returns to when cancelled.
fn gen_return_screen(origin: &GenOrigin) -> Screen {
    match origin {
        GenOrigin::KeyManager => Screen::KeyManager,
        GenOrigin::EditForm { editing } => Screen::Edit { editing: *editing },
    }
}

/// Append a path as a new IdentityFile row in the edit form and focus it.
fn add_identity_row(app: &mut App, path: &str) {
    let Some(f) = app.form.fields.get_mut(form_idx::IDENTITY) else {
        return;
    };
    f.rows.push(path.to_string());
    f.row_sel = f.rows.len() - 1;
    app.form.focused = form_idx::IDENTITY;
}

fn run_generate(app: &mut App, origin: GenOrigin) {
    let w = app.gen_wizard.clone();
    let filename = w.filename.trim();
    if filename.is_empty() {
        app.toast("filename is required", true);
        return;
    }
    let Some(dir) = ssh_dir() else {
        app.toast("cannot resolve ~/.ssh", true);
        return;
    };
    let out = dir.join(filename);
    match generate_key(w.key_type, &out, w.comment.trim()) {
        Ok(()) => {
            app.reload_keys();
            match origin {
                GenOrigin::KeyManager => {
                    app.screen = Screen::KeyManager;
                    app.toast(format!("generated {filename}"), false);
                }
                GenOrigin::EditForm { editing } => {
                    let id_path = tildify(&out);
                    add_identity_row(app, &id_path);
                    app.screen = Screen::Edit { editing };
                    app.toast(format!("generated {filename} → IdentityFile"), false);
                }
            }
        }
        Err(e) => app.toast(format!("{e}"), true),
    }
}

/// Key picker modal (opened from the edit form's IdentityFile field).
fn handle_pick_key(app: &mut App, key: KeyEvent, editing: Option<usize>) {
    match key.code {
        KeyCode::Esc => app.screen = Screen::Edit { editing },
        KeyCode::Char('g') => {
            app.gen_wizard = crate::app::GenWizard::default();
            app.screen = Screen::GenerateKey {
                origin: GenOrigin::EditForm { editing },
            };
        }
        KeyCode::Char('j') | KeyCode::Down => move_list(&mut app.pick_key_state, app.keys.len(), 1),
        KeyCode::Char('k') | KeyCode::Up => move_list(&mut app.pick_key_state, app.keys.len(), -1),
        KeyCode::Enter => {
            if let Some(sel) = app
                .pick_key_state
                .selected()
                .filter(|&s| s < app.keys.len())
            {
                let id_path = tildify(&app.keys[sel].private_path());
                add_identity_row(app, &id_path);
                app.toast(format!("added {id_path}"), false);
            }
            app.screen = Screen::Edit { editing };
        }
        _ => {}
    }
}

/// Host picker modal (opened from the edit form's ProxyJump field).
fn handle_pick_jump(app: &mut App, key: KeyEvent, editing: Option<usize>) {
    let candidates = app.jump_candidates();
    match key.code {
        KeyCode::Esc => app.screen = Screen::Edit { editing },
        KeyCode::Char('j') | KeyCode::Down => {
            move_list(&mut app.pick_jump_state, candidates.len(), 1)
        }
        KeyCode::Char('k') | KeyCode::Up => {
            move_list(&mut app.pick_jump_state, candidates.len(), -1)
        }
        KeyCode::Enter => {
            if let Some(&host_idx) = app
                .pick_jump_state
                .selected()
                .and_then(|s| candidates.get(s))
            {
                let alias = app.hosts[host_idx].alias().to_string();
                if let Some(field) = app.form.fields.get_mut(form_idx::PROXYJUMP) {
                    field.value = alias.clone();
                    field.cursor = field.value.len();
                }
                app.form.focused = form_idx::PROXYJUMP;
                app.toast(format!("ProxyJump → {alias}"), false);
            }
            app.screen = Screen::Edit { editing };
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// S4 — known_hosts
// ---------------------------------------------------------------------------

fn handle_known_hosts(app: &mut App, key: KeyEvent) {
    if app.kh_searching {
        match key.code {
            KeyCode::Esc => {
                app.kh_searching = false;
                app.kh_search.clear();
                app.clamp_kh_selection();
            }
            KeyCode::Enter => app.kh_searching = false,
            KeyCode::Backspace => {
                app.kh_search.pop();
                app.clamp_kh_selection();
            }
            KeyCode::Char(c) => {
                app.kh_search.push(c);
                app.clamp_kh_selection();
            }
            _ => {}
        }
        return;
    }

    let filtered_len = app.kh_filtered().len();
    match key.code {
        KeyCode::Esc => {
            if !app.kh_search.is_empty() {
                app.kh_search.clear();
                app.clamp_kh_selection();
            } else {
                app.screen = Screen::List;
            }
        }
        KeyCode::Char('?') => open_overlay(app, Screen::Help),
        KeyCode::Char('/') => app.kh_searching = true,
        KeyCode::Char('j') | KeyCode::Down => move_list(&mut app.kh_state, filtered_len, 1),
        KeyCode::Char('k') | KeyCode::Up => move_list(&mut app.kh_state, filtered_len, -1),
        KeyCode::Char('g') => app.kh_state.select((filtered_len > 0).then_some(0)),
        KeyCode::Char('G') => app.kh_state.select(filtered_len.checked_sub(1)),
        KeyCode::Char('d') => {
            if let Some(sel) = app.kh_state.selected()
                && let Some(&entry_idx) = app.kh_filtered().get(sel)
            {
                let entry = &app.known_hosts[entry_idx];
                let action = ConfirmAction::RemoveKnownHost {
                    line_no: entry.line_no,
                    raw: entry.raw.clone(),
                };
                open_confirm(app, action);
            }
        }
        KeyCode::Char('r') => {
            app.reload_known_hosts();
            app.toast("known_hosts reloaded", false);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Password vault
// ---------------------------------------------------------------------------

/// Enter the vault. If it's already unlocked this session, go straight to the
/// list; otherwise open the master-password prompt (in "create" mode when no
/// vault file exists yet).
fn open_vault(app: &mut App) {
    if let Some(v) = &app.vault {
        let has_entries = !v.entries.is_empty();
        app.vault_state.select(has_entries.then_some(0));
        // Re-entering the vault starts masked: reveal is a per-visit opt-in, so a
        // reveal left on from an earlier visit never silently shows secrets here.
        app.vault_reveal = false;
        app.screen = Screen::Vault;
        return;
    }
    let path = vault::default_path();
    // Recover a crash-orphaned backup before deciding create-vs-unlock. If a
    // backup exists but can't be restored, a vault DOES exist — never offer to
    // create over it (a create would delete the backup); surface the error and
    // fall into unlock mode instead.
    let exists = match &path {
        Some(p) => match vault::recover_backup(p) {
            Ok(()) => p.exists(),
            Err(e) => {
                app.toast(format!("{e}"), true);
                true
            }
        },
        None => false,
    };
    // Struct-update (`..Default::default()`) can't move out of a Drop type.
    let mut u = VaultUnlock::default();
    u.creating = !exists;
    app.vault_unlock = u;
    open_overlay(app, Screen::VaultUnlock);
}

fn handle_vault(app: &mut App, key: KeyEvent) {
    let len = app.vault.as_ref().map(|v| v.entries.len()).unwrap_or(0);
    match key.code {
        KeyCode::Esc => {
            // Leaving the vault clears reveal, so the next visit is masked again.
            app.vault_reveal = false;
            app.screen = Screen::List;
        }
        KeyCode::Char('?') => open_overlay(app, Screen::Help),
        KeyCode::Char('j') | KeyCode::Down => move_list(&mut app.vault_state, len, 1),
        KeyCode::Char('k') | KeyCode::Up => move_list(&mut app.vault_state, len, -1),
        KeyCode::Char('g') => app.vault_state.select((len > 0).then_some(0)),
        KeyCode::Char('G') => app.vault_state.select(len.checked_sub(1)),
        KeyCode::Char(' ') => app.vault_reveal = !app.vault_reveal,
        KeyCode::Char('a') => open_vault_entry(app, None),
        KeyCode::Char('e') | KeyCode::Enter => {
            if let Some(sel) = app.vault_state.selected().filter(|&s| s < len) {
                open_vault_entry(app, Some(sel));
            }
        }
        KeyCode::Char('y') | KeyCode::Char('c') => copy_vault_secret(app),
        KeyCode::Char('d') => {
            if let Some(sel) = app.vault_state.selected().filter(|&s| s < len) {
                open_confirm(app, ConfirmAction::DeleteVaultEntry(sel));
            }
        }
        KeyCode::Char('p') => toggle_password_autofill(app),
        KeyCode::Char('L') => {
            // Lock: drop the decrypted vault from memory, and forget the session's
            // password-confirm consents (they must not survive a re-unlock).
            app.vault = None;
            app.vault_reveal = false;
            app.confirmed_password_targets.clear();
            app.screen = Screen::List;
            app.toast("vault locked", false);
        }
        _ => {}
    }
}

/// Flip the session-scoped connect-time **password** auto-fill opt-in. Off by
/// default because the password method is server-facing under `SSH_ASKPASS=force`
/// and a server offering keyboard-interactive (not `password`) would burn an auth
/// attempt; passphrase auto-fill is always on and unaffected. Enabling also still
/// requires a one-time per-target confirm at connect time.
fn toggle_password_autofill(app: &mut App) {
    app.password_autofill_enabled = !app.password_autofill_enabled;
    if app.password_autofill_enabled {
        app.toast(
            "password auto-fill ON — armed per-host after a one-time confirm; passphrases were already on",
            false,
        );
    } else {
        // Disabling drops any consents so re-enabling re-asks per target.
        app.confirmed_password_targets.clear();
        app.toast("password auto-fill OFF", false);
    }
}

fn copy_vault_secret(app: &mut App) {
    let Some(secret) = app
        .vault
        .as_ref()
        .zip(app.vault_state.selected())
        .and_then(|(v, sel)| v.entries.get(sel))
        .map(|e| Zeroizing::new(e.secret.as_str().to_owned()))
    else {
        return;
    };
    match crate::os::clipboard::set_secret(secret.as_str()) {
        Ok(()) => {
            app.clipboard_hash = hash_secret(app.clipboard_hash_key, secret.as_str());
            app.clipboard_clear_at = Some(Instant::now() + CLIPBOARD_CLEAR_AFTER);
            app.toast(
                format!(
                    "secret copied — auto-clears in {}s (a clipboard-history manager may still retain it)",
                    CLIPBOARD_CLEAR_AFTER.as_secs()
                ),
                false,
            );
        }
        Err(e) => app.toast(format!("clipboard error: {e}"), true),
    }
}

fn handle_vault_unlock(app: &mut App, key: KeyEvent) {
    let creating = app.vault_unlock.creating;
    match key.code {
        KeyCode::Esc => {
            app.vault_unlock = VaultUnlock::default();
            close_overlay(app);
        }
        KeyCode::Tab | KeyCode::Down | KeyCode::Up if creating => {
            app.vault_unlock.field ^= 1;
            let u = &mut app.vault_unlock;
            u.cursor = if u.field == 0 {
                u.password.len()
            } else {
                u.confirm.len()
            };
        }
        KeyCode::Enter => submit_vault_unlock(app),
        KeyCode::Backspace => {
            let u = &mut app.vault_unlock;
            let s = if u.field == 0 {
                &mut u.password
            } else {
                &mut u.confirm
            };
            backspace_secret(s, &mut u.cursor);
        }
        KeyCode::Char(c) => {
            let u = &mut app.vault_unlock;
            let s = if u.field == 0 {
                &mut u.password
            } else {
                &mut u.confirm
            };
            insert_char_secret(s, &mut u.cursor, c);
        }
        _ => {}
    }
}

fn submit_vault_unlock(app: &mut App) {
    let Some(path) = vault::default_path() else {
        app.toast("cannot resolve ~/.ssh", true);
        return;
    };
    let creating = app.vault_unlock.creating;
    if app.vault_unlock.password.is_empty() {
        app.toast("master password is required", true);
        return;
    }
    if creating && app.vault_unlock.password != app.vault_unlock.confirm {
        app.toast("passwords do not match", true);
        return;
    }

    // Borrow the password directly — never clone the whole unlock struct (which
    // would scatter another un-scrubbed copy of the master password on the heap).
    let result = if creating {
        Vault::create(&app.vault_unlock.password).and_then(|v| v.save(&path).map(|()| v))
    } else {
        Vault::unlock(&path, &app.vault_unlock.password)
    };

    match result {
        Ok(v) => {
            let n = v.entries.len();
            app.vault = Some(v);
            app.vault_state.select((n > 0).then_some(0));
            // Replacing the struct drops the old one, whose Drop scrubs the password.
            app.vault_unlock = VaultUnlock::default();
            app.prev_screen = None;
            app.screen = Screen::Vault;
            app.toast(
                if creating {
                    "vault created"
                } else {
                    "vault unlocked"
                },
                false,
            );
        }
        Err(e) => {
            // Keep the prompt open; scrub the typed password so the user can retry.
            app.vault_unlock.password.zeroize();
            app.vault_unlock.confirm.zeroize();
            app.vault_unlock.cursor = 0;
            app.vault_unlock.field = 0;
            app.toast(format!("{e}"), true);
        }
    }
}

/// Open the entry form to add (`None`) or edit (`Some(idx)`) a vault entry.
fn open_vault_entry(app: &mut App, editing: Option<usize>) {
    let form = match editing.and_then(|i| app.vault.as_ref().and_then(|v| v.entries.get(i))) {
        Some(e) => VaultEntryForm {
            editing,
            host: e.host.clone(),
            kind: e.kind,
            secret: e.secret.as_str().to_string(),
            note: e.note.clone(),
            field: 0,
            cursor: e.host.len(),
        },
        None => {
            // Pre-fill the host from the current list selection, if any.
            let host = app
                .selected_host()
                .and_then(|i| app.hosts.get(i))
                .map(|h| h.alias().to_string())
                .unwrap_or_default();
            let mut form = VaultEntryForm::default();
            form.cursor = host.len();
            form.host = host;
            form
        }
    };
    app.vault_entry = form;
    app.screen = Screen::VaultEntry { editing };
}

fn handle_vault_entry(app: &mut App, key: KeyEvent, editing: Option<usize>) {
    const NFIELDS: usize = 4; // host, kind, secret, note
    match key.code {
        KeyCode::Esc => {
            app.vault_entry = VaultEntryForm::default();
            app.screen = Screen::Vault;
            return;
        }
        KeyCode::Enter => {
            save_vault_entry(app, editing);
            return;
        }
        KeyCode::Tab | KeyCode::Down => {
            app.vault_entry.field = (app.vault_entry.field + 1) % NFIELDS;
            sync_vault_entry_cursor(app);
            return;
        }
        KeyCode::BackTab | KeyCode::Up => {
            app.vault_entry.field = (app.vault_entry.field + NFIELDS - 1) % NFIELDS;
            sync_vault_entry_cursor(app);
            return;
        }
        _ => {}
    }

    // The kind field is a toggle, not a text field.
    if app.vault_entry.field == 1 {
        if matches!(
            key.code,
            KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right
        ) {
            app.vault_entry.kind = app.vault_entry.kind.toggled();
        }
        return;
    }

    let f = &mut app.vault_entry;
    // Field 2 is the secret; its edits go through the zeroizing variants so a
    // typed passphrase never leaves an un-scrubbed copy on the heap.
    let secret_field = f.field == 2;
    let s = match f.field {
        0 => &mut f.host,
        2 => &mut f.secret,
        _ => &mut f.note,
    };
    match key.code {
        KeyCode::Left => f.cursor = prev_boundary(s, f.cursor),
        KeyCode::Right => f.cursor = next_boundary(s, f.cursor),
        KeyCode::Home => f.cursor = 0,
        KeyCode::End => f.cursor = s.len(),
        KeyCode::Backspace if secret_field => backspace_secret(s, &mut f.cursor),
        KeyCode::Backspace => backspace(s, &mut f.cursor),
        KeyCode::Delete if secret_field => delete_forward_secret(s, &mut f.cursor),
        KeyCode::Delete => delete_forward(s, &mut f.cursor),
        KeyCode::Char(c) if secret_field => insert_char_secret(s, &mut f.cursor, c),
        KeyCode::Char(c) => insert_char(s, &mut f.cursor, c),
        _ => {}
    }
}

/// Re-anchor the cursor to the end of the newly focused text field.
fn sync_vault_entry_cursor(app: &mut App) {
    let f = &mut app.vault_entry;
    f.cursor = match f.field {
        0 => f.host.len(),
        2 => f.secret.len(),
        3 => f.note.len(),
        _ => f.cursor,
    };
}

fn save_vault_entry(app: &mut App, editing: Option<usize>) {
    let Some(path) = vault::default_path() else {
        app.toast("cannot resolve ~/.ssh", true);
        return;
    };
    let host = app.vault_entry.host.trim().to_string();
    if host.is_empty() {
        app.toast("host is required", true);
        return;
    }
    if app.vault_entry.secret.is_empty() {
        app.toast("secret is required", true);
        return;
    }
    // A secret with a newline/CR (or over OpenSSH's 1023-byte read cap) cannot be
    // delivered by the connect-time auto-fill helper, so reject it at save time
    // rather than store a value that would be silently truncated on the channel.
    if let Err(e) = vault::reject_unservable_secret(&app.vault_entry.secret) {
        app.toast(format!("{e}"), true);
        return;
    }
    let entry = VaultEntry {
        host,
        kind: app.vault_entry.kind,
        secret: app.vault_entry.secret.clone().into(),
        note: app.vault_entry.note.trim().to_string(),
    };
    let Some(v) = app.vault.as_mut() else {
        app.toast("vault is locked", true);
        return;
    };
    // Persist with rollback: on a save failure the in-memory entry is reverted so
    // the list never shows an add/edit that is not actually on disk.
    if let Err(e) = v.upsert_and_save(editing, entry, &path) {
        app.toast(format!("save failed: {e}"), true);
        return;
    }
    let n = v.entries.len();
    app.vault_entry = VaultEntryForm::default();
    app.vault_state
        .select(Some(editing.unwrap_or(n - 1).min(n - 1)));
    app.screen = Screen::Vault;
    app.toast("secret saved", false);
}

// ---------------------------------------------------------------------------
// Overlays: help, confirm, action menu
// ---------------------------------------------------------------------------

fn handle_help(app: &mut App, key: KeyEvent) {
    if matches!(
        key.code,
        KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')
    ) {
        close_overlay(app);
    }
}

fn handle_confirm(app: &mut App, key: KeyEvent, action: ConfirmAction) -> Result<()> {
    match key.code {
        KeyCode::Char('y') | KeyCode::Enter => {
            perform_confirm(app, action);
        }
        KeyCode::Char('n') | KeyCode::Esc => close_overlay(app),
        _ => {}
    }
    Ok(())
}

fn perform_confirm(app: &mut App, action: ConfirmAction) {
    match action {
        ConfirmAction::Quit => {
            // Quitting stops the tick that auto-clears the clipboard, so wipe any
            // still-pending copied secret now rather than leaving it behind.
            if app.clipboard_clear_at.is_some() {
                force_clear_clipboard(app);
            }
            app.should_quit = true;
        }
        ConfirmAction::DiscardEdit => {
            app.screen = Screen::List;
            app.prev_screen = None;
        }
        ConfirmAction::DeleteHost(item) => {
            let result = app.config.delete_host(item).and_then(|_| app.config.save());
            match result {
                Ok(()) => {
                    app.rebuild_hosts();
                    app.refresh_all_liveness();
                    app.screen = Screen::List;
                    app.prev_screen = None;
                    app.toast("host deleted", false);
                }
                Err(e) => {
                    app.screen = Screen::List;
                    app.toast(format!("{e}"), true);
                }
            }
        }
        ConfirmAction::RemoveKey(sel) => {
            remove_key_files(app, sel);
            app.screen = Screen::KeyManager;
            app.prev_screen = None;
        }
        ConfirmAction::RemoveKnownHost { line_no, raw } => {
            match remove_entry(line_no, &raw) {
                Ok(()) => {
                    app.reload_known_hosts();
                    app.toast("entry removed", false);
                }
                Err(e) => app.toast(format!("{e}"), true),
            }
            app.screen = Screen::KnownHosts;
            app.prev_screen = None;
        }
        ConfirmAction::DeleteVaultEntry(idx) => {
            delete_vault_entry(app, idx);
            app.screen = Screen::Vault;
            app.prev_screen = None;
        }
    }
}

fn delete_vault_entry(app: &mut App, idx: usize) {
    let Some(path) = vault::default_path() else {
        app.toast("cannot resolve ~/.ssh", true);
        return;
    };
    let Some(v) = app.vault.as_mut() else {
        return;
    };
    // Persist with rollback: on a save failure the removal is reverted in memory
    // so the list never shows a deletion that did not actually reach disk.
    if let Err(e) = v.remove_and_save(idx, &path) {
        app.toast(format!("save failed: {e}"), true);
        return;
    }
    let n = v.entries.len();
    if n == 0 {
        app.vault_state.select(None);
    } else {
        let sel = app.vault_state.selected().unwrap_or(0).min(n - 1);
        app.vault_state.select(Some(sel));
    }
    app.toast("secret deleted", false);
}

fn remove_key_files(app: &mut App, sel: usize) {
    let Some(k) = app.keys.get(sel) else {
        return;
    };
    let pub_path = k.pub_path.clone();
    let priv_path = k.private_path();
    let mut errors = Vec::new();
    if let Some(pub_path) = pub_path
        && let Err(e) = std::fs::remove_file(&pub_path)
    {
        errors.push(format!("pub: {e}"));
    }
    // `symlink_metadata` (unlike `exists`) does not follow the link, so a
    // broken-symlink private key is still detected and removed.
    if priv_path.symlink_metadata().is_ok()
        && let Err(e) = std::fs::remove_file(&priv_path)
    {
        errors.push(format!("priv: {e}"));
    }
    app.reload_keys();
    if errors.is_empty() {
        app.toast("key deleted", false);
    } else {
        app.toast(format!("delete error: {}", errors.join("; ")), true);
    }
}

fn handle_action_menu(
    app: &mut App,
    key: KeyEvent,
    host_idx: usize,
    terminal: &mut DefaultTerminal,
) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('o') => close_overlay(app),
        KeyCode::Char('j') | KeyCode::Down => {
            app.menu_sel = (app.menu_sel + 1) % ACTION_LABELS.len();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.menu_sel = (app.menu_sel + ACTION_LABELS.len() - 1) % ACTION_LABELS.len();
        }
        KeyCode::Enter => {
            let sel = app.menu_sel;
            // Ensure the list selection matches the menu's host.
            if let Some(row) = app.filtered.iter().position(|&i| i == host_idx) {
                app.list_state.select(Some(row));
            }
            match sel {
                // Delete opens its confirm WHILE the action menu is still the
                // current screen, so cancelling the confirm returns to the menu.
                4 => {
                    let item = app.host_items[host_idx];
                    open_confirm(app, ConfirmAction::DeleteHost(item));
                }
                _ => {
                    close_overlay(app);
                    match sel {
                        0 => connect_selected(app, terminal, ConnectMode::Inline)?,
                        1 => connect_selected(app, terminal, ConnectMode::NewWtTab)?,
                        2 => copy_command(app),
                        3 => open_edit(app),
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn move_list(state: &mut ratatui::widgets::ListState, len: usize, delta: i32) {
    if len == 0 {
        state.select(None);
        return;
    }
    let cur = state.selected().unwrap_or(0) as i32;
    let next = (cur + delta).clamp(0, len as i32 - 1);
    state.select(Some(next as usize));
}

// ---------------------------------------------------------------------------
// Screen-transition helpers
// ---------------------------------------------------------------------------

fn open_overlay(app: &mut App, overlay: Screen) {
    app.prev_screen = Some(app.screen.clone());
    app.screen = overlay;
}

fn open_confirm(app: &mut App, action: ConfirmAction) {
    app.prev_screen = Some(app.screen.clone());
    app.screen = Screen::Confirm(action);
}

fn close_overlay(app: &mut App) {
    if let Some(prev) = app.prev_screen.take() {
        app.screen = prev;
    } else {
        app.screen = Screen::List;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The secret-field editing helpers must behave identically to the plain
    // ones — they only differ in scrubbing the freed heap buffer, which must not
    // change what the user sees. These guard that the zeroizing rebuild keeps the
    // value (and cursor) correct, including across capacity growth and on
    // multibyte boundaries.

    fn type_str(insert: impl Fn(&mut String, &mut usize, char), text: &str) -> (String, usize) {
        let mut s = String::new();
        let mut cur = 0;
        for c in text.chars() {
            insert(&mut s, &mut cur, c);
        }
        (s, cur)
    }

    #[test]
    fn insert_char_secret_matches_plain_across_reallocs() {
        // 300 chars forces several growth reallocations through the zeroizing path.
        let long: String = "passwörd-🔐-".chars().cycle().take(300).collect();
        let (plain, pc) = type_str(insert_char, &long);
        let (secret, sc) = type_str(insert_char_secret, &long);
        assert_eq!(plain, long);
        assert_eq!(secret, plain);
        assert_eq!(sc, pc);
        assert_eq!(sc, long.len());
    }

    #[test]
    fn insert_char_secret_inserts_at_cursor() {
        let mut s = "abef".to_string();
        let mut cur = 2; // between 'b' and 'e'
        insert_char_secret(&mut s, &mut cur, 'c');
        insert_char_secret(&mut s, &mut cur, 'd');
        assert_eq!(s, "abcdef");
        assert_eq!(cur, 4);
    }

    #[test]
    fn backspace_and_delete_secret_match_plain() {
        let start = "héllo🔐wörld";
        // Walk a cursor backwards deleting, comparing both variants step by step.
        for cut in [3usize, 6, 10] {
            let cut = (0..=start.len())
                .rev()
                .find(|&i| i <= cut && start.is_char_boundary(i))
                .unwrap();
            let (mut a, mut ca) = (start.to_string(), cut);
            let (mut b, mut cb) = (start.to_string(), cut);
            backspace(&mut a, &mut ca);
            backspace_secret(&mut b, &mut cb);
            assert_eq!(a, b, "backspace mismatch at {cut}");
            assert_eq!(ca, cb);

            let (mut c, mut cc) = (start.to_string(), cut);
            let (mut d, mut cd) = (start.to_string(), cut);
            delete_forward(&mut c, &mut cc);
            delete_forward_secret(&mut d, &mut cd);
            assert_eq!(c, d, "delete mismatch at {cut}");
            assert_eq!(cc, cd);
        }
    }

    #[test]
    fn remove_range_secret_keeps_remaining_value() {
        let mut s = "secret-value".to_string();
        remove_range_secret(&mut s, 0..7); // drop "secret-"
        assert_eq!(s, "value");
        // Capacity preserved so subsequent edits do not immediately realloc.
        assert!(s.capacity() >= "secret-value".len());
    }

    #[test]
    fn connect_plan_gates() {
        let both = MatchedKinds {
            password: true,
            passphrase: true,
        };
        let pp_only = MatchedKinds {
            password: false,
            passphrase: true,
        };

        // No candidacy (no match, or vault locked) -> normal, silent.
        assert_eq!(
            connect_plan(None, false, true, false, true, false, "h"),
            ConnectPlan::Normal(None)
        );
        // Match-exec on the host -> degrade silently (no ssh -G side effects).
        assert_eq!(
            connect_plan(Some(both), true, true, false, true, false, "h"),
            ConnectPlan::Normal(None)
        );
        // resolve failed/timed out -> degrade silently.
        assert_eq!(
            connect_plan(Some(both), false, false, false, true, false, "h"),
            ConnectPlan::Normal(None)
        );
        // proxied -> degrade silently (permanent skip).
        assert_eq!(
            connect_plan(Some(both), false, true, true, true, false, "h"),
            ConnectPlan::Normal(None)
        );
        // not yet known (TOFU) -> normal WITH the nudge toast.
        match connect_plan(Some(both), false, true, false, false, false, "h") {
            ConnectPlan::Normal(Some(msg)) => assert!(msg.contains("not yet trusted")),
            other => panic!("expected Normal(Some), got {other:?}"),
        }
        // passphrase-only + not known -> STILL normal+nudge, never armed. The TOFU
        // gate is blanket because arming sets force, which hijacks the host-key
        // prompt; an unknown host must not be armed even for a local passphrase.
        match connect_plan(Some(pp_only), false, true, false, false, false, "h") {
            ConnectPlan::Normal(Some(msg)) => assert!(msg.contains("not yet trusted")),
            other => panic!("expected Normal(Some) for passphrase-only/not-known, got {other:?}"),
        }
        // password armed + known + not yet confirmed -> defer to the confirm modal.
        assert_eq!(
            connect_plan(Some(both), false, true, false, true, false, "h"),
            ConnectPlan::DeferPasswordConfirm(both)
        );
        // password armed + already confirmed -> arm.
        assert_eq!(
            connect_plan(Some(both), false, true, false, true, true, "h"),
            ConnectPlan::Arm(both)
        );
        // passphrase only (no password kind) -> arm directly, no confirm.
        assert_eq!(
            connect_plan(Some(pp_only), false, true, false, true, false, "h"),
            ConnectPlan::Arm(pp_only)
        );
    }

    #[test]
    fn apply_password_choice_drops_password_only_on_withhold() {
        let both = MatchedKinds {
            password: true,
            passphrase: true,
        };
        let pw_only = MatchedKinds {
            password: true,
            passphrase: false,
        };
        let pp_only = MatchedKinds {
            password: false,
            passphrase: true,
        };
        // Ask / Confirmed pass the candidacy through unchanged.
        assert_eq!(
            apply_password_choice(Some(both), PasswordChoice::Ask),
            Some(both)
        );
        assert_eq!(
            apply_password_choice(Some(both), PasswordChoice::Confirmed),
            Some(both)
        );
        // Withheld drops the password: both -> passphrase-only, password-only -> None.
        assert_eq!(
            apply_password_choice(Some(both), PasswordChoice::Withheld),
            Some(pp_only)
        );
        assert_eq!(
            apply_password_choice(Some(pw_only), PasswordChoice::Withheld),
            None
        );
        assert_eq!(
            apply_password_choice(Some(pp_only), PasswordChoice::Withheld),
            Some(pp_only)
        );
        assert_eq!(apply_password_choice(None, PasswordChoice::Ask), None);
    }

    #[test]
    fn password_confirmed_reads_the_session_consent_set() {
        let mut set = std::collections::HashSet::new();
        set.insert("deploy@web1".to_string());
        // Confirmed always; Withheld never (regardless of the set).
        assert!(password_confirmed(PasswordChoice::Confirmed, None, &set));
        assert!(!password_confirmed(
            PasswordChoice::Withheld,
            Some("deploy@web1"),
            &set
        ));
        // Ask: only when the resolved target is already consented this session.
        assert!(password_confirmed(
            PasswordChoice::Ask,
            Some("deploy@web1"),
            &set
        ));
        assert!(!password_confirmed(
            PasswordChoice::Ask,
            Some("deploy@other"),
            &set
        ));
        assert!(!password_confirmed(PasswordChoice::Ask, None, &set));
    }

    #[test]
    fn resolved_target_uses_host_key_alias_then_hostname() {
        // HostKeyAlias wins (verbatim) — it is what OpenSSH's prompt carries.
        let rc = ResolvedConfig {
            user: Some("deploy".into()),
            hostname: Some("10.0.0.5".into()),
            host_key_alias: Some("prod-db".into()),
            ..Default::default()
        };
        assert_eq!(resolved_target(&rc), "deploy@prod-db");
        // No alias -> the resolved hostname.
        let rc = ResolvedConfig {
            user: Some("deploy".into()),
            hostname: Some("web1.example.com".into()),
            ..Default::default()
        };
        assert_eq!(resolved_target(&rc), "deploy@web1.example.com");
        // Missing user/host degrade to empty halves (still a stable key).
        assert_eq!(resolved_target(&ResolvedConfig::default()), "@");
    }

    #[test]
    fn known_hosts_files_concatenates_user_then_global() {
        let rc = ResolvedConfig {
            user_known_hosts_files: vec!["u1".into(), "u2".into()],
            global_known_hosts_files: vec!["g1".into()],
            ..Default::default()
        };
        assert_eq!(known_hosts_files(&rc), vec!["u1", "u2", "g1"]);
    }

    #[test]
    fn connect_toast_maps_outcome_and_exit() {
        use crate::os::askpass::{DeclineReason, Outcome};
        // Served + clean exit -> a success toast naming the kind.
        let t = connect_toast(
            "h",
            Some(0),
            &Outcome::Served {
                kind: SecretKind::Passphrase,
            },
        );
        assert_eq!(
            t,
            Some(("auto-filled passphrase · connected".into(), false))
        );
        // Served + 255 -> served-but-auth-failed (error).
        let t = connect_toast(
            "h",
            Some(255),
            &Outcome::Served {
                kind: SecretKind::Password,
            },
        );
        assert!(t.is_some_and(|(m, e)| e && m.contains("auto-filled password")));
        // Keyboard-interactive decline -> an informational withheld toast.
        let t = connect_toast(
            "h",
            Some(255),
            &Outcome::Declined {
                reason: DeclineReason::KeyboardInteractive,
            },
        );
        assert!(t.is_some_and(|(m, e)| !e && m.contains("keyboard-interactive")));
        // Withheld / no-match + 255 -> a decline-aware, alias-named diagnostic.
        let t = connect_toast(
            "web1",
            Some(255),
            &Outcome::Declined {
                reason: DeclineReason::NoMatch,
            },
        );
        assert!(t.is_some_and(|(m, e)| e && m.contains("web1") && m.contains("withheld")));
        // NotAttempted + 255 -> a "never requested", alias-named diagnostic.
        let t = connect_toast("web1", Some(255), &Outcome::NotAttempted);
        assert!(t.is_some_and(|(m, e)| e && m.contains("web1") && m.contains("never requested")));
        // Nothing served, clean exit (key auth never prompted) -> no toast.
        assert_eq!(connect_toast("h", Some(0), &Outcome::NotAttempted), None);
        // A detached/stalled teardown (TimedOut) folds into the exit summary:
        // 255 -> error toast, clean exit -> no toast.
        assert!(connect_toast("h", Some(255), &Outcome::TimedOut).is_some_and(|(_, e)| e));
        assert_eq!(connect_toast("h", Some(0), &Outcome::TimedOut), None);
    }
}
