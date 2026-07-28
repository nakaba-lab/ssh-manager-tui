//! ssh-agent integration (#49): what the agent holds, and loading/unloading a
//! key into it. See CLAUDE.md layering — no ratatui here.
//!
//! The agent's state is read from `ssh-add -l`'s **exit code**, never from its
//! message text: the wording ("The agent has no identities.") varies by locale
//! and implementation, while the codes are contractual — 0 = has keys,
//! 1 = reachable but empty, 2 = no agent reachable.
//!
//! Membership is decided by SHA256 **public fingerprint**, not by path: the
//! agent does not retain where a key was loaded from, so two copies of the same
//! key at different paths are (correctly) one entry.

use std::collections::HashSet;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::JoinHandle;

use super::binaries::tools;
use super::keys::parse_fingerprint_line;

/// What the ssh-agent is doing, as far as we could tell.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AgentStatus {
    /// A probe is in flight; we have no answer yet.
    #[default]
    Probing,
    /// The agent answered. Holds the SHA256 public fingerprints it has loaded
    /// (empty when the agent is running but holds nothing).
    Running(HashSet<String>),
    /// `ssh-add` ran and reported it could not reach an agent (exit 2).
    NotRunning,
    /// `ssh-add` itself could not be run, or failed in a way we can't read.
    /// Distinct from [`AgentStatus::NotRunning`] because the advice differs:
    /// "install/By OpenSSH" vs "start the agent".
    Unavailable,
}

/// Where a *particular* key stands relative to the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAgentState {
    /// The agent holds this key.
    Loaded,
    /// The agent answered and does not hold this key.
    NotLoaded,
    /// This key has no readable fingerprint (e.g. an encrypted legacy PEM whose
    /// public half `ssh-keygen` won't surface), so membership cannot be decided.
    /// Deliberately NOT reported as `NotLoaded` — that would be a false negative.
    Unknown,
    /// No agent to compare against, so the question has no per-key answer.
    NoAgent,
}

/// State of the Windows `ssh-agent` **service**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    Running,
    Stopped,
    /// A transitional state (start/stop pending).
    Transitioning,
    /// No `STATE` line found — service absent, or `sc.exe` failed.
    Unknown,
}

/// Map one `ssh-add -l` run to an [`AgentStatus`]. `exit` is `None` when
/// `ssh-add` could not be run at all (or was killed by a signal).
///
/// Pure: takes the already-captured exit code and stdout so it is unit-testable
/// without an agent, on any platform.
pub fn status_from_exit(exit: Option<i32>, stdout: &str) -> AgentStatus {
    match exit {
        Some(0) => AgentStatus::Running(parse_loaded_fingerprints(stdout)),
        Some(1) => AgentStatus::Running(HashSet::new()),
        Some(2) => AgentStatus::NotRunning,
        // Anything outside the documented 0/1/2 contract (including a spawn
        // failure or a signal kill, both `None`) is not readable as either
        // "running" or "stopped" — say so rather than guessing.
        _ => AgentStatus::Unavailable,
    }
}

/// Parse `ssh-add -l` output into the set of SHA256 public fingerprints it
/// reports. Lines that are not fingerprint lines (such as the "no identities"
/// notice) are skipped.
pub fn parse_loaded_fingerprints(stdout: &str) -> HashSet<String> {
    // `parse_fingerprint_line` requires a leading bit-count integer, so prose
    // lines (the "no identities" notice, error text) are rejected for free.
    stdout
        .lines()
        .filter_map(|line| parse_fingerprint_line(line).map(|(_, fp, _, _)| fp))
        .collect()
}

/// Decide how a key stands relative to the agent. `key_fingerprint` is
/// [`super::keys::KeyInfo::fingerprint`], which is **empty** when it could not
/// be read.
pub fn key_state(status: &AgentStatus, key_fingerprint: &str) -> KeyAgentState {
    // Checked before the agent state: with no fingerprint we cannot decide
    // membership even against a healthy agent, and reporting `NotLoaded` there
    // would be a false negative on exactly the keys most likely to be loaded.
    if key_fingerprint.is_empty() {
        return KeyAgentState::Unknown;
    }
    match status {
        AgentStatus::Running(fingerprints) => {
            if fingerprints.contains(key_fingerprint) {
                KeyAgentState::Loaded
            } else {
                KeyAgentState::NotLoaded
            }
        }
        _ => KeyAgentState::NoAgent,
    }
}

/// Parse `sc.exe query ssh-agent` output into a [`ServiceState`].
///
/// Reads the **numeric** token of the `STATE` line (`STATE : 4  RUNNING`), not
/// the trailing label: on a localized Windows the label is translated (e.g.
/// `実行中`), so matching on it would break outside English. Kept free of
/// `#[cfg(windows)]` so it is unit-testable on every platform (the spawn that
/// feeds it is the Windows-only part).
pub fn parse_service_state(sc_stdout: &str) -> ServiceState {
    for line in sc_stdout.lines() {
        let Some((label, rest)) = line.split_once(':') else {
            continue;
        };
        if !label.trim().eq_ignore_ascii_case("STATE") {
            continue;
        }
        let Some(code) = rest
            .split_whitespace()
            .next()
            .and_then(|token| token.parse::<u32>().ok())
        else {
            continue;
        };
        // Win32 SERVICE_STATUS codes: 1 STOPPED, 2 START_PENDING,
        // 3 STOP_PENDING, 4 RUNNING, 5 CONTINUE_PENDING, 6 PAUSE_PENDING,
        // 7 PAUSED.
        return match code {
            1 => ServiceState::Stopped,
            4 => ServiceState::Running,
            2 | 3 | 5 | 6 | 7 => ServiceState::Transitioning,
            _ => ServiceState::Unknown,
        };
    }
    ServiceState::Unknown
}

/// One complete answer from a probe: what the agent holds, plus (on Windows)
/// the state of the service that hosts it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentSnapshot {
    pub status: AgentStatus,
    /// `None` where the question does not apply (non-Windows has no ssh-agent
    /// *service*), which is what makes the UI omit the row rather than render a
    /// misleading "unknown" on a platform that will never have one.
    pub service: Option<ServiceState>,
}

/// Build a snapshot from already-captured command output. Pure, so the whole
/// composition is testable without an agent or a Windows box; the thread body
/// only supplies the real output.
///
/// `sc_stdout` is `None` off Windows (and when `sc.exe` could not be run), which
/// is what makes the UI omit the service row entirely rather than show it
/// as "unknown" on a platform that has no such service.
pub fn snapshot_from(
    ssh_add_exit: Option<i32>,
    ssh_add_stdout: &str,
    sc_stdout: Option<&str>,
) -> AgentSnapshot {
    AgentSnapshot {
        status: status_from_exit(ssh_add_exit, ssh_add_stdout),
        service: sc_stdout.map(parse_service_state),
    }
}

/// Arguments for loading `key_path` into the agent (`ssh-add <path>`).
///
/// The path is passed verbatim — no quoting, no tilde expansion. We spawn the
/// binary directly (never through a shell), so a Windows path with spaces must
/// NOT be pre-quoted or the quotes become part of the filename.
pub fn load_args(key_path: &Path) -> Vec<String> {
    vec![key_path.display().to_string()]
}

/// Arguments for removing `key_path` from the agent (`ssh-add -d <path>`).
pub fn unload_args(key_path: &Path) -> Vec<String> {
    vec!["-d".to_string(), key_path.display().to_string()]
}

/// A probe in flight. One job, so this is a single detached thread reporting
/// over `mpsc` rather than [`super::liveness::LivenessProbe`]'s worker pool —
/// the `drain` contract is the same, the queue machinery would be dead weight.
///
/// The UI thread never blocks: it calls [`AgentProbe::drain`] once per tick.
/// That matters most in the failure case this feature exists to surface — a
/// wedged Windows ssh-agent service, where a synchronous `ssh-add -l` would
/// freeze the draw loop rather than report the problem.
pub struct AgentProbe {
    rx: Receiver<AgentSnapshot>,
    _handle: JoinHandle<()>,
}

impl AgentProbe {
    /// Start probing. Returns immediately; the answer arrives via [`Self::drain`].
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let _ = tx.send(probe_now());
        });
        AgentProbe {
            rx,
            _handle: handle,
        }
    }

    /// Non-blocking. Returns the snapshot if one has arrived, and `true` once
    /// the channel has closed (the probe finished and the caller may drop this).
    pub fn drain(&self) -> (Option<AgentSnapshot>, bool) {
        let mut latest = None;
        loop {
            match self.rx.try_recv() {
                Ok(snapshot) => latest = Some(snapshot),
                Err(TryRecvError::Empty) => return (latest, false),
                Err(TryRecvError::Disconnected) => return (latest, true),
            }
        }
    }
}

/// Run the probe commands and compose a snapshot. Blocking — always called on a
/// worker thread, never on the UI thread.
fn probe_now() -> AgentSnapshot {
    let output = Command::new(&tools().ssh_add)
        .arg("-l")
        .stdin(Stdio::null())
        .output();
    let (exit, stdout) = match &output {
        Ok(o) => (o.status.code(), String::from_utf8_lossy(&o.stdout)),
        // `ssh-add` is absent or unrunnable — Unavailable, not NotRunning.
        Err(_) => (None, std::borrow::Cow::Borrowed("")),
    };
    let service = query_service_state();
    snapshot_from(exit, &stdout, service.as_deref())
}

/// Capture `sc.exe query ssh-agent` output. `None` off Windows (no such
/// service) and when `sc.exe` cannot be run at all.
#[cfg(windows)]
fn query_service_state() -> Option<String> {
    let output = Command::new("sc.exe")
        .args(["query", "ssh-agent"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    // sc.exe reports a missing service via a non-zero exit AND prints no STATE
    // line, so the parser handles both; take stdout either way.
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(not(windows))]
fn query_service_state() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ssh-add -l` prints the same shape as `ssh-keygen -l`, one line per key.
    const TWO_KEYS: &str = "\
256 SHA256:AAAAxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx me@desktop (ED25519)
3072 SHA256:BBBByyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy me@laptop (RSA)";

    // --- AC1: agent status comes from the exit code, not the message text ---

    #[test]
    fn status_from_exit_maps_zero_to_running_with_the_listed_keys() {
        // given / when
        let status = status_from_exit(Some(0), TWO_KEYS);
        // then
        let AgentStatus::Running(fps) = status else {
            panic!("exit 0 must mean the agent is running, got {status:?}");
        };
        assert_eq!(fps.len(), 2);
        assert!(fps.contains("SHA256:AAAAxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"));
    }

    #[test]
    fn status_from_exit_maps_one_to_running_with_no_keys() {
        // given: exit 1 means "reachable, but holds nothing" — NOT "no agent".
        let status = status_from_exit(Some(1), "The agent has no identities.");
        // then
        assert_eq!(status, AgentStatus::Running(HashSet::new()));
    }

    #[test]
    fn status_from_exit_maps_two_to_not_running() {
        let status = status_from_exit(Some(2), "Error connecting to agent");
        assert_eq!(status, AgentStatus::NotRunning);
    }

    // --- AC2: a missing `ssh-add` is a different state than a stopped agent ---

    #[test]
    fn status_from_exit_maps_a_spawn_failure_to_unavailable() {
        // given: None = ssh-add could not be run at all.
        assert_eq!(status_from_exit(None, ""), AgentStatus::Unavailable);
    }

    #[test]
    fn status_from_exit_maps_an_unexpected_code_to_unavailable() {
        // An exit code outside the documented 0/1/2 contract must not be
        // silently read as "running" or "not running".
        assert_eq!(status_from_exit(Some(77), ""), AgentStatus::Unavailable);
    }

    // --- AC3: membership is by fingerprint ---

    #[test]
    fn parse_loaded_fingerprints_reads_every_listed_key() {
        let fps = parse_loaded_fingerprints(TWO_KEYS);
        assert_eq!(fps.len(), 2);
        assert!(fps.contains("SHA256:BBBByyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy"));
    }

    #[test]
    fn parse_loaded_fingerprints_skips_the_no_identities_notice() {
        // The notice is prose, not a fingerprint line — it must not become an entry.
        assert!(parse_loaded_fingerprints("The agent has no identities.").is_empty());
    }

    #[test]
    fn key_state_reports_loaded_when_the_agent_holds_the_fingerprint() {
        let status = status_from_exit(Some(0), TWO_KEYS);
        let state = key_state(
            &status,
            "SHA256:AAAAxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        );
        assert_eq!(state, KeyAgentState::Loaded);
    }

    #[test]
    fn key_state_reports_not_loaded_when_the_agent_lacks_the_fingerprint() {
        let status = status_from_exit(Some(0), TWO_KEYS);
        let state = key_state(&status, "SHA256:ZZZZnotloadedzzzzzzzzzzzzzzzzzzzzzzzzzzz");
        assert_eq!(state, KeyAgentState::NotLoaded);
    }

    // --- AC4: an unreadable fingerprint is Unknown, never a false "not loaded" ---

    #[test]
    fn key_state_reports_unknown_for_a_key_whose_fingerprint_could_not_be_read() {
        // KeyInfo::fingerprint is empty when `ssh-keygen -l` failed (encrypted
        // legacy PEM). Reporting NotLoaded there would be a false negative: the
        // key may well be in the agent.
        let status = status_from_exit(Some(0), TWO_KEYS);
        assert_eq!(key_state(&status, ""), KeyAgentState::Unknown);
    }

    #[test]
    fn key_state_reports_no_agent_when_the_agent_is_not_running() {
        let state = key_state(
            &AgentStatus::NotRunning,
            "SHA256:AAAAxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        );
        assert_eq!(state, KeyAgentState::NoAgent);
    }

    #[test]
    fn key_state_reports_no_agent_while_a_probe_is_still_in_flight() {
        let state = key_state(
            &AgentStatus::Probing,
            "SHA256:AAAAxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        );
        assert_eq!(state, KeyAgentState::NoAgent);
    }

    // --- AC7/AC8: the Windows service state is read as a number ---

    const SC_RUNNING: &str = "\
SERVICE_NAME: ssh-agent
        TYPE               : 20  WIN32_SHARE_PROCESS
        STATE              : 4  RUNNING
                                (STOPPABLE, PAUSABLE, ACCEPTS_SHUTDOWN)
        WIN32_EXIT_CODE    : 0  (0x0)";

    #[test]
    fn parse_service_state_reads_the_numeric_running_token() {
        assert_eq!(parse_service_state(SC_RUNNING), ServiceState::Running);
    }

    #[test]
    fn parse_service_state_ignores_a_localized_state_label() {
        // B49 regression: on Japanese Windows sc.exe translates the trailing
        // label, so matching "RUNNING" as text would report the service stopped
        // on every non-English install. Only the numeric 4 is contractual.
        let localized = "\
SERVICE_NAME: ssh-agent
        TYPE               : 20  WIN32_SHARE_PROCESS
        STATE              : 4  実行中
        WIN32_EXIT_CODE    : 0  (0x0)";
        assert_eq!(parse_service_state(localized), ServiceState::Running);
    }

    #[test]
    fn parse_service_state_reads_stopped() {
        let stopped = "\
SERVICE_NAME: ssh-agent
        STATE              : 1  STOPPED";
        assert_eq!(parse_service_state(stopped), ServiceState::Stopped);
    }

    #[test]
    fn parse_service_state_reports_transitioning_for_a_pending_state() {
        let pending = "        STATE              : 2  START_PENDING";
        assert_eq!(parse_service_state(pending), ServiceState::Transitioning);
    }

    #[test]
    fn parse_service_state_reports_unknown_when_the_service_is_absent() {
        // sc.exe on a machine without the service prints an error, no STATE line.
        let absent = "[SC] EnumQueryServicesStatus:OpenService FAILED 1060:";
        assert_eq!(parse_service_state(absent), ServiceState::Unknown);
    }

    // --- AC5/AC6: the ssh-add argument contract ---

    // --- AC9/AC10: snapshot composition, and the platform-scoped service row ---

    #[test]
    fn snapshot_from_combines_the_agent_and_service_answers() {
        let snap = snapshot_from(Some(0), TWO_KEYS, Some(SC_RUNNING));
        assert_eq!(snap.status, status_from_exit(Some(0), TWO_KEYS));
        assert_eq!(snap.service, Some(ServiceState::Running));
    }

    #[test]
    fn snapshot_from_omits_the_service_when_it_does_not_apply() {
        // AC9: off Windows there is no ssh-agent service, so the row must be
        // absent — not rendered as "unknown", which would imply we failed to
        // read something that exists.
        let snap = snapshot_from(Some(1), "", None);
        assert_eq!(snap.service, None);
    }

    #[test]
    fn snapshot_from_reports_a_stopped_service_alongside_a_dead_agent() {
        // The pairing users actually hit on Windows: service stopped, so the
        // agent is unreachable. Both facts must survive into one snapshot.
        let stopped = "        STATE              : 1  STOPPED";
        let snap = snapshot_from(Some(2), "Error connecting to agent", Some(stopped));
        assert_eq!(snap.status, AgentStatus::NotRunning);
        assert_eq!(snap.service, Some(ServiceState::Stopped));
    }

    #[test]
    fn load_args_passes_the_key_path_alone() {
        let args = load_args(Path::new("/home/me/.ssh/id_ed25519"));
        assert_eq!(args, vec!["/home/me/.ssh/id_ed25519".to_string()]);
    }

    #[test]
    fn unload_args_puts_the_delete_flag_before_the_path() {
        let args = unload_args(Path::new("/home/me/.ssh/id_ed25519"));
        assert_eq!(
            args,
            vec!["-d".to_string(), "/home/me/.ssh/id_ed25519".to_string()]
        );
    }

    #[test]
    fn load_args_does_not_quote_a_path_containing_spaces() {
        // We spawn ssh-add directly, never via a shell. Pre-quoting here would
        // make the quotes part of the filename and the load would fail with a
        // confusing "No such file" on exactly the Windows paths that need it
        // most (C:\Users\First Last\.ssh\...).
        let args = load_args(Path::new(r"C:\Users\First Last\.ssh\id_rsa"));
        assert_eq!(args, vec![r"C:\Users\First Last\.ssh\id_rsa".to_string()]);
    }
}
