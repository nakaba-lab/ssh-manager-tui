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
use crate::config::diff::DiffLine;
use crate::config::model::{HostView, parse_sshm_tags};
use crate::os::connect::{ConnectOverrides, Protocol};
use crate::os::history::History;
use crate::os::keys::KeyInfo;
use crate::os::known_hosts::{HostSpec, KnownHostEntry};
use crate::os::liveness::{Liveness, LivenessProbe, ProbeTarget};
use crate::os::resolve::{ResolvedConfig, has_match_exec};
use crate::os::sftp::{RemoteEntry, SftpEvent, SftpSession};
use crate::os::vault::{MatchedKinds, SecretKind, Vault, match_vault_kinds};
use crate::os::{self, agent, keys, known_hosts};

/// Ordered labels of the edit-form fields. Indices are referenced by name in
/// [`FormIdx`].
pub const FIELD_LABELS: [&str; 12] = [
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
    "Tags (comma-separated)",
    "Description",
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
    // #45: host metadata persisted as `# sshm:` comments (single-line fields,
    // appended after EXTRAS so the existing indices never shift).
    pub const TAGS: usize = 10;
    pub const DESCRIPTION: usize = 11;
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
    /// Before-save diff preview (issue #42): a modal overlaid on the Edit form
    /// showing exactly what the save will write to `~/.ssh/config` versus the
    /// current on-disk file. Esc returns to the form; Enter commits the pending
    /// edit ([`App::pending_save`]) and saves. The rendered diff and scroll
    /// position live on [`App::diff_preview`] / [`App::diff_scroll`].
    DiffPreview,
    KeyManager,
    KnownHosts,
    /// Effective-config inspector (#43): a full-screen, filterable/scrollable view
    /// of a host's `ssh -G` resolution. A base screen (like [`Screen::KnownHosts`]),
    /// NOT a modal overlay; all state lives on `App` (`inspect_*`). Esc → List. The
    /// `ssh -G` resolve runs once at open time (see `update::open_inspect`), never
    /// per-draw, and is refused for configs that could trigger a `Match exec`.
    Inspect,
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
    /// Inline SFTP transfer form: collects a direction + local/remote paths for a
    /// one-shot `sftp -b` transfer. The host index and field values live in
    /// [`App::sftp_form`]. Submitting suspends the TUI and runs the transfer
    /// inline, just like an inline connect.
    SftpTransfer,
    /// Dual-pane SFTP browser (local | remote). A full base screen, not an
    /// overlay; all state lives in [`App::sftp_browser`].
    SftpBrowser,
    /// Password vault: list of stored secrets (login passwords / passphrases).
    Vault,
    /// Master-password prompt modal — unlock an existing vault, or create one.
    VaultUnlock,
    /// Add / edit a vault entry. `editing = Some(idx)` edits in place.
    VaultEntry {
        editing: Option<usize>,
    },
    /// Bulk vault-passphrase update modal (Issue #47), offered after a successful
    /// `ssh-keygen -p` when stored `Passphrase` entries reference the changed key
    /// (they now hold the OLD passphrase). One typed passphrase is upserted into
    /// every affected entry. The affected hosts + typed secret live in
    /// [`App::passphrase_sync`].
    PassphraseSync,
    /// Change the master password, or upgrade the vault's KDF parameters (#44). A
    /// modal overlay over [`Screen::Vault`]; the mode and typed passwords live off
    /// the `Screen` enum on [`App::vault_rekey`] (a unit variant here), so no
    /// plaintext password is ever cloned or formatted through `Screen`'s derived
    /// `Debug`/`Clone`.
    VaultRekey,
    /// One-time **password** consent modal, shown before arming a stored password
    /// the first time the resolved `<user@host>` is targeted this session — from
    /// either the connect path or the SFTP browser launch (see `origin`). Carries
    /// **no secret** — only the resolved `target` (for display), the armed `kinds`,
    /// and the `origin` needed to resume. Enter confirms (arm the password); Esc/`n`
    /// declines (passphrase stays armed, password withheld). A consent/typo guard,
    /// not a redirect defense.
    PasswordConfirm {
        kinds: MatchedKinds,
        target: String,
        origin: PasswordConfirmOrigin,
    },
}

/// Where a [`Screen::PasswordConfirm`] modal was opened from — determines what its
/// Enter (confirm) / Esc (decline) resumes. Both paths gate a server-facing
/// password identically; the modal only differs in what it re-enters afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordConfirmOrigin {
    /// The connect path: resume `connect_by_alias` for `alias` with the cached
    /// `ssh -G` resolution (`rc`) so the Confirmed/Withheld re-entry need not re-run
    /// `ssh -G` (which would execute any `Match exec` predicate a second time), plus
    /// the `mode`/`protocol`/overrides needed to rebuild the same launch. `alias`
    /// lives here (not on the shared modal) because only this path resumes by alias —
    /// the browser resumes by host index. Boxed values keep the `Screen` enum small.
    Connect {
        alias: String,
        mode: ConnectMode,
        /// Which client (`ssh` / `sftp`) the resumed connect launches.
        protocol: Protocol,
        rc: Box<ResolvedConfig>,
        /// Ad-hoc overrides to re-apply on the resumed connect (empty for a plain
        /// saved-host connect).
        ov: Box<ConnectOverrides>,
    },
    /// The SFTP browser launch (`b`): open the browser for this `host` index. On
    /// confirm the consent is recorded first, so the browser's arm recipe then
    /// releases the password; on decline the browser opens un-armed.
    SftpBrowse { host: usize },
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
    /// Overwrite an existing SFTP transfer destination. The browser has validated the
    /// name and confirmed the destination exists; `y` re-runs the transfer.
    OverwriteTransfer {
        direction: SftpDirection,
        name: String,
    },
    Quit,
}

/// A validated config mutation held between the diff-preview and its commit
/// (issue #42). `save_form` builds one, previews it by applying it to a *clone*
/// of the config, and — on confirm — applies the very same value to the real
/// config before saving. Preview and commit therefore run the identical
/// operation via [`apply_pending`](crate::update::apply_pending), so what the
/// user reviewed is byte-for-byte what gets written. The `HostView` is boxed to
/// keep the enum (and `Screen`, which it never enters, plus `App`) compact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingSave {
    /// Apply an edited view back onto the existing host block at this item index.
    Apply { item: usize, view: Box<HostView> },
    /// Append a brand-new host block built from this view.
    Add { view: Box<HostView> },
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
    /// Passphrase mode for the new key (Issue #47).
    pub passphrase: keys::GenPassphrase,
    pub field: usize, // 0 = type, 1 = filename, 2 = comment, 3 = passphrase
}

impl Default for GenWizard {
    fn default() -> Self {
        Self {
            key_type: keys::KeyType::Ed25519,
            filename: "id_ed25519".to_string(),
            filename_cursor: "id_ed25519".len(),
            comment: String::new(),
            comment_cursor: 0,
            passphrase: keys::GenPassphrase::NoPassphrase,
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

/// Which rekey operation the [`Screen::VaultRekey`] modal is performing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RekeyMode {
    /// Change the master password: current + new + confirm fields.
    #[default]
    ChangePassword,
    /// Upgrade the vault's KDF parameters, keeping the same password: only the
    /// current-password field is shown (re-deriving the key needs the plaintext,
    /// which the unlocked vault does not retain).
    UpgradeKdf,
}

/// Master-password change / KDF-upgrade modal state. Held off the `Screen` enum
/// (which derives `Debug`/`Clone`) so the typed passwords are never cloned or
/// formatted through it; each field scrubs on drop and is redacted in `Debug`.
#[derive(Default, Clone)]
pub struct VaultRekey {
    pub mode: RekeyMode,
    pub current: String,
    pub new: String,
    pub confirm: String,
    /// Focused field: 0 = current, 1 = new, 2 = confirm. `UpgradeKdf` uses only 0.
    pub field: usize,
    pub cursor: usize,
}

impl VaultRekey {
    /// Focusable field count for the current mode (3 for a password change, 1 for
    /// a KDF-only upgrade).
    pub fn field_count(&self) -> usize {
        match self.mode {
            RekeyMode::ChangePassword => 3,
            RekeyMode::UpgradeKdf => 1,
        }
    }
}

impl Drop for VaultRekey {
    fn drop(&mut self) {
        self.current.zeroize();
        self.new.zeroize();
        self.confirm.zeroize();
    }
}

impl std::fmt::Debug for VaultRekey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultRekey")
            .field("mode", &self.mode)
            .field("current", &"***")
            .field("new", &"***")
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

/// Bulk vault-passphrase update form (modal over the key manager, Issue #47).
/// The typed secret is scrubbed on drop and redacted in `Debug`, mirroring
/// [`VaultEntryForm`].
#[derive(Default, Clone)]
pub struct PassphraseSyncForm {
    /// Host aliases whose vault `Passphrase` entries reference the changed key.
    pub hosts: Vec<String>,
    pub secret: String,
    pub cursor: usize,
}

impl Drop for PassphraseSyncForm {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

impl std::fmt::Debug for PassphraseSyncForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PassphraseSyncForm")
            .field("hosts", &self.hosts)
            .field("secret", &"***")
            .field("cursor", &self.cursor)
            .finish()
    }
}

/// Direction of an SFTP transfer relative to the remote host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SftpDirection {
    /// Download: remote → local (`get`).
    #[default]
    Get,
    /// Upload: local → remote (`put`).
    Put,
}

impl SftpDirection {
    pub fn label(self) -> &'static str {
        match self {
            SftpDirection::Get => "Get  (remote → local)",
            SftpDirection::Put => "Put  (local → remote)",
        }
    }
}

/// Session-only state for the inline SFTP transfer modal. Collects a direction
/// plus a local and a remote path, then runs a one-shot `sftp -b` transfer
/// inline (the TUI suspends, exactly like an inline connect). Holds no secret.
#[derive(Debug, Clone, Default)]
pub struct SftpForm {
    /// Index into [`App::hosts`] of the host being transferred to/from.
    pub host: usize,
    pub direction: SftpDirection,
    pub local: String,
    pub local_cursor: usize,
    pub remote: String,
    pub remote_cursor: usize,
    /// Focused field: 0 = direction, 1 = local path, 2 = remote path.
    pub field: usize,
}

/// Number of focusable fields in the SFTP transfer form.
pub const SFTP_FIELDS: usize = 3;

/// Which pane of the dual-pane SFTP browser has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SftpPane {
    Local,
    Remote,
}

/// One entry in the local pane of the SFTP browser.
#[derive(Debug, Clone)]
pub struct LocalEntry {
    pub name: String,
    pub is_dir: bool,
}

/// Read a local directory into sorted entries (directories first, then files,
/// each alphabetical, case-insensitive), prepending a `..` row when `path` has a
/// parent. Unreadable entries are silently skipped (best-effort, like the rest of
/// the browser). Never fails — an unreadable directory yields just the `..` row.
pub fn read_local_dir(path: &std::path::Path) -> Vec<LocalEntry> {
    let mut entries: Vec<LocalEntry> = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| {
            // `file_type` is free (from the dir read) for the common case; only a
            // symlink needs an extra `metadata` stat to follow it, so a
            // symlink-to-directory counts as a directory (Enter descends into it)
            // rather than being misrouted to a file transfer. A broken symlink fails
            // the stat and falls back to "not a directory".
            let ft = e.file_type().ok();
            let is_dir = match ft {
                Some(t) if t.is_symlink() => std::fs::metadata(e.path())
                    .map(|m| m.is_dir())
                    .unwrap_or(false),
                Some(t) => t.is_dir(),
                None => false,
            };
            LocalEntry {
                name: e.file_name().to_string_lossy().into_owned(),
                is_dir,
            }
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    if path.parent().is_some() {
        entries.insert(
            0,
            LocalEntry {
                name: "..".to_string(),
                is_dir: true,
            },
        );
    }
    entries
}

/// State for the dual-pane SFTP browser (local | remote). The browse session
/// runs each remote op as a short-lived background `sftp -b` child; results are
/// drained per tick by [`App::drain_sftp_browser`]. Held in `App::sftp_browser`
/// (`None` when not browsing) because it owns a non-clonable [`SftpSession`].
pub struct SftpBrowser {
    pub host: usize,
    pub focus: SftpPane,
    pub local_cwd: std::path::PathBuf,
    pub local_entries: Vec<LocalEntry>,
    pub local_sel: usize,
    /// Absolute remote directory; empty until the initial `pwd` resolves it.
    pub remote_cwd: String,
    pub remote_entries: Vec<RemoteEntry>,
    pub remote_sel: usize,
    /// True while a remote listing op is in flight (drives a spinner).
    pub remote_loading: bool,
    /// Last status / error line shown in the browser footer area.
    pub status: String,
    pub session: SftpSession,
}

/// Consecutive armed-op failures that trip the circuit-breaker's count-based backstop
/// (disarm regardless of how the server phrased the rejection). A recognized
/// `auth_failure` disarms on the first; this bounds the unrecognized case to 2 armed
/// server-facing attempts.
const MAX_ARMED_FAILURES: u32 = 2;

/// Apply one drained [`SftpEvent`] to the browser state. Pure (no I/O), so the
/// stale-listing / unresolved-home / stale-failure handling is unit-testable
/// without a live session.
pub fn apply_sftp_event(b: &mut SftpBrowser, event: SftpEvent) {
    match event {
        SftpEvent::Listing { path, cwd, entries } => {
            let is_initial = b.remote_cwd.is_empty() && path == ".";
            if is_initial {
                match cwd {
                    Some(home) => b.remote_cwd = home,
                    None => {
                        // Without the absolute home, navigation would build wrong
                        // paths from an empty cwd — refuse to apply and prompt retry.
                        b.remote_loading = false;
                        b.status = "could not resolve remote home — press r to retry".to_string();
                        return;
                    }
                }
            } else if path != b.remote_cwd {
                // A stale listing for a directory we've navigated away from.
                return;
            }
            let mut sorted = entries;
            sorted.sort_by(|a, z| {
                z.is_dir
                    .cmp(&a.is_dir)
                    .then_with(|| a.name.to_lowercase().cmp(&z.name.to_lowercase()))
            });
            let mut list = vec![RemoteEntry::parent()];
            list.extend(sorted);
            b.remote_entries = list;
            b.remote_sel = b.remote_sel.min(b.remote_entries.len().saturating_sub(1));
            b.remote_loading = false;
            b.status.clear();
            // A successful op resets the breaker's armed-failure streak.
            b.session.note_op_succeeded();
        }
        SftpEvent::Failed {
            path,
            msg,
            auth_failure,
            served,
        } => {
            let stale = matches!(&path, Some(p) if !b.remote_cwd.is_empty() && *p != b.remote_cwd);

            // Circuit-breaker accounting runs even for a superseded (stale) op — it
            // still consumed a server-facing attempt, so it must count toward the
            // lockout bound. Disarm on a RECOGNIZED auth failure (immediately) OR after
            // MAX consecutive armed failures (a classification-INDEPENDENT backstop, so
            // a rejection phrasing `is_auth_failure` misses — a server that drops the
            // connection, a non-OpenSSH banner — can't keep re-sending the password
            // without bound). The streak resets on any success, so a benign command
            // error after a successful auth does not, on its own, kill auto-fill.
            let mut disarmed_now = false;
            if b.session.is_armed() {
                let streak = b.session.note_op_failed();
                if auth_failure || streak >= MAX_ARMED_FAILURES {
                    b.session.disarm();
                    disarmed_now = true;
                }
            }

            // A stale failure is accounted above but never alters the DISPLAY (its dir
            // is no longer shown); the current op's result is still pending.
            if stale {
                return;
            }
            b.remote_loading = false;
            if disarmed_now {
                b.status = if served {
                    // A stored secret was released and the server still rejected it.
                    "stored secret rejected — re-check the vault, or press F".to_string()
                } else {
                    // Disarmed without a released secret (arm failed, an unrecognized
                    // rejection, or the count backstop). Cause-neutral.
                    "auto-fill did not complete — press F to enter it, or retry".to_string()
                };
            } else {
                // Not disarmed: either un-armed, or armed with the first unrecognized
                // failure (still under the backstop) / a benign post-auth error — show
                // the actual error and stay as-is.
                b.status = friendly_sftp_error(&msg, auth_failure);
            }
            // `navigate_remote` cleared the old entries before this (failed) listing,
            // so reseed the synthetic `..` row — otherwise the pane is empty and a
            // user can't select a row to go back up (Backspace still works too).
            if b.remote_entries.is_empty() {
                b.remote_entries = vec![RemoteEntry::parent()];
                b.remote_sel = 0;
            }
        }
    }
}

/// Per-tick watchdog: if the remote pane is marked loading but no worker op is
/// still in flight, treat the in-flight listing as lost and clear the stuck flag, so
/// an armed session's serialization guard does not drop every later listing and
/// freeze the pane; the user can retry with `r`. Returns true if it fired.
///
/// The lost-event case it primarily guards (a worker that finished without reporting)
/// has no present trigger — `run_op` returns an event on every normal path. There is,
/// however, one BENIGN false-positive: `SftpSession::drain` empties the channel and
/// only then reaps finished handles, so if a worker `tx.send`s its event in the sliver
/// between the drain's last `try_recv` and its handle-reap, that drain collects no
/// event yet drops the now-finished handle — this watchdog then fires spuriously. It
/// is self-correcting: the still-queued event applies on the very next tick (it keys on
/// `remote_cwd`/`path`, not `remote_loading`), clearing the status — worst case a
/// one-frame flicker. (review: INFO)
pub(crate) fn sftp_loading_watchdog(b: &mut SftpBrowser) -> bool {
    if b.remote_loading && !b.session.has_inflight() {
        b.remote_loading = false;
        b.status = "listing did not complete — press r to retry".to_string();
        // Reseed the synthetic `..` row if the pane was cleared, so a user can still
        // navigate up (mirrors the failed-listing recovery in `apply_sftp_event`).
        if b.remote_entries.is_empty() {
            b.remote_entries = vec![RemoteEntry::parent()];
            b.remote_sel = 0;
        }
        true
    } else {
        false
    }
}

/// Map a raw sftp error to a more actionable message where we can recognise it.
/// `auth_failure` is precomputed by `run_op` over the FULL stderr; the host-key
/// arm stays first and keys on `msg`.
fn friendly_sftp_error(msg: &str, auth_failure: bool) -> String {
    if msg.contains("Host key verification failed") {
        "host key not trusted — connect once (Enter/F) to accept it, then browse".to_string()
    } else if auth_failure {
        "auth failed — press F to open an SFTP session (stored password auto-fills)".to_string()
    } else {
        msg.to_string()
    }
}

/// Source of a host in the flattened [`App::hosts`] list. A `Main` host lives in
/// the editable main config (indexed into `config.items`); an `Included` host is
/// a **read-only** projection from an `Include`d file (indexed into
/// [`App::included`]). Only `Main` reaches the surgical writer, so the type makes
/// an included host structurally uneditable (#52).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostRef {
    Main(usize),
    Included(usize),
}

pub struct App {
    pub should_quit: bool,
    pub screen: Screen,
    pub prev_screen: Option<Screen>,

    // --- config domain ---
    pub config: SshConfig,
    pub hosts: Vec<HostView>,
    /// Source (main-config item index, or read-only include) for each `hosts`
    /// entry. `hosts` lists main hosts first, then read-only included hosts.
    pub host_items: Vec<HostRef>,
    /// Read-only hosts expanded from `Include`d files (#52), forming the tail of
    /// `hosts`. Rebuilt by [`App::rebuild_hosts`]; never written back.
    pub included: Vec<crate::config::includes::IncludedHost>,
    /// The default `~/.ssh/config` — the file `ssh -G` reads no matter what
    /// `--config` loaded — scanned as a second root by [`App::ssh_g_exec_risk`]
    /// (#65). `None` when there is no home directory, and in tests, which must not
    /// depend on the developer's real config.
    pub default_config_root: Option<std::path::PathBuf>,

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

    // --- before-save diff preview (issue #42) ---
    /// The mutation awaiting confirmation in the diff preview, or `None` when no
    /// preview is open. Set by `save_form`, consumed on commit.
    pub pending_save: Option<PendingSave>,
    /// The rendered diff shown by the preview modal (old on-disk file → what the
    /// save will write). Computed once when the preview opens; the UI only paints.
    pub diff_preview: Vec<DiffLine>,
    /// Vertical scroll offset (in lines) of the diff preview.
    pub diff_scroll: u16,

    // --- connect-time override form ---
    pub override_form: OverrideForm,

    // --- inline SFTP transfer form ---
    pub sftp_form: SftpForm,

    // --- dual-pane SFTP browser (None when not browsing) ---
    pub sftp_browser: Option<SftpBrowser>,

    // --- S3 keys ---
    pub keys: Vec<KeyInfo>,
    pub keys_state: ListState,
    pub key_host_ctx: Option<usize>,
    /// Last answer from the ssh-agent probe (#49). `Probing` until the first
    /// result lands; re-probed on entering the key manager and after load/unload.
    pub agent: agent::AgentSnapshot,
    /// In-flight agent probe, drained per tick. `None` when idle.
    pub agent_probe: Option<agent::AgentProbe>,
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

    // --- #43 effective-config inspector (see Screen::Inspect) ---
    /// Alias whose `ssh -G` resolution is shown (for the breadcrumb).
    pub inspect_alias: String,
    /// The resolved `ssh -G` key/value pairs, in emission order. Loaded once at
    /// open time (never recomputed per-draw).
    pub inspect_rows: Vec<(String, String)>,
    pub inspect_state: ListState,
    pub inspect_search: String,
    pub inspect_searching: bool,

    // --- O3 action menu ---
    pub menu_sel: usize,

    // --- vault (password manager) ---
    /// The unlocked vault, held in memory for the session (`None` when locked).
    pub vault: Option<Vault>,
    pub vault_state: ListState,
    pub vault_unlock: VaultUnlock,
    /// Bulk vault-passphrase update modal state (Issue #47).
    pub passphrase_sync: PassphraseSyncForm,
    /// Set when a passphrase change wants to sync the vault but it is locked:
    /// the changed key's path, resumed by `submit_vault_unlock` after a
    /// successful unlock (Esc on the unlock prompt clears it = skip).
    pub passphrase_sync_pending: Option<std::path::PathBuf>,
    pub vault_rekey: VaultRekey,
    pub vault_entry: VaultEntryForm,
    /// When true, secrets are shown in the clear instead of masked.
    pub vault_reveal: bool,
    /// Whether a vault file exists on disk. Lets the breadcrumb distinguish
    /// "locked" (a vault exists but `vault` is `None`) from "no vault yet" without
    /// an fs stat per render frame. Seeded at startup; set true when one is created.
    pub has_vault_file: bool,
    /// Opt-in for connect-time **password** auto-fill (off by default: the password
    /// method is server-facing and can burn an auth attempt under `force`).
    /// Passphrase auto-fill is unaffected. **Persisted** across restart via
    /// [`os::prefs`] (seeded at startup, saved on toggle) so a user who opted in
    /// stays in; the per-target consent set is NOT persisted. Toggled with `p` on
    /// the vault screen; read by connect dispatch + the indicator (via
    /// [`App::vault_secret_kinds`]).
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
    /// Whether the one-time-per-session "auto-fill off: untrusted ssh client
    /// (`[PATH ssh]`)" nudge has fired. Set when a candidate connect is withheld
    /// because the resolved client isn't System32 OpenSSH; keeps that toast from
    /// repeating every connect (the breadcrumb `[PATH ssh]` warning is persistent).
    pub untrusted_client_hint_shown: bool,
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
            included: Vec::new(),
            default_config_root: crate::config::default_config_path().ok(),
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
            pending_save: None,
            diff_preview: Vec::new(),
            diff_scroll: 0,
            override_form: OverrideForm::default(),
            sftp_form: SftpForm::default(),
            sftp_browser: None,
            keys,
            keys_state: ListState::default(),
            key_host_ctx: None,
            agent: agent::AgentSnapshot::default(),
            agent_probe: None,
            gen_wizard: GenWizard::default(),
            pick_key_state: ListState::default(),
            pick_jump_state: ListState::default(),
            known_hosts: known,
            kh_state: ListState::default(),
            kh_search: String::new(),
            kh_searching: false,
            inspect_alias: String::new(),
            inspect_rows: Vec::new(),
            inspect_state: ListState::default(),
            inspect_search: String::new(),
            inspect_searching: false,
            menu_sel: 0,
            vault: None,
            vault_state: ListState::default(),
            vault_unlock: VaultUnlock::default(),
            passphrase_sync: PassphraseSyncForm::default(),
            passphrase_sync_pending: None,
            vault_rekey: VaultRekey::default(),
            vault_entry: VaultEntryForm::default(),
            vault_reveal: false,
            has_vault_file: crate::os::vault::default_path()
                .map(|p| p.exists())
                .unwrap_or(false),
            // Seed the opt-in from the persisted preference (best-effort: a missing
            // or corrupt prefs file falls back to the safe OFF default).
            password_autofill_enabled: crate::os::prefs::Prefs::load().password_autofill_enabled,
            confirmed_password_targets: HashSet::new(),
            password_hint_shown: false,
            untrusted_client_hint_shown: false,
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
        // Read-only expansion of `Include`d files (#52): re-expand so the list
        // reflects the current main file. Main hosts come first (editable), then
        // the read-only included hosts as the tail of `hosts`.
        self.included = self.expand_includes().hosts;

        let main_views = self.config.host_views();
        let total = main_views.len() + self.included.len();
        let mut hosts = Vec::with_capacity(total);
        let mut host_items = Vec::with_capacity(total);
        for (item_index, view) in main_views {
            host_items.push(HostRef::Main(item_index));
            hosts.push(view);
        }
        for (k, inc) in self.included.iter().enumerate() {
            host_items.push(HostRef::Included(k));
            hosts.push(inc.view.clone());
        }
        self.hosts = hosts;
        self.host_items = host_items;

        self.liveness.clear();
        self.rtt.clear();
        // A host edit can change what a confirmed target resolves to, so the
        // session-scoped password-confirm memory must not outlive a rebuild.
        self.confirmed_password_targets.clear();
        self.refilter();
        self.clamp_selection();
    }

    /// Expand the main config's `Include` directives (read-only, #52). Relative
    /// includes resolve against the main config's directory (OpenSSH's `~/.ssh`);
    /// `home` only drives tilde expansion, so a missing home must not skip
    /// expansion of relative/absolute includes.
    fn expand_includes(&self) -> crate::config::includes::Expansion {
        self.expand_includes_of(&self.config)
    }

    /// [`App::expand_includes`] for an arbitrary document — used by the safety
    /// gate, which expands each config as it exists **on disk** (what `ssh -G`
    /// reads) rather than the in-memory projection. Relative includes resolve
    /// against that document's own directory.
    fn expand_includes_of(&self, cfg: &SshConfig) -> crate::config::includes::Expansion {
        let home = dirs::home_dir().unwrap_or_default();
        let base_dir = cfg
            .path
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| home.join(".ssh"));
        crate::config::includes::expand(cfg, &base_dir, &home)
    }

    /// Why running `ssh -G` on this config could **execute** a `Match exec`
    /// predicate, or `None` when it is safe to run (#65). The single gate shared by
    /// every `ssh -G` caller — connect-time autofill, the SFTP arm, and the
    /// effective-config inspector (#43) — so those paths can never disagree about
    /// what counts as risky.
    ///
    /// Risky means: the main file carries a `Match exec`; **or** any `Include`d
    /// file `ssh -G` would read carries one (#52); **or** the config uses an
    /// include form we cannot follow (block-nested / quote-spliced / past
    /// `MAX_DEPTH`), which **fails safe** — un-scannable is treated as unsafe
    /// rather than assumed clean. A plain, scannable `Include` is NOT a risk by
    /// itself, so the common `config.d/*` setup keeps autofill and the inspector.
    ///
    /// Everything is re-read at call time — each root config **and** its includes —
    /// so an externally edited config is re-scanned before each use (all three
    /// callers are off the hot path). What `ssh -G` reads is the file **on disk**,
    /// so that is what gets expanded; the in-memory render is scanned too, purely
    /// as a belt-and-braces over-block for unsaved edits.
    ///
    /// Two roots are scanned: the document sshm loaded **and** the default
    /// `~/.ssh/config` when `--config <path>` made those differ — production never
    /// passes `-F`, so `ssh -G` reads the default one regardless of what sshm
    /// loaded. The system-wide config (`/etc/ssh/ssh_config`) is deliberately out
    /// of scope: it is root-owned, outside this threat model.
    ///
    /// Residual race: a config can be rewritten between this scan and `ssh -G`
    /// starting. That is inherent to any out-of-process gate; this only narrows it.
    pub fn ssh_g_exec_risk(&self) -> Option<&'static str> {
        use crate::config::includes::{ReadOutcome, read_config_text};
        const MAIN: &str = "host config uses `Match exec` — ssh -G would run it; skipped";
        const DEFAULT_ROOT: &str = "~/.ssh/config uses `Match exec` — ssh -G would run it; skipped";
        const INCLUDED: &str = "an included file uses `Match exec` — ssh -G would run it; skipped";
        const UNVERIFIABLE: &str =
            "config uses an `Include` form we can't verify — skipped for safety";

        // Unsaved in-memory edits: an over-block only, since ssh -G reads the disk.
        if has_match_exec(&self.config.render()) {
            return Some(MAIN);
        }
        let mut roots = vec![self.config.path.clone()];
        if let Some(default) = &self.default_config_root
            && *default != self.config.path
        {
            roots.push(default.clone());
        }
        for (i, root) in roots.into_iter().enumerate() {
            // Name the right file when the verdict comes from the default config a
            // `--config` session never showed the user.
            let main_reason = if i == 0 { MAIN } else { DEFAULT_ROOT };
            let text = match read_config_text(&root) {
                ReadOutcome::Text(t) => t,
                ReadOutcome::Missing => continue, // ssh cannot read it either
                ReadOutcome::Unscannable => return Some(UNVERIFIABLE),
            };
            if has_match_exec(&text) {
                return Some(main_reason);
            }
            let expansion = self.expand_includes_of(&crate::config::parser::parse(root, &text));
            if expansion.texts.iter().any(|t| has_match_exec(t)) {
                return Some(INCLUDED);
            }
            if expansion.blind_spot {
                return Some(UNVERIFIABLE);
            }
        }
        None
    }

    /// Whether connect-time secret **autofill must be withheld** because `ssh -G`
    /// could execute a predicate (see [`App::ssh_g_exec_risk`]).
    pub fn autofill_config_unsafe(&self) -> bool {
        self.ssh_g_exec_risk().is_some()
    }

    /// Sticky toast shown when an `Include`d host's edit/delete is refused — it is
    /// read-only in sshm and its source file must be edited directly (#52).
    pub fn toast_included_readonly(&mut self) {
        self.toast(
            "included host is read-only — edit its source file directly",
            true,
        );
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
            // #45: tags fold into the fuzzy haystack (patterns/HostName/User/tags).
            let hay = h.search_haystack();
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
    /// (wildcard-free) aliases other than `self_alias` (the host the picker is
    /// for — pass `""` to exclude nothing). The caller supplies `self_alias`
    /// because the picker serves both the edit form and the override modal, which
    /// identify "self" differently (see [`App::pick_jump_self_alias`]).
    pub fn jump_candidates(&self, self_alias: &str) -> Vec<usize> {
        self.hosts
            .iter()
            .enumerate()
            .filter_map(|(i, h)| {
                let a = h.alias();
                (!a.is_empty() && !a.contains(['*', '?', '!']) && a != self_alias).then_some(i)
            })
            .collect()
    }

    /// The alias to exclude from the ProxyJump picker for a given origin: the
    /// edit form's Host field when editing, or the override modal's target host.
    /// Returns `""` (exclude nothing) when neither is resolvable (e.g. the add
    /// form before a Host is typed).
    pub fn pick_jump_self_alias(&self, origin: &PickOrigin) -> &str {
        match origin {
            PickOrigin::Edit { .. } => self
                .form
                .fields
                .get(form_idx::HOST)
                .and_then(|f| f.value.split_whitespace().next())
                .unwrap_or(""),
            PickOrigin::Override => self
                .hosts
                .get(self.override_form.host)
                .map(|h| h.alias())
                .unwrap_or(""),
        }
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

    /// Start an ssh-agent probe, unless one is already in flight.
    ///
    /// The guard is not an optimisation. Dropping an `AgentProbe` only detaches
    /// its thread — that thread stays blocked in `ssh-add -l` with no timeout.
    /// Without the guard, holding `r` down (terminal autorepeat) against a
    /// wedged agent — the very situation this panel exists to explain — would
    /// pin one thread and one `ssh-add` process per repeat for the rest of the
    /// session.
    pub fn refresh_agent(&mut self) {
        if self.agent_probe.is_some() {
            return;
        }
        self.agent.status = agent::AgentStatus::Probing;
        self.agent_probe = Some(agent::AgentProbe::spawn());
    }

    /// Drain the in-flight agent probe (#49). Called per tick, like
    /// [`drain_liveness`](Self::drain_liveness), so the UI thread never blocks
    /// on `ssh-add`. Returns true if the snapshot changed (so the caller
    /// redraws).
    pub fn drain_agent(&mut self) -> bool {
        let Some(probe) = &self.agent_probe else {
            return false;
        };
        let (snapshot, disconnected) = probe.drain();
        if disconnected {
            self.agent_probe = None;
        }
        match snapshot {
            Some(snapshot) if snapshot != self.agent => {
                self.agent = snapshot;
                true
            }
            // The probe closed its channel without ever sending (its thread
            // died). Fall back to Unavailable rather than leaving the panel on
            // "checking…" until the user happens to press `r`.
            None if disconnected && self.agent.status == agent::AgentStatus::Probing => {
                self.agent.status = agent::AgentStatus::Unavailable;
                true
            }
            _ => false,
        }
    }

    /// How the selected key stands relative to the agent (#49). The key's
    /// [`PairStatus`](crate::os::keys::PairStatus) is part of the decision: a
    /// mismatched pair means the fingerprint we hold is not the one the agent
    /// would report for this private key.
    pub fn key_agent_state(&self, key: &KeyInfo) -> agent::KeyAgentState {
        agent::key_state(&self.agent.status, &key.fingerprint, key.pair)
    }

    /// Drain completed SFTP browse-session ops into the browser state (no-op when
    /// not browsing). Called per tick, like [`drain_liveness`](Self::drain_liveness),
    /// so the UI thread never blocks on a remote op. Returns true if anything
    /// changed (so the caller redraws).
    pub fn drain_sftp_browser(&mut self) -> bool {
        let Some(b) = self.sftp_browser.as_mut() else {
            return false;
        };
        let events = b.session.drain();
        let mut changed = !events.is_empty();
        for event in events {
            apply_sftp_event(b, event);
        }
        // Recover a stuck `remote_loading` if a completion event was lost (no worker
        // still in flight) — otherwise an armed session would freeze the remote pane.
        if sftp_loading_watchdog(b) {
            changed = true;
        }
        changed
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

    /// Lock the vault: drop+zeroize the decrypted vault and EVERY secret derived from
    /// it that could outlive the lock — the session password-confirm consents, any
    /// typed-but-unsaved entry/unlock form secret, and an open SFTP browser's armed
    /// session (its `SftpArm` holds independent copies of the vault secrets). The
    /// single teardown both lock paths (manual `L` and idle auto-lock) route through,
    /// so they can never drift on what a lock must scrub. Does NOT change `screen` or
    /// toast — each caller owns its own UX. (review: lock-teardown parity)
    pub(crate) fn lock_vault(&mut self) {
        self.vault = None;
        self.vault_reveal = false;
        self.confirmed_password_targets.clear();
        // Scrub any typed-but-unsaved secret in the entry/unlock/rekey/sync forms
        // too (all Drop-zeroize when replaced), so a lock leaves nothing behind.
        self.vault_entry = VaultEntryForm::default();
        self.vault_unlock = VaultUnlock::default();
        self.vault_rekey = VaultRekey::default();
        self.passphrase_sync = PassphraseSyncForm::default();
        self.passphrase_sync_pending = None;
        // A locked vault must not keep auto-filling an already-open browser: disarm
        // its session (drops + zeroizes the held SftpArm secrets).
        if let Some(b) = self.sftp_browser.as_mut() {
            b.session.disarm();
        }
    }

    /// Drop (zeroize) the vault if it has been unlocked and idle past
    /// [`VAULT_IDLE_LOCK`]. Returns true if it locked this tick. (#14)
    fn idle_autolock(&mut self) -> bool {
        if self.vault.is_some() && self.last_activity.elapsed() >= VAULT_IDLE_LOCK {
            self.lock_vault();
            // Bounce off any vault screen to the safe list view, INCLUDING an open
            // password-consent modal (else it would hover over a now-locked vault; the
            // lock already cleared consent + disarmed, so the modal is moot). Clear
            // prev_screen too since PasswordConfirm is an overlay. `Screen::VaultUnlock`
            // is intentionally NOT matched: it is only reachable while the vault is
            // locked (see `open_vault`), so the `vault.is_some()` guard above already
            // excludes it — don't "fix" the apparent asymmetry.
            if matches!(
                self.screen,
                Screen::Vault
                    | Screen::VaultEntry { .. }
                    | Screen::VaultRekey
                    | Screen::PasswordConfirm { .. }
                    | Screen::PassphraseSync
            ) {
                self.screen = Screen::List;
                self.prev_screen = None;
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

    /// Indices into `inspect_rows` matching the current `inspect_search`
    /// (case-insensitive substring over the key OR the value). Identity when the
    /// search is empty. Mirrors [`kh_filtered`](Self::kh_filtered). (#43)
    pub fn inspect_filtered(&self) -> Vec<usize> {
        if self.inspect_search.is_empty() {
            return (0..self.inspect_rows.len()).collect();
        }
        let needle = self.inspect_search.to_lowercase();
        self.inspect_rows
            .iter()
            .enumerate()
            .filter(|(_, (key, val))| {
                key.to_lowercase().contains(&needle) || val.to_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn clamp_inspect_selection(&mut self) {
        let len = self.inspect_filtered().len();
        if len == 0 {
            self.inspect_state.select(None);
        } else {
            let sel = self.inspect_state.selected().unwrap_or(0);
            self.inspect_state.select(Some(sel.min(len - 1)));
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
    // #45: tags edited as a single comma-separated line; description as one line.
    set_single(&mut fields[form_idx::TAGS], &view.tags.join(", "));
    if let Some(v) = &view.description {
        set_single(&mut fields[form_idx::DESCRIPTION], v);
    }

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
        // #45: tags reuse the sshm:tags grammar (parse_sshm_tags); desc is one line.
        tags: parse_sshm_tags(&f[form_idx::TAGS].value),
        description: opt(&f[form_idx::DESCRIPTION].value),
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

    // Each extra option must be `Key Value`. A key with no value would emit a
    // bare `-o KEY=` that OpenSSH rejects (exit 255) — and the same flag fed to
    // `ssh -G` silently degrades auto-fill — so reject it in-form instead.
    let mut extra_options: Vec<(String, String)> = Vec::new();
    for row in rows(override_idx::EXTRAS) {
        let mut it = row.splitn(2, char::is_whitespace);
        let Some(k) = it.next().filter(|k| !k.is_empty()).map(str::to_string) else {
            continue;
        };
        let v = it.next().unwrap_or("").trim().to_string();
        if v.is_empty() {
            return Err((
                override_idx::EXTRAS,
                format!("extra option '{k}' needs a value (Key Value)"),
            ));
        }
        extra_options.push((k, v));
    }

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

    fn test_browser() -> SftpBrowser {
        SftpBrowser {
            host: 0,
            focus: SftpPane::Remote,
            local_cwd: std::path::PathBuf::from("/tmp"),
            local_entries: Vec::new(),
            local_sel: 0,
            remote_cwd: String::new(),
            remote_entries: Vec::new(),
            remote_sel: 0,
            remote_loading: true,
            status: String::new(),
            // No ControlMaster, so the session's unix Drop spawns no `ssh -O exit`
            // and these tests stay genuinely I/O-free.
            session: SftpSession::open_no_master("test-host"),
        }
    }

    fn rentry(name: &str, is_dir: bool) -> RemoteEntry {
        RemoteEntry {
            name: name.to_string(),
            is_dir,
            is_link: false,
            size: 0,
        }
    }

    #[test]
    fn apply_initial_listing_resolves_home_sorts_and_prepends_parent() {
        let mut b = test_browser();
        let entries = vec![rentry("b.txt", false), rentry("adir", true)];
        apply_sftp_event(
            &mut b,
            SftpEvent::Listing {
                path: ".".to_string(),
                cwd: Some("/home/me".to_string()),
                entries,
            },
        );
        assert_eq!(b.remote_cwd, "/home/me");
        assert!(!b.remote_loading);
        // ".." first, then directories before files.
        let names: Vec<&str> = b.remote_entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["..", "adir", "b.txt"]);
    }

    #[test]
    fn watchdog_clears_stuck_loading_when_no_op_is_in_flight() {
        // A lost completion event (e.g. a worker that panicked before reporting)
        // would leave `remote_loading` stuck true; for an ARMED session the
        // serialization guard then drops every later listing and freezes the pane.
        // The per-tick watchdog recovers it. (review: INFO — no present trigger)
        let mut b = test_browser(); // remote_loading: true, no op dispatched
        assert!(
            !b.session.has_inflight(),
            "precondition: no worker op is in flight"
        );
        assert!(
            sftp_loading_watchdog(&mut b),
            "watchdog must fire when loading is stuck with nothing in flight"
        );
        assert!(!b.remote_loading, "the stuck loading flag is cleared");
        assert!(b.status.contains("retry"), "status invites a retry");
        // A synthetic `..` row is reseeded so the user can still navigate up.
        assert_eq!(
            b.remote_entries.first().map(|e| e.name.as_str()),
            Some("..")
        );
        // Idempotent: a second pass (loading already false) does nothing.
        assert!(!sftp_loading_watchdog(&mut b));
    }

    #[test]
    fn lock_vault_disarms_an_open_browser_and_clears_secrets() {
        use crate::os::askpass::ResolvedIdentity;
        use crate::os::sftp::SftpArm;
        use crate::os::vault::Vault;

        let mut app = app_fixture("Host h\n  HostName h\n");
        // An unlocked vault, a session password-confirm consent, and an open browser
        // whose session is ARMED (holds independent copies of the vault secrets).
        app.vault = Some(Vault::create("pw").unwrap());
        app.confirmed_password_targets
            .insert("deploy@h".to_string());
        let mut b = test_browser();
        b.session.arm(SftpArm {
            identity: ResolvedIdentity {
                user: "deploy".into(),
                host: "h".into(),
                host_key_alias: None,
                identity_paths: Vec::new(),
            },
            password: None,
            passphrase: None,
        });
        assert!(b.session.is_armed(), "precondition: browser is armed");
        app.sftp_browser = Some(b);

        app.lock_vault();

        // Locking drops the vault and the consents AND disarms the open browser — the
        // shared teardown both the manual `L` and idle auto-lock route through, so the
        // armed browser can never keep auto-filling past a lock. (review: lock parity)
        assert!(app.vault.is_none(), "vault dropped on lock");
        assert!(
            app.confirmed_password_targets.is_empty(),
            "session consents forgotten on lock"
        );
        assert!(
            !app.sftp_browser.as_ref().unwrap().session.is_armed(),
            "an open browser must be disarmed on lock"
        );
    }

    #[test]
    fn apply_stale_listing_is_ignored() {
        let mut b = test_browser();
        b.remote_cwd = "/home/me".to_string();
        b.remote_entries = vec![RemoteEntry::parent()];
        apply_sftp_event(
            &mut b,
            SftpEvent::Listing {
                path: "/somewhere/else".to_string(),
                cwd: None,
                entries: vec![rentry("x", false)],
            },
        );
        assert_eq!(b.remote_entries.len(), 1); // unchanged
    }

    #[test]
    fn apply_initial_listing_without_home_sets_error_status() {
        let mut b = test_browser();
        apply_sftp_event(
            &mut b,
            SftpEvent::Listing {
                path: ".".to_string(),
                cwd: None,
                entries: vec![],
            },
        );
        assert!(b.remote_cwd.is_empty());
        assert!(!b.remote_loading);
        assert!(b.status.contains("could not resolve"));
    }

    #[test]
    fn apply_failed_is_gated_by_path_and_friendlier() {
        let mut b = test_browser();
        b.remote_cwd = "/now".to_string();
        // A failure for a directory we've navigated away from is ignored.
        apply_sftp_event(
            &mut b,
            SftpEvent::Failed {
                path: Some("/old".to_string()),
                msg: "boom".to_string(),
                auth_failure: false,
                served: false,
            },
        );
        assert!(b.status.is_empty());
        // A failure for the current directory is shown.
        apply_sftp_event(
            &mut b,
            SftpEvent::Failed {
                path: Some("/now".to_string()),
                msg: "boom".to_string(),
                auth_failure: false,
                served: false,
            },
        );
        assert_eq!(b.status, "boom");
        // A host-key error is mapped to an actionable hint.
        apply_sftp_event(
            &mut b,
            SftpEvent::Failed {
                path: Some("/now".to_string()),
                msg: "Host key verification failed.".to_string(),
                auth_failure: false,
                served: false,
            },
        );
        assert!(b.status.contains("connect once"));
    }

    #[test]
    fn apply_failed_reseeds_parent_row_when_pane_empty() {
        // navigate_remote clears entries before a listing; if that listing fails,
        // the pane must still show the `..` row so the user can go back up.
        let mut b = test_browser();
        b.remote_cwd = "/now".to_string();
        b.remote_entries.clear();
        apply_sftp_event(
            &mut b,
            SftpEvent::Failed {
                path: Some("/now".to_string()),
                msg: "Permission denied".to_string(),
                auth_failure: false,
                served: false,
            },
        );
        assert_eq!(b.remote_entries.len(), 1);
        assert_eq!(b.remote_entries[0].name, "..");
        assert!(!b.remote_loading);
        assert_eq!(b.status, "Permission denied");
    }

    #[test]
    fn apply_failed_on_unresolved_home_is_applied() {
        // The initial '.' failure (remote_cwd still empty) is NOT a stale failure
        // and must surface, not be skipped by the path gate.
        let mut b = test_browser();
        apply_sftp_event(
            &mut b,
            SftpEvent::Failed {
                path: Some(".".to_string()),
                msg: "Connection refused".to_string(),
                auth_failure: false,
                served: false,
            },
        );
        assert!(!b.remote_loading);
        assert_eq!(b.status, "Connection refused");
        assert_eq!(b.remote_entries[0].name, "..");
    }

    #[test]
    fn apply_sftp_event_trips_circuit_breaker_on_auth_failure() {
        use crate::os::askpass::ResolvedIdentity;
        use crate::os::sftp::{SftpArm, SftpEvent};
        let mut b = SftpBrowser {
            host: 0,
            focus: SftpPane::Remote,
            local_cwd: std::path::PathBuf::from("/"),
            local_entries: Vec::new(),
            local_sel: 0,
            remote_cwd: String::new(),
            remote_entries: Vec::new(),
            remote_sel: 0,
            remote_loading: true,
            status: String::new(),
            session: crate::os::sftp::SftpSession::open_no_master("h"),
        };
        b.session.arm(SftpArm {
            identity: ResolvedIdentity {
                user: "u".into(),
                host: "h".into(),
                host_key_alias: None,
                identity_paths: Vec::new(),
            },
            password: None,
            passphrase: None,
        });
        assert!(b.session.is_armed());
        apply_sftp_event(
            &mut b,
            SftpEvent::Failed {
                path: Some(".".into()), // initial listing
                msg: "u@h: Permission denied (publickey,password).".into(),
                auth_failure: true,
                served: true,
            },
        );
        // First auth failure on an armed session disarms it (no second bad attempt).
        assert!(!b.session.is_armed());
        // The disarm status names the rejected secret (kind-agnostic wording).
        assert!(b.status.contains("rejected"));
    }

    #[test]
    fn breaker_disarms_after_repeated_unrecognized_armed_failures() {
        // An auth-failure phrasing the is_auth_failure denylist does NOT recognize
        // (auth_failure=false — e.g. a server that drops the connection) must not keep
        // the session armed forever re-sending the password. A count-based backstop
        // disarms after MAX consecutive armed failures regardless of classification (B1).
        use crate::os::askpass::ResolvedIdentity;
        use crate::os::sftp::{SftpArm, SftpEvent};
        let mut b = SftpBrowser {
            host: 0,
            focus: SftpPane::Remote,
            local_cwd: std::path::PathBuf::from("/"),
            local_entries: Vec::new(),
            local_sel: 0,
            remote_cwd: String::new(),
            remote_entries: Vec::new(),
            remote_sel: 0,
            remote_loading: true,
            status: String::new(),
            session: crate::os::sftp::SftpSession::open_no_master("h"),
        };
        b.session.arm(SftpArm {
            identity: ResolvedIdentity {
                user: "u".into(),
                host: "h".into(),
                host_key_alias: None,
                identity_paths: Vec::new(),
            },
            password: None,
            passphrase: None,
        });
        let unrecognized = || SftpEvent::Failed {
            path: Some(".".into()),
            msg: "Connection reset by peer".into(),
            auth_failure: false, // not in the denylist
            served: false,
        };
        apply_sftp_event(&mut b, unrecognized());
        assert!(
            b.session.is_armed(),
            "still armed after the 1st unrecognized failure"
        );
        apply_sftp_event(&mut b, unrecognized());
        assert!(
            !b.session.is_armed(),
            "count backstop disarms after MAX consecutive armed failures"
        );
    }

    #[test]
    fn breaker_disarms_on_any_armed_auth_failure_message_varies_by_served() {
        use crate::os::askpass::ResolvedIdentity;
        use crate::os::sftp::{SftpArm, SftpEvent};

        let make_arm = || SftpArm {
            identity: ResolvedIdentity {
                user: "u".into(),
                host: "h".into(),
                host_key_alias: None,
                identity_paths: Vec::new(),
            },
            password: None,
            passphrase: None,
        };
        let mut b = SftpBrowser {
            host: 0,
            focus: SftpPane::Remote,
            local_cwd: std::path::PathBuf::from("/"),
            local_entries: Vec::new(),
            local_sel: 0,
            remote_cwd: String::new(),
            remote_entries: Vec::new(),
            remote_sel: 0,
            remote_loading: true,
            status: String::new(),
            session: crate::os::sftp::SftpSession::open_no_master("h"),
        };

        // served=true: the stored password was sent and rejected -> disarm.
        b.session.arm(make_arm());
        assert!(b.session.is_armed());
        apply_sftp_event(
            &mut b,
            SftpEvent::Failed {
                path: Some(".".into()),
                msg: "u@h: Permission denied (publickey,password).".into(),
                auth_failure: true,
                served: true,
            },
        );
        assert!(!b.session.is_armed());
        assert!(b.status.contains("rejected"));

        // served=false + auth_failure: armed but no secret was released this op
        // (arm_connect failed → ran un-armed, OR the server used a method we don't
        // auto-fill — keyboard-interactive / publickey-only). Still DISARM: keeping it
        // armed would re-run BatchMode=no on every retry and re-attempt interactive
        // auth, burning server-facing auth attempts with no lockout bound. Disarming
        // makes subsequent ops fail fast under BatchMode=yes (M1). Message stays
        // cause-neutral (no server-facing secret was sent).
        b.session.arm(make_arm());
        assert!(b.session.is_armed());
        apply_sftp_event(
            &mut b,
            SftpEvent::Failed {
                path: Some(".".into()),
                msg: "u@h: Permission denied (publickey,password).".into(),
                auth_failure: true,
                served: false,
            },
        );
        assert!(!b.session.is_armed());
        assert!(b.status.contains("did not complete"));
    }

    #[test]
    fn armed_session_stays_armed_on_benign_post_auth_error() {
        // The `&& auth_failure` guard exists so a benign command error AFTER a
        // successful auth (e.g. `cd` into a forbidden/missing dir reports served=true
        // but auth_failure=false) does NOT disarm — otherwise ordinary browsing would
        // kill auto-fill. Pins that guard: a regression dropping `&& auth_failure`
        // (disarming on served alone) would fail here while every auth-failure test
        // still passed (M7). (review: M7 over-disarm coverage)
        use crate::os::askpass::ResolvedIdentity;
        use crate::os::sftp::{SftpArm, SftpEvent};
        let mut b = SftpBrowser {
            host: 0,
            focus: SftpPane::Remote,
            local_cwd: std::path::PathBuf::from("/"),
            local_entries: Vec::new(),
            local_sel: 0,
            remote_cwd: "/home/me".to_string(),
            remote_entries: Vec::new(),
            remote_sel: 0,
            remote_loading: true,
            status: String::new(),
            session: crate::os::sftp::SftpSession::open_no_master("h"),
        };
        b.session.arm(SftpArm {
            identity: ResolvedIdentity {
                user: "u".into(),
                host: "h".into(),
                host_key_alias: None,
                identity_paths: Vec::new(),
            },
            password: None,
            passphrase: None,
        });
        assert!(b.session.is_armed());
        apply_sftp_event(
            &mut b,
            SftpEvent::Failed {
                path: Some("/home/me".into()),
                msg: "remote open(\"/home/me/secret\"): Permission denied".into(),
                auth_failure: false, // a benign post-auth ACL error, NOT an auth failure
                served: true,
            },
        );
        assert!(
            b.session.is_armed(),
            "a benign post-auth error (auth_failure=false) must NOT disarm auto-fill"
        );
        // It surfaces the friendly/raw error, not the 'rejected' disarm wording.
        assert!(!b.status.contains("rejected"));
    }

    #[test]
    fn read_local_dir_sorts_dirs_first_and_prepends_parent() {
        let base = std::env::temp_dir().join(format!("sshm-rld-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("zdir")).unwrap();
        std::fs::create_dir_all(base.join("Adir")).unwrap();
        std::fs::write(base.join("m.txt"), b"x").unwrap();
        let entries = read_local_dir(&base);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        // ".." first (base has a parent), then dirs case-insensitive alpha, then file.
        assert_eq!(names, vec!["..", "Adir", "zdir", "m.txt"]);
        assert!(entries[1].is_dir && entries[2].is_dir && !entries[3].is_dir);
        let _ = std::fs::remove_dir_all(&base);
    }

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
    fn override_form_rejects_value_less_extra() {
        // A key-only extra would emit `-o KEY=` which ssh rejects, so reject it.
        let mut of = override_form_from_host(0);
        of.form.fields[override_idx::EXTRAS].rows = vec!["ForwardAgent".into()];
        assert_eq!(
            overrides_from_form(&of).unwrap_err().0,
            override_idx::EXTRAS
        );
        // A complete `Key Value` row is accepted.
        of.form.fields[override_idx::EXTRAS].rows = vec!["ForwardAgent yes".into()];
        assert_eq!(
            overrides_from_form(&of).unwrap().extra_options,
            vec![("ForwardAgent".to_string(), "yes".to_string())]
        );
    }

    /// Build a real `App` over a throwaway config file so the few methods that
    /// need full state (host list, picker self-exclusion) can be table-tested.
    /// `App::new` reads the real `~/.ssh` (read-only) and may spawn `ssh-keygen`
    /// to fingerprint discovered keys, but the tests assert only on the scratch
    /// config, so the host environment can't affect results. The scratch dir is
    /// removed once `App` has loaded the config into memory (tests never re-read).
    fn app_fixture(config_body: &str) -> App {
        use std::io::Write;
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("sshm-app-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(config_body.as_bytes()).unwrap();
        drop(f);
        let app = App::new(path).expect("App::new over scratch config");
        let _ = std::fs::remove_dir_all(&dir);
        app
    }

    #[test]
    fn jump_candidates_excludes_override_target_not_stale_edit_form() {
        let mut app = app_fixture(
            "Host alpha\n  HostName a\n\nHost beta\n  HostName b\n\nHost gamma\n  HostName g\n",
        );
        assert_eq!(
            app.hosts.iter().map(|h| h.alias()).collect::<Vec<_>>(),
            vec!["alpha", "beta", "gamma"]
        );
        // Leave a stale alias in the edit form to prove the override picker does
        // NOT key off it (the bug this fixes).
        app.form = form_from_view(&app.hosts[0].clone()); // HOST = "alpha"

        // Override modal targeting beta (index 1).
        app.override_form = override_form_from_host(1);
        let origin = PickOrigin::Override;
        assert_eq!(app.pick_jump_self_alias(&origin), "beta");
        let cands: Vec<&str> = app
            .jump_candidates(app.pick_jump_self_alias(&origin))
            .iter()
            .map(|&i| app.hosts[i].alias())
            .collect();
        assert!(
            !cands.contains(&"beta"),
            "the override target must be excluded (no self-loop -J)"
        );
        assert!(
            cands.contains(&"alpha") && cands.contains(&"gamma"),
            "the stale edit-form alias must NOT be excluded from the override list"
        );
    }

    #[test]
    fn pick_jump_self_alias_uses_edit_host_field_for_edit_origin() {
        let mut app = app_fixture("Host one\n  HostName x\n\nHost two\n  HostName y\n");
        app.form = form_from_view(&app.hosts[1].clone()); // HOST = "two"
        assert_eq!(
            app.pick_jump_self_alias(&PickOrigin::Edit { editing: Some(0) }),
            "two"
        );
        // Add form (empty HOST) excludes nothing.
        app.form = EditForm::default();
        assert_eq!(
            app.pick_jump_self_alias(&PickOrigin::Edit { editing: None }),
            ""
        );
    }

    #[test]
    fn search_matches_host_by_tag() {
        // #45 AC7: typing a tag name in the `/` search matches the tagged host
        // via the existing fuzzy filter (tags folded into the haystack), even
        // though the tag appears in neither the alias nor the HostName.
        let mut app = app_fixture(
            "# sshm:tags prod,db\nHost web\n  HostName 1.1.1.1\n\nHost mail\n  HostName 2.2.2.2\n",
        );
        app.search = "prod".to_string();
        app.refilter();
        let matched: Vec<&str> = app.filtered.iter().map(|&i| app.hosts[i].alias()).collect();
        assert_eq!(matched, vec!["web"]);
    }

    #[test]
    fn inspect_filtered_matches_key_or_value_case_insensitively() {
        // #43: the effective-config inspector filters by a case-insensitive
        // substring over BOTH the key and the value, mirroring `kh_filtered`.
        let mut app = app_fixture("Host a\n  HostName 1.2.3.4\n");
        app.inspect_rows = vec![
            ("hostname".to_string(), "10.0.0.5".to_string()),
            ("user".to_string(), "deploy".to_string()),
            ("port".to_string(), "2222".to_string()),
        ];

        // Empty search = identity (every row shown).
        app.inspect_search.clear();
        assert_eq!(app.inspect_filtered(), vec![0, 1, 2]);

        // Match on the key (case-insensitive).
        app.inspect_search = "USER".to_string();
        assert_eq!(app.inspect_filtered(), vec![1]);

        // Match on the value.
        app.inspect_search = "10.0".to_string();
        assert_eq!(app.inspect_filtered(), vec![0]);

        // No match = empty.
        app.inspect_search = "zzz".to_string();
        assert!(app.inspect_filtered().is_empty());
    }

    #[test]
    fn clamp_inspect_selection_shrinks_to_last_row_or_none() {
        // #43: when the filter narrows below the current selection, the
        // selection must clamp to the new last index (or clear when empty).
        let mut app = app_fixture("Host a\n  HostName 1.2.3.4\n");
        app.inspect_rows = vec![
            ("hostname".to_string(), "10.0.0.5".to_string()),
            ("user".to_string(), "deploy".to_string()),
            ("port".to_string(), "2222".to_string()),
        ];

        // Select the last row, then narrow the filter to a single match: the
        // selection clamps to the new last index (0).
        app.inspect_state.select(Some(2));
        app.inspect_search = "user".to_string();
        app.clamp_inspect_selection();
        assert_eq!(app.inspect_state.selected(), Some(0));

        // A filter matching nothing clears the selection.
        app.inspect_search = "zzz".to_string();
        app.clamp_inspect_selection();
        assert_eq!(app.inspect_state.selected(), None);
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

    #[test]
    fn friendly_sftp_error_steers_on_auth_failure() {
        // `auth_failure` is precomputed by run_op; pass it through here.
        // Auth failure -> steer to F. Host-key failure keeps the accept-key steer.
        let s = friendly_sftp_error("u@h: Permission denied (publickey,password).", true);
        assert!(s.contains('F') && s.to_lowercase().contains("password"));
        let hk = friendly_sftp_error("Host key verification failed.", false);
        assert!(hk.contains("host key") && !hk.to_lowercase().contains("stored password"));
        // A directory-ACL denial passes through unchanged (not an auth failure).
        let acl = friendly_sftp_error("remote open(\"/root/x\"): Permission denied", false);
        assert_eq!(acl, "remote open(\"/root/x\"): Permission denied");
    }
}
