//! Central application state plus the screen/mode enums that drive both
//! rendering ([`crate::ui`]) and input dispatch ([`crate::update`]).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use anyhow::Context;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::widgets::{ListState, TableState};
use zeroize::Zeroize;

/// Idle period after which an unlocked vault auto-locks (drops + zeroizes). (#14)
pub const VAULT_IDLE_LOCK: Duration = Duration::from_secs(15 * 60);

use crate::config::SshConfig;
use crate::config::model::HostView;
use crate::os::connect::ConnectOverrides;
use crate::os::history::History;
use crate::os::keys::KeyInfo;
use crate::os::known_hosts::{HostSpec, KnownHostEntry};
use crate::os::liveness::{Liveness, LivenessProbe, ProbeTarget};
use crate::os::resolve::ResolvedConfig;
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

/// Ordered labels of the connect-time override form. A leaner, session-only
/// cousin of the edit form: it never persists, so it omits Host/HostName and
/// adds a Verbose toggle. Indices are named in [`override_idx`].
pub const OVERRIDE_LABELS: [&str; 9] = [
    "User",
    "Port",
    "IdentityFile",
    "ProxyJump",
    "LocalForward",
    "RemoteForward",
    "DynamicForward",
    "Extra options (Key Value)",
    "Verbose (-v)",
];

/// Symbolic indices into the override form's `fields` vector.
pub mod override_idx {
    pub const USER: usize = 0;
    pub const PORT: usize = 1;
    pub const IDENTITY: usize = 2;
    pub const PROXYJUMP: usize = 3;
    pub const LOCAL_FWD: usize = 4;
    pub const REMOTE_FWD: usize = 5;
    pub const DYNAMIC_FWD: usize = 6;
    pub const EXTRAS: usize = 7;
    /// A boolean toggle, not a text field — its `FormField` slot is a placeholder
    /// and the real state lives in [`OverrideForm::verbose`].
    pub const VERBOSE: usize = 8;
}

/// Override-form field indices holding a list of rows (the forwards + extras).
/// IdentityFile is single-valued here (one ad-hoc key per session), unlike the
/// edit form where it is multi.
pub const OVERRIDE_MULTI: [usize; 4] = [
    override_idx::LOCAL_FWD,
    override_idx::REMOTE_FWD,
    override_idx::DYNAMIC_FWD,
    override_idx::EXTRAS,
];

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
    /// Key picker modal, opened from either form's IdentityFile field. The
    /// `origin` records which form to return to and write the choice back into.
    PickKey {
        origin: PickOrigin,
    },
    /// Host picker modal, opened from either form's ProxyJump field, to choose
    /// a registered host as the jump host.
    PickJump {
        origin: PickOrigin,
    },
    /// Connect-time override form: a session-only modal that edits a
    /// [`ConnectOverrides`] for one connection without touching `~/.ssh/config`.
    /// `host` is the index into [`App::hosts`] being connected to.
    ConnectOverride {
        host: usize,
    },
    /// Password vault: list of stored secrets (login passwords / passphrases).
    Vault,
    /// Master-password prompt modal — unlock an existing vault, or create one.
    VaultUnlock,
    /// Add / edit a vault entry. `editing = Some(idx)` edits in place.
    VaultEntry {
        editing: Option<usize>,
    },
    /// One-time connect-time **password** consent modal, shown before arming a
    /// stored password the first time the resolved `<user@host>` is targeted this
    /// session. Carries **no secret** — only the resolved `target` (for display),
    /// the armed `kinds`, and the `alias`/`mode` needed to resume the connect.
    /// Enter confirms (arm the password); Esc/`n` declines (passphrase stays
    /// armed, password withheld). A consent/typo guard, not a redirect defense.
    PasswordConfirm {
        alias: String,
        mode: ConnectMode,
        kinds: MatchedKinds,
        target: String,
        /// The `ssh -G` resolution from the first (Ask) pass, cached so the
        /// Confirmed/Withheld re-entry need not re-run `ssh -G` (which would
        /// execute any `Match exec` predicate a second time). Boxed to keep the
        /// `Screen` enum small.
        rc: Box<ResolvedConfig>,
        /// Ad-hoc overrides to re-apply on the resumed connect (empty for a plain
        /// saved-host connect). Carried so the consent re-entry rebuilds the same
        /// `ssh` args it would have without the detour through the modal.
        ov: Box<ConnectOverrides>,
    },
}

/// Which form a key/host picker was opened from — drives where it returns and
/// where the picked value is written back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickOrigin {
    /// The edit form (`editing = Some(idx)` edits an existing host).
    Edit { editing: Option<usize> },
    /// The connect-time override form.
    Override,
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

/// Order the host list is sorted by while not searching (a fuzzy search supplies
/// its own ranking, so the sort only applies to the unfiltered view). Cycled with
/// `s` on the list screen. `Config` is the default — it preserves the verbatim
/// order of `Host` blocks in `~/.ssh/config`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortMode {
    /// Verbatim `~/.ssh/config` order (no reordering).
    #[default]
    Config,
    /// Most-recently connected first; never-connected hosts last.
    Recent,
    /// Alias, case-insensitive A→Z.
    Name,
    /// Reachability: up → checking → unknown → skipped → down.
    Status,
}

impl SortMode {
    /// Short label for the title bar / toast.
    pub fn label(self) -> &'static str {
        match self {
            SortMode::Config => "file",
            SortMode::Recent => "recent",
            SortMode::Name => "name",
            SortMode::Status => "status",
        }
    }

    /// The next mode in the cycle (wraps back to `Config`).
    pub fn next(self) -> SortMode {
        match self {
            SortMode::Config => SortMode::Recent,
            SortMode::Recent => SortMode::Name,
            SortMode::Name => SortMode::Status,
            SortMode::Status => SortMode::Config,
        }
    }
}

/// Sort key for [`SortMode::Status`] — lower sorts first.
fn liveness_rank(l: Liveness) -> u8 {
    match l {
        Liveness::Up => 0,
        Liveness::Checking => 1,
        Liveness::Unknown => 2,
        Liveness::Skipped => 3,
        Liveness::Down => 4,
    }
}

/// Precomputed per-host sort key, one per entry in `App::hosts` (same index).
struct SortKey {
    /// Last-connected unix seconds, or `None` if never connected.
    recency: Option<u64>,
    /// Alias, lowercased once (so the comparator doesn't re-allocate per compare).
    alias_lc: String,
    /// Reachability rank (see [`liveness_rank`]).
    rank: u8,
}

/// Order `0..keys.len()` by `sort`, tie-breaking on the original index so the
/// result is stable and deterministic. Pure (no `App`) — directly unit-tested.
fn order_by(sort: SortMode, keys: &[SortKey]) -> Vec<usize> {
    use std::cmp::Ordering;
    let mut idx: Vec<usize> = (0..keys.len()).collect();
    match sort {
        SortMode::Config => {}
        SortMode::Recent => idx.sort_by(|&a, &b| match (keys[a].recency, keys[b].recency) {
            // Both seen: most-recent (larger timestamp) first.
            (Some(x), Some(y)) => y.cmp(&x).then(a.cmp(&b)),
            // A seen host sorts ahead of a never-connected one.
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => a.cmp(&b),
        }),
        SortMode::Name => {
            idx.sort_by(|&a, &b| keys[a].alias_lc.cmp(&keys[b].alias_lc).then(a.cmp(&b)))
        }
        SortMode::Status => idx.sort_by(|&a, &b| {
            keys[a]
                .rank
                .cmp(&keys[b].rank)
                .then_with(|| keys[a].alias_lc.cmp(&keys[b].alias_lc))
                .then(a.cmp(&b))
        }),
    }
    idx
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

/// In-progress connect-time override form. Wraps a reused [`EditForm`] (so it
/// shares the field-editing machinery) plus the out-of-band `verbose` toggle and
/// the index of the host being connected to. Never written to disk.
#[derive(Debug, Default)]
pub struct OverrideForm {
    pub form: EditForm,
    pub verbose: bool,
    pub host: usize,
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
    /// Active sort for the unfiltered list (cycled with `s`).
    pub sort: SortMode,

    // --- connection history (non-secret: last-connected timestamps) ---
    pub history: History,

    // --- liveness (keyed by host index in `hosts`) ---
    pub liveness: HashMap<usize, Liveness>,
    pub rtt: HashMap<usize, Duration>,
    pub probes: Vec<LivenessProbe>,
    pub last_sweep: Instant,

    // --- S2 form ---
    pub form: EditForm,

    // --- connect-time override form ---
    pub override_form: OverrideForm,

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
    pub confirmed_password_targets: HashSet<String>,
    /// Whether the one-time-per-session "this host has a stored password — enable
    /// auto-fill" discoverability nudge has already fired (shown at most once).
    pub password_hint_shown: bool,
    /// When a vault secret was copied, the deadline to auto-clear the clipboard,
    /// plus a (non-reversible) hash of the copied secret so the clear only fires
    /// if the clipboard still holds it — never the plaintext, which stays sealed.
    pub clipboard_clear_at: Option<Instant>,
    pub clipboard_hash: u64,
    /// Per-session key mixed into `clipboard_hash`, so the value held in memory is
    /// not a stand-alone, precomputable digest of the secret.
    pub clipboard_hash_key: u64,
    /// Wall-clock of the last keypress; drives the vault idle auto-lock (#14).
    pub last_activity: Instant,

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
            sort: SortMode::default(),
            history: History::load(),
            liveness: HashMap::new(),
            rtt: HashMap::new(),
            probes: Vec::new(),
            last_sweep: Instant::now(),
            form: EditForm::default(),
            override_form: OverrideForm::default(),
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
            password_hint_shown: false,
            clipboard_clear_at: None,
            clipboard_hash: 0,
            clipboard_hash_key: {
                let mut b = [0u8; 8];
                let _ = getrandom::getrandom(&mut b);
                u64::from_le_bytes(b)
            },
            last_activity: Instant::now(),
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
    pub fn vault_secret_kinds(&self, host: &HostView) -> Option<MatchedKinds> {
        let vault = self.vault.as_ref()?;
        let matched = match_vault_kinds(&host.patterns, &vault.entries);
        mask_password_kinds(matched, self.password_autofill_enabled)
    }

    /// A cheap, in-memory approximation of "this host has a plain `known_hosts`
    /// pin", used ONLY to render the connect-time secret indicator active (a known
    /// pin → auto-fill will fire) vs muted (a candidate not yet trusted). This is
    /// deliberately NOT the authoritative connect-time TOFU gate
    /// (`os::resolve::is_host_known`, which resolves via `ssh -G` + `ssh-keygen -F`
    /// and so cannot run per render frame): it settles for a hostname match against
    /// the already-parsed `known_hosts`. Marker entries (`@revoked`/`@cert-authority`)
    /// don't count, mirroring the real gate. A hashed `known_hosts` can't be matched
    /// by name, so every candidate then shows muted (a safe, conservative hint).
    pub fn host_known_hint(&self, host: &HostView) -> bool {
        let target = host
            .host_name
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| host.alias());
        if target.is_empty() {
            return false;
        }
        self.known_hosts
            .iter()
            .any(|e| e.marker.is_none() && known_host_spec_matches(&e.host, target))
    }

    /// Recompute `filtered` from `search` (fuzzy ranked; sorted by [`App::sort`]
    /// when the search is empty).
    pub fn refilter(&mut self) {
        if self.search.is_empty() {
            self.filtered = self.sorted_indices();
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

    /// Host indices in [`App::sort`] order (the unfiltered list). The per-host
    /// sort keys (last-connected timestamp, lowercased alias, liveness rank) are
    /// computed once up front, then ordered by the pure [`order_by`] — so the
    /// ordering logic is unit-testable without building an `App`.
    fn sorted_indices(&self) -> Vec<usize> {
        if self.sort == SortMode::Config {
            return (0..self.hosts.len()).collect();
        }
        let keys: Vec<SortKey> = (0..self.hosts.len())
            .map(|i| {
                let alias = self.hosts[i].alias();
                SortKey {
                    recency: self.history.last(alias),
                    alias_lc: alias.to_ascii_lowercase(),
                    rank: liveness_rank(self.liveness_by_index(i)),
                }
            })
            .collect();
        order_by(self.sort, &keys)
    }

    /// Re-run the filter/sort while keeping the cursor on the same *host* (not the
    /// same row), so a reorder never makes the selection jump to an unrelated
    /// entry. Shared by the `s` cycle, a recorded connect, and the per-tick
    /// status re-sort.
    pub fn refilter_keeping_selection(&mut self) {
        let cur = self.selected_host();
        self.refilter();
        if let Some(h) = cur
            && let Some(pos) = self.filtered.iter().position(|&i| i == h)
        {
            self.list_state.select(Some(pos));
        }
    }

    /// After liveness results land, the `Status` sort's order is stale (it ranks by
    /// reachability, which just changed). Re-sort so the rows match the dots —
    /// only for `Status`, and never while a fuzzy search supplies its own order.
    pub fn resort_after_liveness(&mut self) {
        if self.sort == SortMode::Status && self.search.is_empty() {
            self.refilter_keeping_selection();
        }
    }

    /// Stamp `alias` as connected-now in the history (persisted best-effort). When
    /// the list is sorted by recency the just-connected host floats to the top, so
    /// re-sort and keep the same host selected.
    pub fn record_connect(&mut self, alias: &str) {
        self.history.record(alias);
        if self.sort == SortMode::Recent && self.search.is_empty() {
            self.refilter_keeping_selection();
        }
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
    /// probes whose channel has closed. Returns true if any host's reachability
    /// **rank** changed — the signal the `Status` sort needs to re-order. An
    /// RTT-only refresh or a result that re-confirms the same state returns false,
    /// so an unchanged list is not needlessly re-sorted. (The loop redraws every
    /// tick regardless, so this gates only the re-sort, not the repaint.)
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

        let mut rank_changed = false;
        for r in results {
            let prev = self.liveness.insert(r.id, r.state);
            if prev.map(liveness_rank) != Some(liveness_rank(r.state)) {
                rank_changed = true;
            }
            if let Some(d) = r.rtt {
                self.rtt.insert(r.id, d);
            }
        }
        rank_changed
    }

    /// Per-tick housekeeping: expire transient toasts and auto-lock an idle vault.
    /// Returns true if anything changed (so the caller redraws).
    pub fn on_tick(&mut self) -> bool {
        let mut changed = false;
        // Success toasts auto-dismiss after 4s; errors stay until next key.
        if let Some(at) = self.toast.shown_at
            && !self.toast.is_error
            && at.elapsed() > Duration::from_secs(4)
        {
            self.toast = Toast::default();
            changed = true;
        }
        if self.idle_autolock() {
            changed = true;
        }
        changed
    }

    /// Drop (zeroize) the vault if it has been unlocked and idle past
    /// [`VAULT_IDLE_LOCK`]. Returns true if it locked this tick. (#14)
    fn idle_autolock(&mut self) -> bool {
        if self.vault.is_some() && self.last_activity.elapsed() >= VAULT_IDLE_LOCK {
            self.vault = None;
            self.vault_reveal = false;
            self.confirmed_password_targets.clear();
            // Scrub any typed-but-unsaved secret in the entry/unlock forms too
            // (both Drop-zeroize when replaced), so a lock leaves nothing behind.
            self.vault_entry = VaultEntryForm::default();
            self.vault_unlock = VaultUnlock::default();
            // Bounce off any vault screen to the safe list view. `Screen::VaultUnlock`
            // is intentionally NOT matched: it is only reachable while the vault is
            // locked (see `open_vault`), so the `vault.is_some()` guard above already
            // excludes it — don't "fix" the apparent asymmetry.
            if matches!(self.screen, Screen::Vault | Screen::VaultEntry { .. }) {
                self.screen = Screen::List;
            }
            self.toast("vault auto-locked (idle)", false);
            return true;
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

/// Build a blank override form for the host at index `host`. Every field starts
/// empty: a blank field inherits the host's saved/effective value, and only a
/// typed field becomes an override (the UI shows the inherited value as a hint).
pub fn override_form_from_host(host: usize) -> OverrideForm {
    let fields: Vec<FormField> = OVERRIDE_LABELS
        .iter()
        .enumerate()
        .map(|(i, label)| FormField {
            label: label.to_string(),
            value: String::new(),
            cursor: 0,
            multi: OVERRIDE_MULTI.contains(&i),
            rows: Vec::new(),
            row_sel: 0,
        })
        .collect();
    OverrideForm {
        form: EditForm {
            fields,
            focused: 0,
            mode: FormMode::Navigate,
            errors: Vec::new(),
            original: HostView::default(),
            edit_backup: String::new(),
        },
        verbose: false,
        host,
    }
}

/// Build [`ConnectOverrides`] from the override form, validating exactly as the
/// edit form does (Port parses as `u16`, no embedded `"`). On the first invalid
/// field, returns its index + a message so the caller can highlight it. A blank
/// field contributes nothing — it inherits the host's saved value.
pub fn overrides_from_form(of: &OverrideForm) -> Result<ConnectOverrides, (usize, String)> {
    let f = &of.form.fields;
    let opt = |s: &str| {
        let t = s.trim();
        (!t.is_empty()).then(|| t.to_string())
    };
    let rows = |i: usize| -> Vec<String> {
        f[i].rows
            .iter()
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
            .collect()
    };

    // ssh_config has no escape for a literal double-quote, and neither does an
    // `-o key="val"` on the wire — reject rather than corrupt the connection.
    for (i, field) in f.iter().enumerate() {
        if field.value.contains('"') || field.rows.iter().any(|r| r.contains('"')) {
            return Err((i, "value cannot contain a double-quote (\")".into()));
        }
    }

    let port = match opt(&f[override_idx::PORT].value) {
        Some(p) => Some(p.parse::<u16>().map_err(|_| {
            (
                override_idx::PORT,
                "Port must be a number 1–65535".to_string(),
            )
        })?),
        None => None,
    };

    let extra_options = rows(override_idx::EXTRAS)
        .into_iter()
        .filter_map(|row| {
            let mut it = row.splitn(2, char::is_whitespace);
            let k = it.next()?.to_string();
            let v = it.next().unwrap_or("").trim().to_string();
            (!k.is_empty()).then_some((k, v))
        })
        .collect();

    Ok(ConnectOverrides {
        port,
        user: opt(&f[override_idx::USER].value),
        identity_file: opt(&f[override_idx::IDENTITY].value).map(std::path::PathBuf::from),
        proxy_jump: opt(&f[override_idx::PROXYJUMP].value),
        local_forwards: rows(override_idx::LOCAL_FWD),
        remote_forwards: rows(override_idx::REMOTE_FWD),
        dynamic_forwards: rows(override_idx::DYNAMIC_FWD),
        extra_options,
        verbose: of.verbose,
    })
}

/// Whether a `known_hosts` host field matches `target` by name (the cheap
/// indicator probe — see [`App::host_known_hint`]). Only `Plain` specs match;
/// each comma-separated token is compared case-insensitively after stripping a
/// `[host]:port` bracket. Hashed specs never match by name.
fn known_host_spec_matches(spec: &HostSpec, target: &str) -> bool {
    let HostSpec::Plain(list) = spec else {
        return false;
    };
    list.split(',').any(|tok| {
        let host = tok
            .strip_prefix('[')
            .and_then(|t| t.split("]:").next())
            .unwrap_or(tok);
        host.eq_ignore_ascii_case(target)
    })
}

/// Apply the password-autofill opt-in mask to a raw candidacy match. While
/// password auto-fill is off, the Password kind is dropped: a password-only host
/// then yields `None`, a both-kinds host downgrades to passphrase-only.
/// Passphrase is never gated. Pure, so the masking is unit-tested directly.
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

    #[test]
    fn override_form_empty_inherits_everything() {
        // A blank override form yields the default (all-inherit) overrides.
        let of = override_form_from_host(0);
        assert_eq!(overrides_from_form(&of), Ok(ConnectOverrides::default()));
    }

    #[test]
    fn override_form_maps_typed_fields() {
        let mut of = override_form_from_host(3);
        of.form.fields[override_idx::USER].value = "deploy".into();
        of.form.fields[override_idx::PORT].value = "2222".into();
        of.form.fields[override_idx::IDENTITY].value = "/k/id".into();
        of.form.fields[override_idx::PROXYJUMP].value = "bastion".into();
        of.form.fields[override_idx::LOCAL_FWD].rows = vec!["8080 localhost:80".into()];
        of.form.fields[override_idx::REMOTE_FWD].rows = vec!["9090 localhost:90".into()];
        of.form.fields[override_idx::DYNAMIC_FWD].rows = vec!["1080".into()];
        // A blank trailing row is dropped; a real one is split key/value.
        of.form.fields[override_idx::EXTRAS].rows = vec!["ForwardAgent yes".into(), "   ".into()];
        of.verbose = true;

        assert_eq!(
            overrides_from_form(&of),
            Ok(ConnectOverrides {
                port: Some(2222),
                user: Some("deploy".into()),
                identity_file: Some(std::path::PathBuf::from("/k/id")),
                proxy_jump: Some("bastion".into()),
                local_forwards: vec!["8080 localhost:80".into()],
                remote_forwards: vec!["9090 localhost:90".into()],
                dynamic_forwards: vec!["1080".into()],
                extra_options: vec![("ForwardAgent".into(), "yes".into())],
                verbose: true,
            })
        );
    }

    #[test]
    fn override_form_blank_field_is_not_an_override() {
        let mut of = override_form_from_host(0);
        of.form.fields[override_idx::USER].value = "   ".into();
        assert_eq!(
            overrides_from_form(&of).unwrap().user,
            None,
            "a whitespace-only field must inherit, not override with empty"
        );
    }

    #[test]
    fn override_form_rejects_bad_port_and_quotes() {
        let mut bad_port = override_form_from_host(0);
        bad_port.form.fields[override_idx::PORT].value = "nope".into();
        assert_eq!(
            overrides_from_form(&bad_port).unwrap_err().0,
            override_idx::PORT
        );

        let mut quoted = override_form_from_host(0);
        quoted.form.fields[override_idx::USER].value = "a\"b".into();
        assert_eq!(
            overrides_from_form(&quoted).unwrap_err().0,
            override_idx::USER
        );
    }

    #[test]
    fn sort_mode_cycles_through_all_and_wraps() {
        let mut m = SortMode::default();
        assert_eq!(m, SortMode::Config);
        m = m.next();
        assert_eq!(m, SortMode::Recent);
        m = m.next();
        assert_eq!(m, SortMode::Name);
        m = m.next();
        assert_eq!(m, SortMode::Status);
        m = m.next();
        assert_eq!(m, SortMode::Config); // wraps
    }

    fn key(recency: Option<u64>, alias_lc: &str, rank: u8) -> SortKey {
        SortKey {
            recency,
            alias_lc: alias_lc.to_string(),
            rank,
        }
    }

    #[test]
    fn order_by_recent_newest_first_never_connected_last() {
        // idx0 never, idx1 @100, idx2 @300, idx3 never.
        let keys = [
            key(None, "a", 0),
            key(Some(100), "b", 0),
            key(Some(300), "c", 0),
            key(None, "d", 0),
        ];
        // Most-recent first (2 then 1), then never-connected in config order (0, 3).
        assert_eq!(order_by(SortMode::Recent, &keys), vec![2, 1, 0, 3]);
    }

    #[test]
    fn order_by_recent_equal_timestamps_break_on_config_order() {
        let keys = [key(Some(50), "z", 0), key(Some(50), "a", 0)];
        assert_eq!(order_by(SortMode::Recent, &keys), vec![0, 1]);
    }

    #[test]
    fn order_by_name_is_case_insensitive_and_stable() {
        let keys = [
            key(None, "banana", 0),
            key(None, "apple", 0),
            key(None, "apple", 0), // duplicate alias -> config-order tie-break
        ];
        assert_eq!(order_by(SortMode::Name, &keys), vec![1, 2, 0]);
    }

    #[test]
    fn order_by_status_ranks_then_alias() {
        // ranks: up(0), down(4), up(0); aliases chosen so the up pair sorts by name.
        let keys = [
            key(None, "web", liveness_rank(Liveness::Up)),
            key(None, "db", liveness_rank(Liveness::Down)),
            key(None, "api", liveness_rank(Liveness::Up)),
        ];
        // Both up first (api < web by alias), then the down host.
        assert_eq!(order_by(SortMode::Status, &keys), vec![2, 0, 1]);
    }

    #[test]
    fn order_by_config_is_identity() {
        let keys = [key(Some(9), "z", 4), key(None, "a", 0)];
        assert_eq!(order_by(SortMode::Config, &keys), vec![0, 1]);
    }

    #[test]
    fn liveness_rank_orders_up_before_down() {
        assert!(liveness_rank(Liveness::Up) < liveness_rank(Liveness::Checking));
        assert!(liveness_rank(Liveness::Checking) < liveness_rank(Liveness::Unknown));
        assert!(liveness_rank(Liveness::Unknown) < liveness_rank(Liveness::Skipped));
        assert!(liveness_rank(Liveness::Skipped) < liveness_rank(Liveness::Down));
    }

    #[test]
    fn known_host_spec_matches_plain_and_bracketed() {
        let plain = HostSpec::Plain("web1.example.com".into());
        assert!(known_host_spec_matches(&plain, "web1.example.com"));
        assert!(known_host_spec_matches(&plain, "WEB1.EXAMPLE.COM")); // case-insensitive
        assert!(!known_host_spec_matches(&plain, "web2.example.com"));
        // Comma-separated list with a bracketed [host]:port token.
        let list = HostSpec::Plain("alias,[10.0.0.5]:2222".into());
        assert!(known_host_spec_matches(&list, "alias"));
        assert!(known_host_spec_matches(&list, "10.0.0.5"));
        assert!(!known_host_spec_matches(&list, "2222"));
        // Hashed specs never match by name (so they always render muted).
        assert!(!known_host_spec_matches(
            &HostSpec::Hashed("|1|abc=|def=".into()),
            "web1.example.com"
        ));
    }
}
