//! Central application state plus the screen/mode enums that drive both
//! rendering ([`crate::ui`]) and input dispatch ([`crate::update`]).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use anyhow::Context;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::widgets::{ListState, TableState};
use zeroize::Zeroize;

use crate::config::SshConfig;
use crate::config::model::HostView;
use crate::os::keys::KeyInfo;
use crate::os::known_hosts::KnownHostEntry;
use crate::os::liveness::{Liveness, LivenessProbe, ProbeTarget};
use crate::os::vault::{MatchedKinds, SecretKind, Vault, match_vault_kinds};
use crate::os::{self, keys, known_hosts};

/// Ordered labels of the edit-form fields. Indices are referenced by name in
/// [`FormIdx`].
pub const FIELD_LABELS: [&str; 10] = [
    "Host (alias / patterns)",
    "HostName",
    "User",
    "Port",
    "IdentityFile",
    "ProxyJump",
    "LocalForward",
    "RemoteForward",
    "DynamicForward",
    "Extra options (Key Value)",
];

/// Symbolic indices into the edit form's `fields` vector.
pub mod form_idx {
    pub const HOST: usize = 0;
    pub const HOSTNAME: usize = 1;
    pub const USER: usize = 2;
    pub const PORT: usize = 3;
    pub const IDENTITY: usize = 4;
    pub const PROXYJUMP: usize = 5;
    pub const LOCAL_FWD: usize = 6;
    pub const REMOTE_FWD: usize = 7;
    pub const DYNAMIC_FWD: usize = 8;
    pub const EXTRAS: usize = 9;
}

/// Field indices that hold a list of rows rather than a single value.
pub const MULTI_FIELDS: [usize; 5] = [
    form_idx::IDENTITY,
    form_idx::LOCAL_FWD,
    form_idx::REMOTE_FWD,
    form_idx::DYNAMIC_FWD,
    form_idx::EXTRAS,
];

pub fn is_multi(field: usize) -> bool {
    MULTI_FIELDS.contains(&field)
}

/// Top-level screen; drives both rendering and key dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    List,
    /// `editing = Some(item_index)` edits an existing host; `None` adds new.
    Edit {
        editing: Option<usize>,
    },
    KeyManager,
    KnownHosts,
    Help,
    Confirm(ConfirmAction),
    ActionMenu(usize),
    GenerateKey {
        origin: GenOrigin,
    },
    /// Key picker modal, opened from the edit form's IdentityFile field.
    /// Carries the edited host so it can return to the right form.
    PickKey {
        editing: Option<usize>,
    },
    /// Host picker modal, opened from the edit form's ProxyJump field, to choose
    /// a registered host as the jump host.
    PickJump {
        editing: Option<usize>,
    },
    /// Password vault: list of stored secrets (login passwords / passphrases).
    Vault,
    /// Master-password prompt modal — unlock an existing vault, or create one.
    VaultUnlock,
    /// Add / edit a vault entry. `editing = Some(idx)` edits in place.
    VaultEntry {
        editing: Option<usize>,
    },
}

/// Where the generate-key wizard was opened from — drives where it returns and
/// whether the freshly generated key is wired into the edit form's IdentityFile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenOrigin {
    KeyManager,
    EditForm { editing: Option<usize> },
}

/// What a confirm popup performs on "yes".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    DeleteHost(usize),
    RemoveKey(usize),
    /// Remove a known_hosts entry, content-addressed by its verbatim line so a
    /// stale index can't delete the wrong key after an external file change.
    RemoveKnownHost {
        line_no: usize,
        raw: String,
    },
    DiscardEdit,
    /// Delete the vault entry at this index.
    DeleteVaultEntry(usize),
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectMode {
    Inline,
    NewWtTab,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListFocus {
    Hosts,
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormMode {
    #[default]
    Navigate,
    Editing,
}

#[derive(Debug, Default, Clone)]
pub struct FormField {
    pub label: String,
    pub value: String,
    pub cursor: usize,
    pub multi: bool,
    pub rows: Vec<String>,
    pub row_sel: usize,
}

/// In-progress edit form. Built from a [`HostView`]; written back on save.
#[derive(Debug, Default)]
pub struct EditForm {
    pub fields: Vec<FormField>,
    pub focused: usize,
    pub mode: FormMode,
    pub errors: Vec<(usize, String)>,
    pub original: HostView,
    /// Pre-edit snapshot of the active field, restored on Esc.
    pub edit_backup: String,
}

/// Transient one-line status / toast.
#[derive(Debug, Default, Clone)]
pub struct Toast {
    pub text: String,
    pub is_error: bool,
    pub shown_at: Option<Instant>,
}

/// Generate-key wizard state (modal over the Key Manager).
#[derive(Debug, Clone)]
pub struct GenWizard {
    pub key_type: keys::KeyType,
    pub filename: String,
    pub filename_cursor: usize,
    pub comment: String,
    pub comment_cursor: usize,
    pub field: usize, // 0 = type, 1 = filename, 2 = comment
}

impl Default for GenWizard {
    fn default() -> Self {
        Self {
            key_type: keys::KeyType::Ed25519,
            filename: "id_ed25519".to_string(),
            filename_cursor: "id_ed25519".len(),
            comment: String::new(),
            comment_cursor: 0,
            field: 0,
        }
    }
}

/// Master-password prompt state (modal). Doubles as the "create vault" form,
/// in which case the confirm field is also shown. The typed password is scrubbed
/// on drop and redacted in `Debug` so it never lingers or leaks.
#[derive(Default, Clone)]
pub struct VaultUnlock {
    /// True when no vault file exists yet — collect + confirm a new password.
    pub creating: bool,
    pub password: String,
    pub confirm: String,
    /// 0 = password, 1 = confirm (only reachable while `creating`).
    pub field: usize,
    pub cursor: usize,
}

impl Drop for VaultUnlock {
    fn drop(&mut self) {
        self.password.zeroize();
        self.confirm.zeroize();
    }
}

impl std::fmt::Debug for VaultUnlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultUnlock")
            .field("creating", &self.creating)
            .field("password", &"***")
            .field("confirm", &"***")
            .field("field", &self.field)
            .field("cursor", &self.cursor)
            .finish()
    }
}

/// Add/edit form for a single vault entry (modal over the vault list). The typed
/// secret is scrubbed on drop and redacted in `Debug`.
#[derive(Default, Clone)]
pub struct VaultEntryForm {
    /// Index into the vault being edited, or `None` when adding.
    pub editing: Option<usize>,
    pub host: String,
    pub kind: SecretKind,
    pub secret: String,
    pub note: String,
    /// 0 = host, 1 = kind, 2 = secret, 3 = note.
    pub field: usize,
    pub cursor: usize,
}

impl Drop for VaultEntryForm {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

impl std::fmt::Debug for VaultEntryForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultEntryForm")
            .field("editing", &self.editing)
            .field("host", &self.host)
            .field("kind", &self.kind)
            .field("secret", &"***")
            .field("note", &self.note)
            .field("field", &self.field)
            .field("cursor", &self.cursor)
            .finish()
    }
}

pub struct App {
    pub should_quit: bool,
    pub screen: Screen,
    pub prev_screen: Option<Screen>,

    // --- config domain ---
    pub config: SshConfig,
    pub hosts: Vec<HostView>,
    /// `config.items` index for each entry in `hosts`.
    pub host_items: Vec<usize>,

    // --- S1 list ---
    pub focus: ListFocus,
    pub list_state: TableState,
    pub detail_scroll: u16,
    pub search: String,
    pub searching: bool,
    pub filtered: Vec<usize>,

    // --- liveness (keyed by host index in `hosts`) ---
    pub liveness: HashMap<usize, Liveness>,
    pub rtt: HashMap<usize, Duration>,
    pub probes: Vec<LivenessProbe>,
    pub last_sweep: Instant,

    // --- S2 form ---
    pub form: EditForm,

    // --- S3 keys ---
    pub keys: Vec<KeyInfo>,
    pub keys_state: ListState,
    pub key_host_ctx: Option<usize>,
    pub gen_wizard: GenWizard,
    /// Selection state for the IdentityFile key picker modal.
    pub pick_key_state: ListState,
    /// Selection state for the ProxyJump host picker modal.
    pub pick_jump_state: ListState,

    // --- S4 known_hosts ---
    pub known_hosts: Vec<KnownHostEntry>,
    pub kh_state: ListState,
    pub kh_search: String,
    pub kh_searching: bool,

    // --- O3 action menu ---
    pub menu_sel: usize,

    // --- vault (password manager) ---
    /// The unlocked vault, held in memory for the session (`None` when locked).
    pub vault: Option<Vault>,
    pub vault_state: ListState,
    pub vault_unlock: VaultUnlock,
    pub vault_entry: VaultEntryForm,
    /// When true, secrets are shown in the clear instead of masked.
    pub vault_reveal: bool,
    /// Session opt-in for connect-time **password** auto-fill (off by default:
    /// the password method is server-facing and can burn an auth attempt under
    /// `force`). Passphrase auto-fill is unaffected. Not persisted across restart.
    /// Toggled with `p` on the vault screen; read by connect dispatch + the
    /// indicator (via [`App::vault_secret_kinds`]).
    pub password_autofill_enabled: bool,
    /// Resolved `<user@host>` targets the user has confirmed for connect-time
    /// **password** auto-fill this session (the one-time password-confirm modal's
    /// memory). Holds no secret — only the resolved identity string. Session-
    /// scoped: cleared on lock, on `rebuild_hosts` (a host edit could change what
    /// a target resolves to), and never persisted.
    // TODO(phase3): read by connect dispatch (T8).
    #[allow(dead_code)]
    pub confirmed_password_targets: HashSet<String>,
    /// When a vault secret was copied, the deadline to auto-clear the clipboard,
    /// plus a (non-reversible) hash of the copied secret so the clear only fires
    /// if the clipboard still holds it — never the plaintext, which stays sealed.
    pub clipboard_clear_at: Option<Instant>,
    pub clipboard_hash: u64,
    /// Per-session key mixed into `clipboard_hash`, so the value held in memory is
    /// not a stand-alone, precomputable digest of the secret.
    pub clipboard_hash_key: u64,

    // --- chrome ---
    pub toast: Toast,
    pub ssh_path_warning: bool,
    pub include_note: bool,
}

impl App {
    pub fn new(config_path: std::path::PathBuf) -> anyhow::Result<Self> {
        let config = SshConfig::load(config_path).context("loading ssh config")?;

        let keys = keys::list_keys();
        let known = known_hosts::parse_known_hosts();

        let mut app = App {
            should_quit: false,
            screen: Screen::List,
            prev_screen: None,
            config,
            hosts: Vec::new(),
            host_items: Vec::new(),
            focus: ListFocus::Hosts,
            list_state: TableState::default(),
            detail_scroll: 0,
            search: String::new(),
            searching: false,
            filtered: Vec::new(),
            liveness: HashMap::new(),
            rtt: HashMap::new(),
            probes: Vec::new(),
            last_sweep: Instant::now(),
            form: EditForm::default(),
            keys,
            keys_state: ListState::default(),
            key_host_ctx: None,
            gen_wizard: GenWizard::default(),
            pick_key_state: ListState::default(),
            pick_jump_state: ListState::default(),
            known_hosts: known,
            kh_state: ListState::default(),
            kh_search: String::new(),
            kh_searching: false,
            menu_sel: 0,
            vault: None,
            vault_state: ListState::default(),
            vault_unlock: VaultUnlock::default(),
            vault_entry: VaultEntryForm::default(),
            vault_reveal: false,
            password_autofill_enabled: false,
            confirmed_password_targets: HashSet::new(),
            clipboard_clear_at: None,
            clipboard_hash: 0,
            clipboard_hash_key: {
                let mut b = [0u8; 8];
                let _ = getrandom::getrandom(&mut b);
                u64::from_le_bytes(b)
            },
            toast: Toast::default(),
            ssh_path_warning: !os::tools().is_system32,
            include_note: false,
        };

        app.include_note = app.config.include_count() > 0;
        app.rebuild_hosts();
        if !app.hosts.is_empty() {
            app.list_state.select(Some(0));
        }
        if !app.keys.is_empty() {
            app.keys_state.select(Some(0));
        }
        if !app.known_hosts.is_empty() {
            app.kh_state.select(Some(0));
        }
        app.refresh_all_liveness();
        Ok(app)
    }

    /// Rebuild the editable host projection (and item-index map) from `config`.
    /// Liveness is keyed by host index, which can shift when hosts are added or
    /// removed, so clear it here; callers re-probe afterwards.
    pub fn rebuild_hosts(&mut self) {
        let views = self.config.host_views();
        self.host_items = views.iter().map(|(i, _)| *i).collect();
        self.hosts = views.into_iter().map(|(_, v)| v).collect();
        self.liveness.clear();
        self.rtt.clear();
        // A host edit can change what a confirmed target resolves to, so the
        // session-scoped password-confirm memory must not outlive a rebuild.
        self.confirmed_password_targets.clear();
        self.refilter();
        self.clamp_selection();
    }

    /// The shared connect-time **candidacy** predicate: which secret kinds a host
    /// would auto-fill with — used by both the pre-connect indicator and connect
    /// dispatch so they never disagree. A pure host↔vault-entry match (NOT the
    /// listener's release logic), with the password opt-in folded in: while
    /// `password_autofill_enabled` is off the Password kind is masked out (so a
    /// password-only host yields `None` and a both-kinds host downgrades to
    /// passphrase-only). `None` when the vault is locked. Reads `vault` live.
    // TODO(phase3): called by connect dispatch (T8) + the list indicator (T11).
    #[allow(dead_code)]
    pub fn vault_secret_kinds(&self, host: &HostView) -> Option<MatchedKinds> {
        let vault = self.vault.as_ref()?;
        let matched = match_vault_kinds(&host.patterns, &vault.entries);
        mask_password_kinds(matched, self.password_autofill_enabled)
    }

    /// Recompute `filtered` from `search` (fuzzy ranked; identity when empty).
    pub fn refilter(&mut self) {
        if self.search.is_empty() {
            self.filtered = (0..self.hosts.len()).collect();
            self.clamp_selection();
            return;
        }
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse(&self.search, CaseMatching::Ignore, Normalization::Smart);

        let mut scored: Vec<(usize, u32)> = Vec::new();
        let mut buf = Vec::new();
        for (i, h) in self.hosts.iter().enumerate() {
            let hay = format!(
                "{} {} {}",
                h.patterns.join(" "),
                h.host_name.as_deref().unwrap_or(""),
                h.user.as_deref().unwrap_or("")
            );
            let hs = Utf32Str::new(&hay, &mut buf);
            if let Some(score) = pattern.score(hs, &mut matcher) {
                scored.push((i, score));
            }
        }
        scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        self.filtered = scored.into_iter().map(|(i, _)| i).collect();
        self.clamp_selection();
    }

    fn clamp_selection(&mut self) {
        if self.filtered.is_empty() {
            self.list_state.select(None);
        } else {
            let sel = self.list_state.selected().unwrap_or(0);
            self.list_state
                .select(Some(sel.min(self.filtered.len() - 1)));
        }
    }

    /// Index into `hosts` of the current list selection (through `filtered`).
    pub fn selected_host(&self) -> Option<usize> {
        let row = self.list_state.selected()?;
        self.filtered.get(row).copied()
    }

    /// Indices into `hosts` usable as a ProxyJump target: concrete
    /// (wildcard-free) aliases other than the host currently being edited.
    pub fn jump_candidates(&self) -> Vec<usize> {
        let self_alias = self
            .form
            .fields
            .get(form_idx::HOST)
            .and_then(|f| f.value.split_whitespace().next())
            .unwrap_or("");
        self.hosts
            .iter()
            .enumerate()
            .filter_map(|(i, h)| {
                let a = h.alias();
                (!a.is_empty() && !a.contains(['*', '?', '!']) && a != self_alias).then_some(i)
            })
            .collect()
    }

    /// Liveness state for a host by its index in `hosts`.
    pub fn liveness_by_index(&self, host_idx: usize) -> Liveness {
        self.liveness
            .get(&host_idx)
            .copied()
            .unwrap_or(Liveness::Unknown)
    }

    /// Round-trip time for a host by its index in `hosts`.
    pub fn rtt_by_index(&self, host_idx: usize) -> Option<Duration> {
        self.rtt.get(&host_idx).copied()
    }

    /// Spawn a probe for every host (adds to any in-flight probes).
    pub fn refresh_all_liveness(&mut self) {
        let indices: Vec<usize> = (0..self.hosts.len()).collect();
        self.spawn_probe(&indices);
    }

    /// Re-probe a single host by its `hosts` index.
    pub fn refresh_one_liveness(&mut self, host_idx: usize) {
        self.spawn_probe(&[host_idx]);
    }

    fn spawn_probe(&mut self, host_indices: &[usize]) {
        let mut targets = Vec::new();
        for &i in host_indices {
            let Some(h) = self.hosts.get(i) else { continue };
            let alias = h.alias().to_string();
            let port = h
                .port
                .as_deref()
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(22);
            // Behind a proxy (ProxyJump or ProxyCommand) → a direct TCP probe
            // can't reach the real host, so skip it rather than report "down".
            let proxied = h.is_proxied();

            // Resolve a probe target: explicit HostName, else a wildcard-free alias.
            let target = match &h.host_name {
                Some(hn) if !hn.is_empty() => Some(hn.clone()),
                _ if !alias.contains(['*', '?', '!']) && !alias.is_empty() => Some(alias.clone()),
                _ => None,
            };

            match target {
                Some(t) if !proxied => {
                    self.liveness.insert(i, Liveness::Checking);
                    targets.push(ProbeTarget {
                        id: i,
                        target: t,
                        port,
                    });
                }
                _ => {
                    self.liveness.insert(i, Liveness::Skipped);
                }
            }
        }
        if !targets.is_empty() {
            self.probes.push(LivenessProbe::spawn(
                targets,
                Duration::from_millis(1500),
                8,
            ));
        }
        self.last_sweep = Instant::now();
    }

    /// Non-blocking drain of liveness results from all in-flight probes; drops
    /// probes whose channel has closed. Returns true if any results landed.
    pub fn drain_liveness(&mut self) -> bool {
        let mut results = Vec::new();
        let mut keep = Vec::new();
        for probe in std::mem::take(&mut self.probes) {
            let (mut batch, disconnected) = probe.drain();
            results.append(&mut batch);
            if !disconnected {
                keep.push(probe);
            }
        }
        self.probes = keep;

        let changed = !results.is_empty();
        for r in results {
            self.liveness.insert(r.id, r.state);
            if let Some(d) = r.rtt {
                self.rtt.insert(r.id, d);
            }
        }
        changed
    }

    /// Per-tick housekeeping: expire transient toasts. Returns true if changed.
    pub fn on_tick(&mut self) -> bool {
        if let Some(at) = self.toast.shown_at {
            // Success toasts auto-dismiss after 4s; errors stay until next key.
            if !self.toast.is_error && at.elapsed() > Duration::from_secs(4) {
                self.toast = Toast::default();
                return true;
            }
        }
        false
    }

    pub fn toast(&mut self, text: impl Into<String>, is_error: bool) {
        self.toast = Toast {
            text: text.into(),
            is_error,
            shown_at: Some(Instant::now()),
        };
    }

    pub fn clear_toast(&mut self) {
        self.toast = Toast::default();
    }

    /// Reload keys / known_hosts from disk (after generate / delete).
    pub fn reload_keys(&mut self) {
        self.keys = keys::list_keys();
        if self.keys.is_empty() {
            self.keys_state.select(None);
        } else {
            let sel = self.keys_state.selected().unwrap_or(0);
            self.keys_state.select(Some(sel.min(self.keys.len() - 1)));
        }
    }

    pub fn reload_known_hosts(&mut self) {
        self.known_hosts = known_hosts::parse_known_hosts();
        self.clamp_kh_selection();
    }

    /// Indices into `known_hosts` matching the current `kh_search` (substring on
    /// host display + key type). Identity when the search is empty.
    pub fn kh_filtered(&self) -> Vec<usize> {
        if self.kh_search.is_empty() {
            return (0..self.known_hosts.len()).collect();
        }
        let needle = self.kh_search.to_lowercase();
        self.known_hosts
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.host.display().to_lowercase().contains(&needle)
                    || e.key_type.to_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn clamp_kh_selection(&mut self) {
        let len = self.kh_filtered().len();
        if len == 0 {
            self.kh_state.select(None);
        } else {
            let sel = self.kh_state.selected().unwrap_or(0);
            self.kh_state.select(Some(sel.min(len - 1)));
        }
    }
}

/// Build an edit form from a host view (or defaults for an "add new" form).
pub fn form_from_view(view: &HostView) -> EditForm {
    let mut fields: Vec<FormField> = Vec::with_capacity(FIELD_LABELS.len());
    for (i, label) in FIELD_LABELS.iter().enumerate() {
        fields.push(FormField {
            label: label.to_string(),
            value: String::new(),
            cursor: 0,
            multi: is_multi(i),
            rows: Vec::new(),
            row_sel: 0,
        });
    }

    let set_single = |f: &mut FormField, v: &str| {
        f.value = v.to_string();
        f.cursor = v.len();
    };

    set_single(&mut fields[form_idx::HOST], &view.patterns.join(" "));
    if let Some(v) = &view.host_name {
        set_single(&mut fields[form_idx::HOSTNAME], v);
    }
    if let Some(v) = &view.user {
        set_single(&mut fields[form_idx::USER], v);
    }
    if let Some(v) = &view.port {
        set_single(&mut fields[form_idx::PORT], v);
    }
    if let Some(v) = &view.proxy_jump {
        set_single(&mut fields[form_idx::PROXYJUMP], v);
    }
    fields[form_idx::IDENTITY].rows = view.identity_files.clone();
    fields[form_idx::LOCAL_FWD].rows = view.local_forwards.clone();
    fields[form_idx::REMOTE_FWD].rows = view.remote_forwards.clone();
    fields[form_idx::DYNAMIC_FWD].rows = view.dynamic_forwards.clone();
    fields[form_idx::EXTRAS].rows = view
        .extras
        .iter()
        .map(|(k, v)| format!("{k} {v}"))
        .collect();

    EditForm {
        fields,
        focused: 0,
        mode: FormMode::Navigate,
        errors: Vec::new(),
        original: view.clone(),
        edit_backup: String::new(),
    }
}

/// Extract a host view from the current form values.
pub fn view_from_form(form: &EditForm) -> HostView {
    let opt = |s: &str| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    };
    let rows = |f: &FormField| -> Vec<String> {
        f.rows
            .iter()
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
            .collect()
    };

    let f = &form.fields;
    let patterns: Vec<String> = f[form_idx::HOST]
        .value
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();

    let extras = rows(&f[form_idx::EXTRAS])
        .into_iter()
        .filter_map(|row| {
            let mut it = row.splitn(2, char::is_whitespace);
            let k = it.next()?.to_string();
            let v = it.next().unwrap_or("").trim().to_string();
            if k.is_empty() { None } else { Some((k, v)) }
        })
        .collect();

    HostView {
        patterns,
        host_name: opt(&f[form_idx::HOSTNAME].value),
        user: opt(&f[form_idx::USER].value),
        port: opt(&f[form_idx::PORT].value),
        identity_files: rows(&f[form_idx::IDENTITY]),
        proxy_jump: opt(&f[form_idx::PROXYJUMP].value),
        local_forwards: rows(&f[form_idx::LOCAL_FWD]),
        remote_forwards: rows(&f[form_idx::REMOTE_FWD]),
        dynamic_forwards: rows(&f[form_idx::DYNAMIC_FWD]),
        extras,
    }
}

/// Apply the password-autofill opt-in mask to a raw candidacy match. While
/// password auto-fill is off, the Password kind is dropped: a password-only host
/// then yields `None`, a both-kinds host downgrades to passphrase-only.
/// Passphrase is never gated. Pure, so the masking is unit-tested directly.
// TODO(phase3): consumed by App::vault_secret_kinds.
#[allow(dead_code)]
fn mask_password_kinds(
    matched: Option<MatchedKinds>,
    password_enabled: bool,
) -> Option<MatchedKinds> {
    let mut kinds = matched?;
    if !password_enabled {
        kinds.password = false;
    }
    kinds.any().then_some(kinds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_password_kinds_applies_opt_in() {
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

        // Enabled: the match passes through unchanged.
        assert_eq!(mask_password_kinds(Some(both), true), Some(both));
        assert_eq!(mask_password_kinds(Some(pw_only), true), Some(pw_only));
        // Disabled: the Password kind is masked out.
        assert_eq!(mask_password_kinds(Some(both), false), Some(pp_only)); // both -> passphrase-only
        assert_eq!(mask_password_kinds(Some(pw_only), false), None); // password-only -> None
        assert_eq!(mask_password_kinds(Some(pp_only), false), Some(pp_only)); // passphrase unaffected
        // No candidacy match stays None regardless.
        assert_eq!(mask_password_kinds(None, true), None);
        assert_eq!(mask_password_kinds(None, false), None);
    }
}
