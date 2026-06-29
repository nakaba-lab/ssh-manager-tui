//! SFTP browsing support.
//!
//! Phase 3 foundation: a **pure** parser for the `ls -l`-style directory listing
//! that the OpenSSH `sftp` client prints. OpenSSH exposes no machine-readable
//! attribute mode, so a remote browser must screen-scrape the human listing —
//! this is best-effort by nature (locale, column widths, date format and symlink
//! suffixes vary), so the parser is deliberately defensive: anything it cannot
//! confidently parse is skipped rather than mis-rendered.
//!
//! This module has **zero ratatui dependency**. The parser is fully unit-tested
//! on every platform; the [`SftpSession`] worker drives a live `sftp` and so can
//! only be exercised against a real server (the batch-construction helpers it
//! uses are unit-tested in isolation).

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use crate::os::askpass::{Outcome, ResolvedIdentity, arm_connect};
use crate::os::binaries::tools;
use crate::os::vault::Secret;

/// The per-session arming recipe: the resolved identity to bind to, plus the
/// vault secrets to release. Each browse op mints its OWN listener from a clone of
/// this, so the per-connect single-shot is consumed exactly once per op.
///
/// **Bounded residency**: the secrets live for the whole browse session, cloned
/// per op. This is deliberate — avoiding a per-nav vault re-open — not a leak.
/// The bound is enforced by zeroize-on-drop (both `Secret` and the listener) and
/// by the two teardown paths (`idle_autolock` and the circuit-breaker calling
/// `disarm`) that drop the `SftpArm` promptly when the session should no longer
/// auto-fill.
///
/// `Secret` and `ResolvedIdentity` both derive `Clone`, so the derive suffices.
#[derive(Clone)]
pub struct SftpArm {
    pub identity: ResolvedIdentity,
    pub password: Option<Secret>,
    pub passphrase: Option<Secret>,
}

/// One entry in a remote directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    /// File name (the symlink *name*, never the target).
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    /// Size in bytes as reported by the listing (0 when unparseable).
    pub size: u64,
}

impl RemoteEntry {
    /// A synthetic "parent directory" entry for navigating up a level.
    pub fn parent() -> Self {
        RemoteEntry {
            name: "..".to_string(),
            is_dir: true,
            is_link: false,
            size: 0,
        }
    }
}

/// Parse a block of `sftp` `ls -l` output into entries, one per line.
///
/// Each data line looks like `ls -l`:
/// ```text
/// drwxr-xr-x    2 user  group     4096 Jan 15 10:30 subdir
/// -rw-r--r--    1 user  group     1234 Jan 15 10:30 a file.txt
/// lrwxrwxrwx    1 user  group       10 Jan 15 10:30 link -> /target
/// ```
/// The first eight whitespace-separated fields are fixed (mode, links, owner,
/// group, size, month, day, time/year); the **name** is the remainder of the
/// line, so names containing spaces survive. `total N` headers, blank lines, and
/// the `.`/`..` self entries are dropped, as is any line that doesn't have the
/// minimum field count (malformed-server resilience).
pub fn parse_ls_l(block: &str) -> Vec<RemoteEntry> {
    block.lines().filter_map(parse_ls_l_line).collect()
}

/// Parse a single listing line, or `None` for a header/blank/`.`/`..`/malformed
/// line. Split out so it is independently testable.
fn parse_ls_l_line(line: &str) -> Option<RemoteEntry> {
    let line = line.trim_end_matches(['\r', '\n']);
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with("total ") {
        return None;
    }

    // Walk the first eight whitespace-separated fields, remembering the byte
    // offset (within `line`) just past the eighth — everything after is the name.
    let mut fields: [&str; 8] = [""; 8];
    let name_start = {
        let mut rest = line;
        let mut consumed = 0usize; // bytes of `line` consumed so far
        for slot in fields.iter_mut() {
            let lead = rest.len() - rest.trim_start().len();
            consumed += lead;
            rest = &rest[lead..];
            if rest.is_empty() {
                return None; // fewer than 8 fields → not a listing line
            }
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            *slot = &rest[..end];
            consumed += end;
            rest = &rest[end..];
        }
        // The name is the remainder, with its leading whitespace trimmed.
        let lead = rest.len() - rest.trim_start().len();
        consumed + lead
    };

    let mode = fields[0];
    let first = mode.chars().next()?;
    let is_dir = first == 'd';
    let is_link = first == 'l';
    let size = fields[4].parse::<u64>().unwrap_or(0);

    let raw_name = line.get(name_start..)?.trim();
    if raw_name.is_empty() {
        return None;
    }
    // For a symlink, the name is the part before the ` -> target` suffix.
    let name = if is_link {
        raw_name.split(" -> ").next().unwrap_or(raw_name).trim()
    } else {
        raw_name
    };
    // Drop the self/parent entries; the UI synthesises its own ".." row.
    if name == "." || name == ".." || name.is_empty() {
        return None;
    }

    Some(RemoteEntry {
        name: name.to_string(),
        is_dir,
        is_link,
        size,
    })
}

// ---------------------------------------------------------------------------
// Live browse session (M3.2)
// ---------------------------------------------------------------------------
//
// Each remote operation runs as a short-lived `sftp -b` child on a worker
// thread, reporting back over an mpsc channel the UI drains per tick (mirroring
// `os/liveness.rs`). Short-lived processes give a clean EOF per op — no fragile
// sentinel framing of a persistent REPL. `-o BatchMode=yes` guarantees an op can
// never hang on an auth prompt: it succeeds via key/agent/ControlMaster or fails
// fast with a diagnosable error. On unix a per-session ControlMaster multiplexes
// the ops over one authenticated connection so only the first pays the handshake.

/// A remote operation to run against the session's host. Only directory listing
/// is wired today; mutating ops (mkdir/rename/remove) are added when a UI consumer
/// exists, to avoid carrying unreachable command-builders.
#[derive(Debug, Clone)]
pub enum SftpOp {
    /// List a directory. [`list_batch`] builds this as `cd <path>` then a bare
    /// `ls -la` (and `"."` resolves + lists the home dir via `pwd` + `ls -la`) — NOT
    /// `ls -la <path>`, whose entry names would each be path-prefixed.
    List(String),
}

/// The result of a completed background op.
#[derive(Debug, Clone)]
pub enum SftpEvent {
    /// A `List` op completed: the listed `path`, its parsed entries, and (for the
    /// initial `"."` listing) the resolved absolute working directory from `pwd`.
    Listing {
        path: String,
        cwd: Option<String>,
        entries: Vec<RemoteEntry>,
    },
    /// An op failed. `path` is the requested directory (so a stale failure from a
    /// superseded navigation can be ignored); `msg` is a short, user-facing reason.
    Failed {
        path: Option<String>,
        msg: String,
        /// `is_auth_failure` over the FULL stderr (not just the first line), so a
        /// leading server Banner can't mask the auth-failure verdict.
        auth_failure: bool,
        /// The per-op askpass listener actually served the stored password this op
        /// (`Outcome::Served`). Distinguishes a real password rejection from a
        /// transient `arm_connect` failure that degraded to an un-armed
        /// `BatchMode=yes` op.
        served: bool,
    },
}

/// The `sftp` batch script for listing `path`, or `None` if the path can't be
/// expressed safely (see [`sftp_quote`]). `"."` is the home-dir sentinel: `pwd`
/// resolves its absolute path (so later navigation builds absolute paths — each
/// op is a fresh connection at the home dir), then `ls -la` lists it.
fn list_batch(path: &str) -> Option<String> {
    if path == "." {
        return Some("pwd\nls -la\n".to_string());
    }
    // `cd <path>` then a bare `ls -la` — NOT `ls -la <path>`. sftp's `ls` prefixes
    // every entry with the path argument it was given (e.g. `/dir/.`, `/dir/..`,
    // `/dir/sub`), so `ls -la <path>` would make each entry name an absolute path:
    // `remote_join(cwd, name)` then doubles it (`/dir//dir/sub`) and the `.`/`..`
    // rows are no longer dropped (their names are `/dir/.`, not `.`). Changing the
    // session's cwd first and listing it with a bare `ls -la` yields simple names.
    //
    // INVARIANT: `cd` MUST stay UNPREFIXED. A `-`-prefixed command (`-cd <path>`)
    // tells `sftp -b` to ignore the error and continue, so a failed `cd` would fall
    // through to the bare `ls -la` and silently list the PREVIOUS cwd (the home dir)
    // as if it were `<path>`. Unprefixed, a failed `cd` aborts the batch → the op
    // exits non-zero → `run_op` reports `Failed` instead of a stale listing.
    Some(format!("cd {}\nls -la\n", sftp_quote(path)?))
}

/// A live browse session against one saved host. Holds the channel the UI drains
/// and (on unix) the ControlMaster socket torn down on drop. Cloning the host
/// alias + per-op args into each worker keeps the workers self-contained.
pub struct SftpSession {
    alias: String,
    /// Extra args shared by every op (the unix control options), in front of the
    /// per-op `BatchMode` pair and `-b <script> -- <alias>`.
    common_args: Vec<String>,
    /// When `Some`, ops run armed: `-o BatchMode=no` plus a fresh per-op askpass
    /// listener minted from this recipe (auto-fills the stored password/passphrase).
    /// **Fail-safe degrade**: if `arm_connect` fails for a given op, that op runs
    /// `-o BatchMode=yes` instead (keyed on `listener.is_some()` in `run_op`), so
    /// `BatchMode=no` is only ever emitted alongside a live askpass environment.
    arm: Option<SftpArm>,
    tx: Sender<SftpEvent>,
    rx: Receiver<SftpEvent>,
    handles: Vec<JoinHandle<()>>,
    #[cfg(unix)]
    control: Option<std::path::PathBuf>,
}

impl SftpSession {
    /// Open a session for `alias`. Cheap: no connection is made until the first
    /// op runs (which, on unix, also establishes the shared ControlMaster).
    pub fn open(alias: &str) -> Self {
        // A private ControlMaster socket under a per-process temp path (unix only;
        // OpenSSH for Windows has no ControlMaster). A random nonce keys it to THIS
        // session so a stale socket (a failed `-O exit`) can never be reused by a
        // later browse of a different host. If `temp_dir()` is pathologically deep
        // the socket path could exceed the AF_UNIX `sun_path` limit (~104 bytes) and
        // fail to bind, so fall back to no master (ops connect individually).
        #[cfg(unix)]
        let control = {
            let p = std::env::temp_dir().join(format!(
                "sshm-sftp-cm-{}-{}",
                std::process::id(),
                nonce()
            ));
            (p.as_os_str().len() < 100).then_some(p)
        };
        Self::with_control(
            alias,
            #[cfg(unix)]
            control,
        )
    }

    /// Shared constructor: build `common_args` from the optional ControlMaster path.
    fn with_control(alias: &str, #[cfg(unix)] control: Option<std::path::PathBuf>) -> Self {
        let (tx, rx) = mpsc::channel();
        // BatchMode is now supplied per-op by `run_op` (via `batchmode_args`) so an
        // armed op can flip it to `no`; `common_args` carries only the unix control
        // options (when present), which multiplex ops over one connection.
        #[cfg(unix)]
        let common_args = {
            let mut a: Vec<String> = Vec::new();
            if let Some(path) = &control {
                a.extend(control_options(path));
            }
            a
        };
        #[cfg(not(unix))]
        let common_args: Vec<String> = Vec::new();

        SftpSession {
            alias: alias.to_string(),
            common_args,
            arm: None,
            tx,
            rx,
            handles: Vec::new(),
            #[cfg(unix)]
            control,
        }
    }

    /// Test-only constructor that never sets up a ControlMaster, so the unix `Drop`
    /// performs no `ssh -O exit` spawn — keeping `apply_sftp_event` tests I/O-free.
    #[cfg(test)]
    pub fn open_no_master(alias: &str) -> Self {
        Self::with_control(
            alias,
            #[cfg(unix)]
            None,
        )
    }

    /// The ControlMaster options to reuse this session's shared connection for an
    /// out-of-band inline transfer (so a browse transfer authenticates at most
    /// once). Empty on Windows / where there is no master — the transfer then
    /// authenticates on its own.
    pub fn control_args(&self) -> Vec<String> {
        #[cfg(unix)]
        if let Some(path) = &self.control {
            return control_options(path);
        }
        Vec::new()
    }

    /// Arm this session: subsequent ops request a fresh per-op askpass listener
    /// and, when that listener is successfully started, run with `-o BatchMode=no`.
    /// If `arm_connect` fails for an individual op it degrades to `BatchMode=yes`
    /// (no password sent) — see the `arm` field doc for the full fail-safe story.
    pub fn arm(&mut self, arm: SftpArm) {
        self.arm = Some(arm);
    }

    /// Disarm (circuit-breaker / teardown): drops + zeroizes the held secrets
    /// and reverts subsequent ops to `BatchMode=yes`.
    pub fn disarm(&mut self) {
        self.arm = None;
    }

    pub fn is_armed(&self) -> bool {
        self.arm.is_some()
    }

    /// Whether any dispatched op worker is still running. The UI's per-tick watchdog
    /// uses this to detect a lost completion (a worker that finished without
    /// reporting an event), which would otherwise leave `remote_loading` stuck true
    /// and — for an armed session — freeze the remote pane behind the serialization
    /// guard.
    pub fn has_inflight(&self) -> bool {
        self.handles.iter().any(|h| !h.is_finished())
    }

    /// Number of dispatched op worker threads tracked so far — lets a test observe
    /// whether a dispatch actually happened (vs was dropped by the serialization
    /// guard) rather than only inspecting status strings.
    #[cfg(test)]
    pub fn pending_ops(&self) -> usize {
        self.handles.len()
    }

    /// Dispatch `op` to a worker thread; its [`SftpEvent`] arrives via [`drain`].
    pub fn request(&mut self, op: SftpOp) {
        let alias = self.alias.clone();
        let common = self.common_args.clone();
        let arm = self.arm.clone();
        let tx = self.tx.clone();
        let handle = std::thread::spawn(move || {
            let event = run_op(&alias, &common, arm, op);
            let _ = tx.send(event);
        });
        self.handles.push(handle);
    }

    /// Non-blocking drain of completed op events. Also reaps finished worker
    /// threads so `handles` can't grow unbounded over a long browse session.
    pub fn drain(&mut self) -> Vec<SftpEvent> {
        let mut out = Vec::new();
        // Both Empty and Disconnected stop the drain; the tx is held by `self`, so
        // Disconnected only occurs at teardown.
        while let Ok(e) = self.rx.try_recv() {
            out.push(e);
        }
        self.handles.retain(|h| !h.is_finished());
        out
    }
}

/// The `-o ControlMaster/ControlPath/ControlPersist` flags for `path` (unix).
#[cfg(unix)]
fn control_options(path: &std::path::Path) -> Vec<String> {
    vec![
        "-o".to_string(),
        "ControlMaster=auto".to_string(),
        "-o".to_string(),
        format!("ControlPath={}", path.display()),
        "-o".to_string(),
        "ControlPersist=30".to_string(),
    ]
}

// Only unix has a ControlMaster to tear down; on Windows the session needs no
// custom Drop (op threads detach, channels close). Gating the whole impl avoids
// an empty Drop body on Windows.
//
// NOTE: in-flight op threads are detached, not killed: each holds a blocking
// `Command::output()` on a short-lived `sftp -b`. On drop the ControlMaster `-O
// exit` severs the shared connection, so an outstanding op fails fast and its
// thread exits promptly. The teardown itself runs on a detached thread so the UI
// thread (which drops the session when the browser closes) never blocks on a
// slow/wedged `ssh -O exit`.
#[cfg(unix)]
impl Drop for SftpSession {
    fn drop(&mut self) {
        // Tear down the shared ControlMaster so no background connection lingers.
        // Run it off the UI thread: `Command::status()` blocks, and a wedged master
        // or unresponsive host could otherwise freeze the interface.
        let Some(path) = self.control.take() else {
            return;
        };
        let alias = self.alias.clone();
        std::thread::spawn(move || {
            let _ = std::process::Command::new(tools().ssh.as_path())
                .arg("-o")
                .arg(format!("ControlPath={}", path.display()))
                .arg("-O")
                .arg("exit")
                .arg("--")
                .arg(&alias)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        });
    }
}

/// The BatchMode option pair for an op. Armed ops pass `BatchMode=no` to override
/// the `-b`-implied `batchmode yes` (sftp injects it into the spawned ssh), which
/// is what re-enables the armed `SSH_ASKPASS` helper for the password prompt. Shared
/// with the inline-transfer path in `update.rs` so both keep identical semantics.
pub(crate) fn batchmode_args(armed: bool) -> [&'static str; 2] {
    if armed {
        ["-o", "BatchMode=no"]
    } else {
        ["-o", "BatchMode=yes"]
    }
}

/// Run one op to completion in a child `sftp -b` process and map it to an event.
fn run_op(alias: &str, common_args: &[String], arm: Option<SftpArm>, op: SftpOp) -> SftpEvent {
    let SftpOp::List(path) = op;
    let Some(batch) = list_batch(&path) else {
        return SftpEvent::Failed {
            path: Some(path),
            msg: "unsafe remote path (contains a quote or control character)".to_string(),
            auth_failure: false,
            served: false,
        };
    };
    let script = match stage_batch(&batch) {
        Ok(p) => p,
        Err(e) => {
            return SftpEvent::Failed {
                path: Some(path),
                msg: format!("could not stage command: {e}"),
                auth_failure: false,
                served: false,
            };
        }
    };
    let _cleanup = BatchCleanup(script.clone());

    // Arm a fresh per-op listener FIRST so the BatchMode choice can key on whether
    // the askpass env is actually present: arming sets the password single-shot
    // (consumed once per op == one ssh connect; the listener zeroizes on
    // stop_and_join). If arming FAILS we must NOT pass BatchMode=no with no env —
    // on Windows that invites a CONIN$ console prompt that corrupts the TUI — so we
    // degrade to a plain BatchMode=yes op.
    let listener_env = arm.and_then(|a| arm_connect(a.identity, a.password, a.passphrase).ok());
    let (listener, env) = match listener_env {
        Some((l, env)) => (Some(l), env),
        None => (None, Vec::new()),
    };
    let armed = listener.is_some();

    let mut args: Vec<String> = common_args.to_vec();
    args.extend(batchmode_args(armed).iter().map(|s| s.to_string()));
    // Read-only connection options (they do not affect auth) so a disappeared /
    // black-holed host fails fast instead of wedging the remote pane (and, on
    // Windows where SftpSession has no Drop, leaking the hung op's listener +
    // un-zeroized secret). Applied to ALL browse ops, armed and un-armed.
    args.extend(["-o".to_string(), "ConnectTimeout=8".to_string()]);
    args.extend(["-o".to_string(), "ServerAliveInterval=5".to_string()]);
    args.extend(["-o".to_string(), "ServerAliveCountMax=2".to_string()]);
    args.push("-b".to_string());
    args.push(script.display().to_string());
    args.push("--".to_string());
    args.push(alias.to_string());

    let output = std::process::Command::new(tools().sftp.as_path())
        .args(&args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .output();

    // Tear the per-op listener down (zeroize) regardless of outcome, capturing its
    // Outcome so we can tell a real password rejection (Served) from a transient
    // arm failure that ran un-armed.
    let outcome = listener.map(|l| l.stop_and_join());
    let served = matches!(outcome, Some(Outcome::Served { .. }));

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let cwd = if path == "." {
                parse_pwd(&stdout)
            } else {
                None
            };
            SftpEvent::Listing {
                path,
                cwd,
                entries: parse_ls_l(&stdout),
            }
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            SftpEvent::Failed {
                path: Some(path),
                msg: first_error_line(&o.stderr, &o.stdout),
                auth_failure: is_auth_failure(&stderr), // FULL stderr, not first line
                served,
            }
        }
        Err(e) => SftpEvent::Failed {
            path: Some(path),
            msg: e.to_string(),
            auth_failure: false,
            served, // false on a spawn error
        },
    }
}

/// True iff `stderr` reports an OpenSSH **authentication** failure (vs a
/// directory-ACL denial, a host-key/KEX failure, or an arbitrary banner). Scans
/// the FULL stderr so a leading server banner cannot mask a later auth-failure
/// line. The recognized forms are:
///
/// - `<user>@<host>: Permission denied (<method-list>).` — the exhaustion message
///   (`contains(": Permission denied (")` plus `ends_with(").")`).
/// - `Permission denied, please try again.` — the per-attempt password-reject
///   message, the FIRST stderr line of an armed wrong-password op (`starts_with`).
/// - `Authentication failed…` — either the bare line-leading form (`starts_with`) or
///   the `Received disconnect from <host> port <p>:2: Authentication failed.`
///   disconnect-wrapped form (some sshd / `NumberOfPasswordPrompts=1`), matched by
///   anchoring on a leading `Received disconnect`. Deliberately NOT a bare `contains`,
///   so an arbitrary banner/MOTD line that merely mentions the phrase can't match.
/// - `Too many authentication failures` (`contains`).
///
/// A bare directory `Permission denied` (no `(method)`) and host-key failures are
/// deliberately negatives.
///
/// **False-positive note**: the remaining shape-based clauses (`": Permission denied ("`
/// and `"Too many authentication failures"`) key on substring, not exact equality, so a
/// crafted server banner containing one could still produce a false positive. This is
/// fail-safe: a false positive only disarms the session (no password is re-sent); it
/// never causes the server to authenticate.
pub fn is_auth_failure(stderr: &str) -> bool {
    stderr.lines().map(str::trim).any(|line| {
        let denied_with_methods = line.contains(": Permission denied (") && line.ends_with(").");
        denied_with_methods
            // The askpass-served password was rejected by the server. This is the
            // FIRST stderr line of an armed wrong-password op (the `(method-list)`
            // exhaustion line comes last), so without this clause the circuit-breaker
            // would never trip on the real armed-wrong-password case (caught by E2E).
            || line.starts_with("Permission denied, please try again")
            // `Authentication failed`: the bare leading form OR the disconnect-wrapped
            // form (`Received disconnect ...: Authentication failed.`). Anchored so a
            // banner that merely contains the phrase does not match.
            || line.starts_with("Authentication failed")
            || (line.starts_with("Received disconnect") && line.contains("Authentication failed"))
            || line.contains("Too many authentication failures")
    })
}

/// The first non-empty stderr line (falling back to stdout, then a generic note)
/// — a compact, user-facing failure reason.
fn first_error_line(stderr: &[u8], stdout: &[u8]) -> String {
    for buf in [stderr, stdout] {
        if let Some(line) = String::from_utf8_lossy(buf)
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
        {
            return line.to_string();
        }
    }
    "sftp command failed".to_string()
}

/// Extract the absolute path from `sftp`'s `pwd` output line
/// (`Remote working directory: /home/user`).
fn parse_pwd(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|l| {
        l.trim()
            .strip_prefix("Remote working directory:")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    })
}

/// Join a child `name` onto an absolute remote directory `cwd` (POSIX `/`).
pub fn remote_join(cwd: &str, name: &str) -> String {
    if cwd.is_empty() || cwd == "/" {
        format!("/{name}")
    } else {
        format!("{}/{}", cwd.trim_end_matches('/'), name)
    }
}

/// The parent of an absolute remote directory `cwd` (POSIX). The root's parent
/// is the root itself.
pub fn remote_parent(cwd: &str) -> String {
    let trimmed = cwd.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => trimmed[..i].to_string(),
    }
}

/// True iff `name` is safe to use as a SINGLE local path component on THIS host —
/// not empty, not `.`/`..`, and free of any path separator, drive prefix, or UNC
/// root. Gates a server-controlled SFTP download filename before it is joined onto
/// a local directory, so a hostile/compromised server cannot escape the chosen
/// local folder (e.g. a remote entry named `..\..\Startup\evil.bat`, `C:\…`, or
/// `\\host\share\…`) — the scp/sftp client-side filename-injection class
/// (CVE-2019-6111). Host path semantics are deliberate: the value becomes a
/// host-local destination, so `\` and `C:` matter on Windows but are ordinary
/// filename bytes on unix. `Path::file_name` collapses any traversing/qualified
/// path to its last component, so requiring it to equal `name` admits only a bare
/// component.
pub fn is_safe_local_name(name: &str) -> bool {
    !name.is_empty() && std::path::Path::new(name).file_name() == Some(std::ffi::OsStr::new(name))
}

/// Quote a path for an `sftp` batch command, or `None` if it can't be expressed
/// safely. sftp's batch parser has no escape for a literal `"`, and a control
/// character (newline/CR/tab/…) would split or corrupt the single-line command —
/// reject both, fail-closed (mirrors the config writer's refusal stance). A value
/// containing whitespace is wrapped in double quotes; backslashes are left intact
/// so Windows local paths round-trip. This is the single quoting gate for every
/// path that reaches an `sftp` batch line (browse ops and inline transfers).
pub fn sftp_quote(p: &str) -> Option<String> {
    if p.contains('"') || p.chars().any(|c| c.is_control()) {
        return None;
    }
    // A leading '-' would be parsed as a flag by the in-batch `ls`/`get`/`put`
    // (which precede no `--` sentinel). `./` makes it an unambiguous relative path;
    // absolute paths (remote `/…`, or a local drive/`/`) never start with '-', so
    // this only affects an operator-typed relative path in the transfer form.
    let normalized = if p.starts_with('-') {
        format!("./{p}")
    } else {
        p.to_string()
    };
    if normalized.chars().any(char::is_whitespace) {
        Some(format!("\"{normalized}\""))
    } else {
        Some(normalized)
    }
}

/// A short random hex nonce for unique, unguessable temp paths (control socket,
/// batch files). On the vanishingly rare RNG failure it falls back to the pid plus
/// a process-global counter, so concurrent callers still get distinct names (the
/// counter is what keeps `stage_batch`'s `create_new` from colliding).
fn nonce() -> String {
    match crate::os::vault::random_bytes(8) {
        Ok(bytes) => bytes.iter().map(|b| format!("{b:02x}")).collect(),
        Err(_) => {
            use std::sync::atomic::{AtomicU64, Ordering};
            static SEQ: AtomicU64 = AtomicU64::new(0);
            format!(
                "{:x}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            )
        }
    }
}

/// A temp-file guard removing a staged batch script on drop.
struct BatchCleanup(std::path::PathBuf);

impl Drop for BatchCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Stage an `sftp -b` batch script in a fresh private temp file and return its
/// path. A random-nonce name plus `create_new` means a pre-planted symlink or
/// file can never redirect the write (it fails closed), and the `0o600` mode is
/// applied atomically at creation on unix (no chmod race).
pub fn stage_batch(contents: &str) -> std::io::Result<std::path::PathBuf> {
    use std::io::Write;
    let path = std::env::temp_dir().join(format!(
        "sshm-sftp-op-{}-{}.txt",
        std::process::id(),
        nonce()
    ));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(&path)?;
    file.write_all(contents.as_bytes())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batchmode_args_flip_on_arm() {
        assert_eq!(batchmode_args(false), ["-o", "BatchMode=yes"]);
        assert_eq!(batchmode_args(true), ["-o", "BatchMode=no"]);
    }

    #[test]
    fn list_batch_scripts() {
        // The "." sentinel resolves the home dir via pwd, then lists it.
        assert_eq!(list_batch("."), Some("pwd\nls -la\n".to_string()));
        // A concrete path is `cd`'d into, then listed with a bare `ls -la` (quoted
        // if it has whitespace) — NOT `ls -la <path>`, which would prefix every
        // entry name with the path. See the list_batch comment.
        assert_eq!(
            list_batch("/var/log"),
            Some("cd /var/log\nls -la\n".to_string())
        );
        assert_eq!(
            list_batch("/srv/new dir"),
            Some("cd \"/srv/new dir\"\nls -la\n".to_string())
        );
        // An unsafe path can't be expressed → no batch (the op fails fast).
        assert_eq!(list_batch("/bad\"quote"), None);
        assert_eq!(list_batch("/bad\nnewline"), None);

        // Batch-abort invariant: NO command line may be `-`-prefixed. `sftp -b` treats
        // a leading `-` as "ignore this command's error and continue", which would let
        // a failed `cd` fall through to the bare `ls -la` and list the stale previous
        // cwd. A leading-'-' PATH is normalised to `./-…` by sftp_quote, so the `cd`
        // line still starts with "cd", never "-". (review: cd-unprefixed safety)
        for p in [".", "/var/log", "/srv/new dir", "-rf", "/x/-y"] {
            let script = list_batch(p).unwrap();
            assert!(
                script.lines().all(|l| !l.starts_with('-')),
                "no batch command may be '-'-prefixed (would suppress the cd abort): {script:?}"
            );
        }
    }

    #[test]
    fn home_listing_pwd_preamble_is_not_surfaced_as_an_entry() {
        // The home branch runs `pwd\nls -la\n`, so the combined stdout fed to
        // parse_ls_l begins with sftp's `Remote working directory: <path>` line. It
        // must NOT become a bogus directory entry: that line has too few `ls -la`
        // columns (its "name" remainder is empty), so the parser drops it. (review:
        // INFO — guards the home-branch parse against the pwd preamble)
        let stdout = "\
Remote working directory: /home/deploy
total 12
drwxr-xr-x    4 deploy deploy 4096 Jan 15 10:30 .
drwxr-xr-x   20 root   root   4096 Jan 15 10:30 ..
-rw-r--r--    1 deploy deploy  220 Jan 15 10:30 .bashrc
drwxr-xr-x    2 deploy deploy 4096 Jan 15 10:30 projects
";
        let entries = parse_ls_l(stdout);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        // Only the two real files survive: no `Remote`/`working`/`directory:` row, and
        // `.`/`..`/`total` are dropped as before.
        assert_eq!(names, vec![".bashrc", "projects"]);
        assert!(
            !names
                .iter()
                .any(|n| n.contains("working") || n.contains("directory")),
            "the pwd preamble line must not leak as an entry"
        );
    }

    #[test]
    fn is_safe_local_name_accepts_plain_files_rejects_traversal() {
        // A plain filename is a safe single component on every host.
        assert!(is_safe_local_name("invoice.txt"));
        assert!(is_safe_local_name("a file (final).txt"));

        // Universal rejections (separator/`..`/`.`/empty/absolute) on every host —
        // a hostile SFTP server must not escape the chosen local directory (H1).
        assert!(!is_safe_local_name(""));
        assert!(!is_safe_local_name("."));
        assert!(!is_safe_local_name(".."));
        assert!(!is_safe_local_name("../evil"));
        assert!(!is_safe_local_name("a/b"));
        assert!(!is_safe_local_name("/etc/passwd"));

        // Windows-only separators / prefixes: backslash, drive, drive-relative, UNC.
        // These ARE legal filename bytes on unix, so the rejection is host-specific —
        // gate the assertions to avoid the cross-platform clippy/test trap.
        #[cfg(windows)]
        {
            assert!(!is_safe_local_name(r"..\evil"));
            assert!(!is_safe_local_name(r"a\b"));
            assert!(!is_safe_local_name(r"C:\Windows\evil"));
            assert!(!is_safe_local_name("C:evil"));
            assert!(!is_safe_local_name(r"\\attacker\share\evil"));
        }
        // On unix a backslash/colon is an ordinary filename char that stays in-cwd.
        #[cfg(unix)]
        {
            assert!(is_safe_local_name(r"a\b"));
            assert!(is_safe_local_name("weird:name"));
        }
    }

    #[test]
    fn sftp_quote_is_fail_closed() {
        assert_eq!(sftp_quote("/normal/path"), Some("/normal/path".to_string()));
        // Backslashes are preserved, not escaped (Windows local paths round-trip).
        assert_eq!(
            sftp_quote("C:\\id\\a.txt"),
            Some("C:\\id\\a.txt".to_string())
        );
        // Whitespace → wrapped in double quotes.
        assert_eq!(sftp_quote("/a b/c"), Some("\"/a b/c\"".to_string()));
        // A literal quote, newline, CR, or other control char is rejected.
        assert_eq!(sftp_quote("/bad\"q"), None);
        assert_eq!(sftp_quote("/bad\nq"), None);
        assert_eq!(sftp_quote("/bad\rq"), None);
        assert_eq!(sftp_quote("/bad\tq"), None);
        // A leading '-' is normalised to `./-…` so it can't be read as a flag;
        // a '-' elsewhere is untouched.
        assert_eq!(sftp_quote("-rf"), Some("./-rf".to_string()));
        assert_eq!(sftp_quote("-a b"), Some("\"./-a b\"".to_string()));
        assert_eq!(sftp_quote("/x/-y"), Some("/x/-y".to_string()));
    }

    #[test]
    fn parse_pwd_extracts_absolute_home() {
        let out =
            "Remote working directory: /home/alice\ntotal 4\n-rw-r--r-- 1 a a 1 Jan 1 00:00 f";
        assert_eq!(parse_pwd(out), Some("/home/alice".to_string()));
        assert_eq!(parse_pwd("no pwd here"), None);
    }

    #[test]
    fn remote_join_and_parent() {
        assert_eq!(remote_join("/home/me", "docs"), "/home/me/docs");
        assert_eq!(remote_join("/", "etc"), "/etc");
        assert_eq!(remote_join("", "etc"), "/etc");
        assert_eq!(remote_join("/home/me/", "docs"), "/home/me/docs");

        assert_eq!(remote_parent("/home/me/docs"), "/home/me");
        assert_eq!(remote_parent("/home"), "/");
        assert_eq!(remote_parent("/"), "/");
        assert_eq!(remote_parent("/home/me/"), "/home");
    }

    #[test]
    fn first_error_line_prefers_stderr() {
        assert_eq!(
            first_error_line(b"\n  Permission denied\nmore\n", b"out"),
            "Permission denied"
        );
        assert_eq!(first_error_line(b"", b"only stdout"), "only stdout");
        assert_eq!(first_error_line(b"", b""), "sftp command failed");
    }

    #[test]
    fn parses_file_dir_and_symlink() {
        let block = "\
drwxr-xr-x    2 user  group     4096 Jan 15 10:30 subdir
-rw-r--r--    1 user  group     1234 Jan 15 10:30 readme.md
lrwxrwxrwx    1 user  group       10 Jan 15 10:30 cur -> /opt/app/current";
        let entries = parse_ls_l(block);
        assert_eq!(entries.len(), 3);

        assert_eq!(entries[0].name, "subdir");
        assert!(entries[0].is_dir);
        assert!(!entries[0].is_link);

        assert_eq!(entries[1].name, "readme.md");
        assert!(!entries[1].is_dir);
        assert_eq!(entries[1].size, 1234);

        // The symlink keeps only the name, never the ` -> target` suffix.
        assert_eq!(entries[2].name, "cur");
        assert!(entries[2].is_link);
        assert!(!entries[2].is_dir);
    }

    #[test]
    fn preserves_names_with_spaces() {
        let block = "-rw-r--r--    1 user  group     12 Jan  1 09:00 my notes (final).txt";
        let entries = parse_ls_l(block);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "my notes (final).txt");
        assert_eq!(entries[0].size, 12);
    }

    #[test]
    fn drops_total_header_self_parent_and_blank_lines() {
        let block = "\
total 20

drwxr-xr-x    5 user  group     4096 Jan 15 10:30 .
drwxr-xr-x   12 user  group     4096 Jan 15 10:30 ..
-rw-r--r--    1 user  group        7 Jan 15 10:30 keep.txt
";
        let entries = parse_ls_l(block);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "keep.txt");
    }

    #[test]
    fn malformed_lines_are_skipped_not_panicked() {
        // Too few columns, a stray banner, and a line that is all whitespace.
        let block = "\
garbage
drwx

-rw-r--r--    1 user  group     99 Jan 15 10:30 ok.txt
Permission denied";
        let entries = parse_ls_l(block);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "ok.txt");
        assert_eq!(entries[0].size, 99);
    }

    #[test]
    fn year_in_time_column_and_crlf_endings() {
        // Old files show a year instead of HH:MM in the 8th field; CRLF line ends
        // (a Windows sftp client) must not leak a '\r' into the name.
        let block = "-rw-r--r--    1 user  group   500 Jan 15  2021 archive.tar\r\n";
        let entries = parse_ls_l(block);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "archive.tar");
        assert_eq!(entries[0].size, 500);
    }

    #[test]
    fn unparseable_size_defaults_to_zero() {
        let block = "-rw-r--r--    1 user  group        ? Jan 15 10:30 weird";
        let entries = parse_ls_l(block);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].size, 0);
        assert_eq!(entries[0].name, "weird");
    }

    #[test]
    fn special_file_types_are_treated_as_non_dir_non_link() {
        // A block device, a fifo, a socket — neither dir nor link, but listed.
        let block = "\
brw-rw----    1 root  disk    8, 0 Jan 15 10:30 sda
prw-r--r--    1 user  group      0 Jan 15 10:30 pipe
srwxr-xr-x    1 user  group      0 Jan 15 10:30 sock";
        let entries = parse_ls_l(block);
        // The block device's size column is `8,` which is unparseable → 0, name
        // still resolves (the device-major/minor splits into fields 5/6, shifting
        // the name; we accept best-effort here and just assert no panic + count).
        assert!(entries.iter().all(|e| !e.is_dir && !e.is_link));
        assert!(entries.iter().any(|e| e.name == "pipe"));
        assert!(entries.iter().any(|e| e.name == "sock"));
    }

    #[test]
    fn parent_helper_entry() {
        let p = RemoteEntry::parent();
        assert_eq!(p.name, "..");
        assert!(p.is_dir);
        assert!(!p.is_link);
    }

    #[test]
    fn is_auth_failure_classifies_auth_only() {
        // Positive: the parenthesized method-list form, and the explicit phrases.
        assert!(is_auth_failure(
            "user@host: Permission denied (publickey,password)."
        ));
        assert!(is_auth_failure("Authentication failed."));
        assert!(is_auth_failure(
            "Received disconnect: Too many authentication failures"
        ));
        // Positive: the askpass-served password was rejected — the armed
        // wrong-password op's FIRST stderr line, which apply_sftp_event classifies
        // (regression for an E2E-found gap where the breaker never tripped).
        assert!(is_auth_failure("Permission denied, please try again."));
        // Positive even when a banner precedes it (scan ALL lines).
        assert!(is_auth_failure(
            "WARNING: unauthorized use prohibited\nuser@host: Permission denied (password)."
        ));
        // Negative: a directory-ACL denial with NO (method-list).
        assert!(!is_auth_failure(
            "remote open(\"/root/x\"): Permission denied"
        ));
        // Negative: host-key / KEX phase (must route to the accept-key steer, not here).
        assert!(!is_auth_failure("Host key verification failed."));
        assert!(!is_auth_failure("Connection closed by 10.0.0.1 port 22"));
        // Negative: a server banner that merely contains the substring before success.
        assert!(!is_auth_failure(
            "Notice: 'Permission denied (' is logged for audits"
        ));
    }

    #[test]
    fn is_auth_failure_sees_past_a_leading_banner() {
        // The full-stderr classification defeats banner-masking: a leading server
        // Banner can't hide a LATER auth-failure line (the breaker is fed this, not
        // just first_error_line).
        assert!(is_auth_failure(
            "************ AUTHORIZED USE ONLY ************\nWelcome banner line 2\nuser@host: Permission denied (publickey,password)."
        ));
    }

    #[test]
    fn is_auth_failure_recognizes_disconnect_wrapped_auth() {
        // OpenSSH (and some non-OpenSSH sshd, or `NumberOfPasswordPrompts=1`) wrap
        // the auth verdict in a `Received disconnect ...:` prefix, so the
        // `Authentication failed` phrase is no longer at the START of the line. The
        // breaker must still trip when a served password was rejected — match the
        // phrase anywhere on the line, not only at its start. (review: breaker
        // lockout-safety false-negative)
        assert!(is_auth_failure(
            "Received disconnect from 10.0.0.1 port 22:2: Authentication failed."
        ));
        // Still seen behind a leading banner (full-stderr scan).
        assert!(is_auth_failure(
            "Welcome\nReceived disconnect from h port 22:2: Authentication failed."
        ));
        // A bare directory-ACL denial must STAY a negative (no auth phrase).
        assert!(!is_auth_failure(
            "Received message too long\nremote open(\"/root/x\"): Permission denied"
        ));
    }

    #[test]
    fn is_auth_failure_ignores_authentication_failed_inside_a_banner() {
        // The `Authentication failed` clause is anchored to the bare leading form or
        // the `Received disconnect ...:` disconnect-wrapped form. An arbitrary
        // banner/MOTD line that merely CONTAINS the phrase must NOT trip the breaker —
        // otherwise an auth-SUCCEEDED session whose later cd/ls fails would be wrongly
        // disarmed and the user told the stored secret was rejected. (review F4)
        assert!(!is_auth_failure(
            "*** NOTICE: repeated Authentication failed events are logged and audited ***"
        ));
        assert!(!is_auth_failure(
            "banner: see https://host/why-Authentication-failed for help"
        ));
        // ...but a genuine disconnect-wrapped auth line behind that banner still trips.
        assert!(is_auth_failure(
            "*** NOTICE: ... ***\nReceived disconnect from h port 22:2: Authentication failed."
        ));
    }
}
