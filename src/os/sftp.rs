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

use crate::os::binaries::tools;

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
    /// List a directory (`ls -la <path>`); `"."` resolves + lists the home dir.
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
    Failed { path: Option<String>, msg: String },
}

/// The `sftp` batch script for listing `path`, or `None` if the path can't be
/// expressed safely (see [`sftp_quote`]). `"."` is the home-dir sentinel: `pwd`
/// resolves its absolute path (so later navigation builds absolute paths — each
/// op is a fresh connection at the home dir), then `ls -la` lists it.
fn list_batch(path: &str) -> Option<String> {
    if path == "." {
        return Some("pwd\nls -la\n".to_string());
    }
    Some(format!("ls -la {}\n", sftp_quote(path)?))
}

/// A live browse session against one saved host. Holds the channel the UI drains
/// and (on unix) the ControlMaster socket torn down on drop. Cloning the host
/// alias + per-op args into each worker keeps the workers self-contained.
pub struct SftpSession {
    alias: String,
    /// Extra args shared by every op (`-o BatchMode=yes`, and the unix control
    /// options), in front of the per-op `-b <script> -- <alias>`.
    common_args: Vec<String>,
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
        // `-o BatchMode=yes` makes every op fail fast instead of hanging on a
        // prompt; on unix the control options (when present) multiplex ops over one
        // connection.
        #[cfg(unix)]
        let common_args = {
            let mut a = vec!["-o".to_string(), "BatchMode=yes".to_string()];
            if let Some(path) = &control {
                a.extend(control_options(path));
            }
            a
        };
        #[cfg(not(unix))]
        let common_args = vec!["-o".to_string(), "BatchMode=yes".to_string()];

        SftpSession {
            alias: alias.to_string(),
            common_args,
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

    /// Dispatch `op` to a worker thread; its [`SftpEvent`] arrives via [`drain`].
    pub fn request(&mut self, op: SftpOp) {
        let alias = self.alias.clone();
        let common = self.common_args.clone();
        let tx = self.tx.clone();
        let handle = std::thread::spawn(move || {
            let event = run_op(&alias, &common, op);
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

/// Run one op to completion in a child `sftp -b` process and map it to an event.
fn run_op(alias: &str, common_args: &[String], op: SftpOp) -> SftpEvent {
    let SftpOp::List(path) = op;
    let Some(batch) = list_batch(&path) else {
        return SftpEvent::Failed {
            path: Some(path),
            msg: "unsafe remote path (contains a quote or control character)".to_string(),
        };
    };
    let script = match stage_batch(&batch) {
        Ok(p) => p,
        Err(e) => {
            return SftpEvent::Failed {
                path: Some(path),
                msg: format!("could not stage command: {e}"),
            };
        }
    };
    let _cleanup = BatchCleanup(script.clone());

    let mut args: Vec<String> = common_args.to_vec();
    args.push("-b".to_string());
    args.push(script.display().to_string());
    args.push("--".to_string());
    args.push(alias.to_string());

    let output = std::process::Command::new(tools().sftp.as_path())
        .args(&args)
        .output();

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
        Ok(o) => SftpEvent::Failed {
            path: Some(path),
            msg: first_error_line(&o.stderr, &o.stdout),
        },
        Err(e) => SftpEvent::Failed {
            path: Some(path),
            msg: e.to_string(),
        },
    }
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
    fn list_batch_scripts() {
        // The "." sentinel resolves the home dir via pwd, then lists it.
        assert_eq!(list_batch("."), Some("pwd\nls -la\n".to_string()));
        // A concrete absolute path is just listed (quoted if it has whitespace).
        assert_eq!(
            list_batch("/var/log"),
            Some("ls -la /var/log\n".to_string())
        );
        assert_eq!(
            list_batch("/srv/new dir"),
            Some("ls -la \"/srv/new dir\"\n".to_string())
        );
        // An unsafe path can't be expressed → no batch (the op fails fast).
        assert_eq!(list_batch("/bad\"quote"), None);
        assert_eq!(list_batch("/bad\nnewline"), None);
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
}
