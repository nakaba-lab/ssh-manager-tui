//! Host-key pre-scan via `ssh-keyscan` (#46).
//!
//! Pure helpers (argument building, output parsing, host-token normalization,
//! classification against existing pins) plus a dedicated worker session
//! (thread + mpsc, drained per tick) mirroring `os/sftp.rs::SftpSession` — the
//! scan takes seconds and must never run on the UI thread.
//!
//! Zero ratatui dependency — see CLAUDE.md layering.

use std::io;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::binaries::tools;
use super::keys::parse_fingerprint_line;
use super::known_hosts::KnownHostEntry;
use crate::secure_fs;

/// Per-connection timeout handed to `ssh-keyscan -T` (seconds).
pub const SCAN_TIMEOUT_SECS: u32 = 5;

/// Hard wall-clock budget for each scan subprocess. Slightly above the `-T`
/// per-connection timeout so keyscan normally times out on its own terms; the
/// kill is the backstop for a wedged process.
const SCAN_BUDGET: Duration = Duration::from_secs(8);

/// One fully-resolved scanned host key: the raw known_hosts material
/// (`key_type` + `key_b64`) joined with its `ssh-keygen -lv` presentation
/// (bits, SHA256 fingerprint, randomart block).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedKey {
    /// known_hosts key type token, e.g. `ssh-ed25519`.
    pub key_type: String,
    /// Base-64 key body as printed by `ssh-keyscan`.
    pub key_b64: String,
    /// `SHA256:...` public fingerprint (from `ssh-keygen -lv -f`).
    pub fingerprint: String,
    pub bits: u32,
    /// The full randomart block, borders included, one string per line.
    pub randomart: Vec<String>,
}

/// One raw `ssh-keyscan` output line: `host keytype base64` (host token is
/// verbatim, e.g. `[db.example.com]:2222` for a non-22 port scan).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyscanLine {
    pub host: String,
    pub key_type: String,
    pub key_b64: String,
}

/// One `ssh-keygen -lv -f` block: the fingerprint line (parsed in the
/// `keys::parse_fingerprint_line` shape) plus the randomart block that
/// immediately follows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerprintBlock {
    pub bits: u32,
    /// `SHA256:...`.
    pub fingerprint: String,
    /// Type as printed in the trailing parens, e.g. `ED25519`.
    pub key_type: String,
    /// Randomart lines, `+--[...]--+` borders included.
    pub randomart: Vec<String>,
}

/// Classification of a scanned key against the existing known_hosts entries
/// for the same lookup key (#46 AC 4/5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinClass {
    /// No existing entry of this key type — eligible for pinning.
    New,
    /// An existing entry with the same type AND base64 — already trusted,
    /// must NOT be appended again.
    AlreadyTrusted,
    /// An existing entry with the same type but a DIFFERENT base64 —
    /// HOST KEY CHANGED. Warn only; never offer a one-key overwrite.
    Changed,
    /// A `@revoked` entry matches this exact key: the user explicitly marked it
    /// compromised. Never presentable as trusted, never pinnable.
    Revoked,
}

impl PinClass {
    /// True for a class that makes the WHOLE scan result untrustworthy: a
    /// contradicted or revoked key means the channel is showing something that
    /// disagrees with what the user already pinned, so no sibling key from the
    /// same scan may be pinned either (#46 review: an attacker's second key of
    /// another type would otherwise be appended next to a genuine pin, and
    /// OpenSSH accepts ANY matching entry — defeating the pin with no warning).
    pub fn poisons_result(self) -> bool {
        matches!(self, PinClass::Changed | PinClass::Revoked)
    }
}

/// A completed background scan, reported over the session channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyscanEvent {
    /// The scan succeeded: all keys the server offered, fingerprinted.
    Keys(Vec<ScannedKey>),
    /// The scan failed (unreachable / timed out / tool error): a short,
    /// user-facing reason.
    Failed(String),
}

/// Build the `ssh-keyscan` argument list: `-T <timeout>` and `-p <port>`
/// first, the host as the FINAL argument. `ssh-keyscan` has no `--`
/// end-of-options sentinel, so a host beginning with `-` must be rejected
/// outright (`InvalidInput`) — the same argv flag-smuggling guard as
/// `resolve_config_with_options`.
pub fn keyscan_args(host: &str, port: &str, timeout_secs: u32) -> io::Result<Vec<String>> {
    if host.starts_with('-') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "host may not start with '-'",
        ));
    }
    Ok(vec![
        "-T".to_string(),
        timeout_secs.to_string(),
        "-p".to_string(),
        port.to_string(),
        host.to_string(),
    ])
}

/// Parse `ssh-keyscan` stdout into `host keytype base64` lines. `#` comment
/// lines, blank lines and malformed lines are skipped; a trailing `\r` (CRLF
/// output) must not leak into the base64 field.
pub fn parse_keyscan_output(out: &str) -> Vec<KeyscanLine> {
    out.lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut fields = l.split_whitespace();
            Some(KeyscanLine {
                host: fields.next()?.to_string(),
                key_type: fields.next()?.to_string(),
                key_b64: fields.next()?.to_string(),
            })
        })
        .collect()
}

/// Split `ssh-keygen -lv -f` output into per-key blocks: each fingerprint
/// line (same shape as `keys::parse_fingerprint_line`) followed by its
/// randomart block (`+--[...]--+` through `+----[SHA256]-----+`).
pub fn parse_keygen_lv_output(out: &str) -> Vec<FingerprintBlock> {
    let mut blocks: Vec<FingerprintBlock> = Vec::new();
    for line in out.lines().map(|l| l.trim_end_matches('\r')) {
        if let Some((bits, fingerprint, _comment, key_type)) = parse_fingerprint_line(line) {
            blocks.push(FingerprintBlock {
                bits,
                fingerprint,
                key_type,
                randomart: Vec::new(),
            });
        } else if let Some(block) = blocks.last_mut()
            && (line.starts_with('+') || line.starts_with('|'))
        {
            block.randomart.push(line.to_string());
        }
    }
    blocks
}

/// Render the known_hosts lines to append on approval: each scanned key as
/// `<lookup_key> <key_type> <key_b64>`, with the host token REWRITTEN to the
/// TOFU lookup key (`tofu_lookup_key` output: HostKeyAlias verbatim, else
/// `host` / `[host]:port`) so the pin is found by the same `ssh-keygen -F`
/// lookup `is_host_known` gates on.
pub fn pinned_lines(keys: &[ScannedKey], lookup_key: &str) -> Vec<String> {
    keys.iter()
        .map(|k| format!("{lookup_key} {} {}", k.key_type, k.key_b64))
        .collect()
}

/// Classify one scanned key against the entries OpenSSH itself matches for this
/// host (`resolve::matching_known_entries` — so hashed / wildcard / case
/// matching is OpenSSH's, not ours).
///
/// Marker handling is load-bearing (#46 review):
/// - `@revoked` matching this exact key → `Revoked`. Presenting a revoked key
///   as "already trusted" would be the strongest possible false reassurance in
///   the one state this feature exists to make honest.
/// - `@cert-authority` lines are CA delegations, not host-key pins, so they
///   take no part in classification (a CA line must never make a scanned host
///   key read as `AlreadyTrusted`, nor as `Changed`).
pub fn classify_key(key: &ScannedKey, existing: &[KnownHostEntry]) -> PinClass {
    let same_key = |e: &&KnownHostEntry| e.key_type == key.key_type && e.key_b64 == key.key_b64;
    if existing
        .iter()
        .filter(|e| e.marker.as_deref() == Some("@revoked"))
        .any(|e| same_key(&e))
    {
        return PinClass::Revoked;
    }
    // Pins only: markers are not host-key pins (`is_host_known` excludes them
    // from the trust gate for the same reason).
    let pins = || existing.iter().filter(|e| e.marker.is_none());
    if pins().any(|e| same_key(&e)) {
        PinClass::AlreadyTrusted
    } else if pins().any(|e| e.key_type == key.key_type) {
        PinClass::Changed
    } else {
        PinClass::New
    }
}

/// A host-key scan session: dispatches `ssh-keyscan` + `ssh-keygen -lv` onto
/// a worker thread and reports back over an mpsc channel the UI drains per
/// tick (mirroring `SftpSession::request`/`drain` — never blocks the UI).
pub struct KeyscanSession {
    tx: Sender<KeyscanEvent>,
    rx: Receiver<KeyscanEvent>,
    handles: Vec<JoinHandle<()>>,
}

impl KeyscanSession {
    /// Open a session. Cheap: nothing is spawned until [`Self::request`].
    pub fn open() -> Self {
        let (tx, rx) = mpsc::channel();
        KeyscanSession {
            tx,
            rx,
            handles: Vec::new(),
        }
    }

    /// Dispatch a scan of `host:port` to a worker thread; the resulting
    /// [`KeyscanEvent`] arrives via [`Self::drain`]. Never blocks.
    pub fn request(&mut self, host: &str, port: &str) {
        let host = host.to_string();
        let port = port.to_string();
        let tx = self.tx.clone();
        let handle = std::thread::spawn(move || {
            let event = match scan_host(&host, &port) {
                Ok(keys) => KeyscanEvent::Keys(keys),
                Err(e) => KeyscanEvent::Failed(e.to_string()),
            };
            // The receiver may be gone (modal closed) — the worker just exits.
            let _ = tx.send(event);
        });
        self.handles.push(handle);
    }

    /// Non-blocking drain of completed scan events; also reaps finished
    /// worker threads.
    pub fn drain(&mut self) -> Vec<KeyscanEvent> {
        let mut events = Vec::new();
        while let Ok(e) = self.rx.try_recv() {
            events.push(e);
        }
        self.handles.retain(|h| !h.is_finished());
        events
    }

    /// Test seam: a clone of the worker-side sender, so plumbing tests can
    /// inject events without spawning a real `ssh-keyscan`.
    #[cfg(test)]
    pub fn sender(&self) -> Sender<KeyscanEvent> {
        self.tx.clone()
    }
}

/// Worker body: scan `host:port` and fingerprint the result. Spawns real
/// `ssh-keyscan` / `ssh-keygen` subprocesses — worker thread only, never the
/// UI thread.
fn scan_host(host: &str, port: &str) -> io::Result<Vec<ScannedKey>> {
    let args = keyscan_args(host, port, SCAN_TIMEOUT_SECS)?;
    let out = run_bounded(
        Command::new(&tools().ssh_keyscan).args(&args),
        "ssh-keyscan",
    )?;
    let lines = parse_keyscan_output(&out);
    if lines.is_empty() {
        return Err(io::Error::other(format!(
            "no host keys returned for {host}:{port} (unreachable or timed out)"
        )));
    }
    fingerprint_lines(&lines)
}

/// Bounded subprocess runner mirroring `resolve::run_ssh_g`: stdin nulled,
/// spawn / try_wait / kill loop under [`SCAN_BUDGET`], kill + wait on timeout.
fn run_bounded(cmd: &mut Command, what: &str) -> io::Result<String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let start = Instant::now();
    loop {
        // Exit status is deliberately not checked: `ssh-keyscan` exits non-zero
        // when any target fails yet still prints the keys it did collect, so
        // emptiness is judged by the caller from the parsed output.
        if child.try_wait()?.is_some() {
            break;
        }
        if start.elapsed() > SCAN_BUDGET {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{what} timed out"),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output()?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Write the scanned lines to a private temp file and run `ssh-keygen -lv -f`
/// over it, joining each key line with its fingerprint + randomart block by
/// position (ssh-keygen emits one block per file line, in order).
fn fingerprint_lines(lines: &[KeyscanLine]) -> io::Result<Vec<ScannedKey>> {
    let tmp = std::env::temp_dir().join(secure_fs::temp_name(".sshm-keyscan")?);
    // Written in a closure so a mid-write failure still reaches the cleanup
    // below rather than leaking the temp file (#46 review).
    let written = (|| {
        use std::io::Write;
        let mut file = secure_fs::create_new_private(&tmp)?;
        for l in lines {
            writeln!(file, "{} {} {}", l.host, l.key_type, l.key_b64)?;
        }
        file.sync_all()
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let result = run_bounded(
        Command::new(&tools().ssh_keygen)
            .arg("-lv")
            .arg("-f")
            .arg(&tmp),
        "ssh-keygen -lv",
    );
    let _ = std::fs::remove_file(&tmp);
    let blocks = parse_keygen_lv_output(&result?);
    if blocks.len() != lines.len() {
        return Err(io::Error::other(
            "could not fingerprint the scanned keys (unexpected ssh-keygen output)",
        ));
    }
    Ok(lines
        .iter()
        .zip(blocks)
        .map(|(l, b)| ScannedKey {
            key_type: l.key_type.clone(),
            key_b64: l.key_b64.clone(),
            fingerprint: b.fingerprint,
            bits: b.bits,
            randomart: b.randomart,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os::known_hosts::parse_line;

    /// Fixture: a scanned key with presentation fields filled mechanically
    /// (classification and pinning only read `key_type` / `key_b64`).
    fn sk(key_type: &str, key_b64: &str) -> ScannedKey {
        ScannedKey {
            key_type: key_type.to_string(),
            key_b64: key_b64.to_string(),
            fingerprint: format!("SHA256:fp-of-{key_b64}"),
            bits: 256,
            randomart: Vec::new(),
        }
    }

    // -- keyscan_args (AC1: bounded scan / flag-smuggling guard) ------------

    #[test]
    fn keyscan_args_builds_timeout_port_then_host() {
        // given / when — a non-22 port with a 5s per-connection timeout
        let args = keyscan_args("db.example.com", "2222", 5).unwrap();
        // then — flags first, the untrusted host is the FINAL argument
        assert_eq!(args, ["-T", "5", "-p", "2222", "db.example.com"]);

        // given / when — the default port is passed explicitly all the same
        let args22 = keyscan_args("web1.example.com", "22", 5).unwrap();
        // then
        assert_eq!(args22, ["-T", "5", "-p", "22", "web1.example.com"]);
    }

    #[test]
    fn keyscan_args_rejects_leading_dash_host() {
        // given — ssh-keyscan has no `--` sentinel, so a dash-host would be
        // parsed as an option (flag smuggling)
        // when
        let err = keyscan_args("-oProxyCommand=evil", "22", 5).unwrap_err();
        // then — refused outright, same guard as resolve_config_with_options
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    // -- parse_keyscan_output (AC1) -----------------------------------------

    #[test]
    fn parse_keyscan_output_extracts_key_lines_and_skips_comments() {
        // given — comment banner, CRLF line ending, blank + malformed lines
        let out = "# [db.example.com]:2222 SSH-2.0-OpenSSH_9.6\n\
                   [db.example.com]:2222 ssh-ed25519 AAAAC3NzEd\r\n\
                   [db.example.com]:2222 ssh-rsa AAAAB3NzRsa\n\
                   \n\
                   not-a-key-line\n\
                   # [db.example.com]:2222 SSH-2.0-OpenSSH_9.6\n";
        // when
        let lines = parse_keyscan_output(out);
        // then — exactly the two key lines survive, fields verbatim, no \r
        assert_eq!(
            lines,
            vec![
                KeyscanLine {
                    host: "[db.example.com]:2222".into(),
                    key_type: "ssh-ed25519".into(),
                    key_b64: "AAAAC3NzEd".into(),
                },
                KeyscanLine {
                    host: "[db.example.com]:2222".into(),
                    key_type: "ssh-rsa".into(),
                    key_b64: "AAAAB3NzRsa".into(),
                },
            ]
        );
    }

    // -- parse_keygen_lv_output (AC1: fingerprint + randomart display) ------

    #[test]
    fn parse_keygen_lv_output_splits_fingerprint_and_randomart_blocks() {
        // given — two keys as `ssh-keygen -lv -f` prints them (11-line art each)
        let out = "\
256 SHA256:RlbEd25519Fp [db.example.com]:2222 (ED25519)
+--[ED25519 256]--+
|      .o+o.      |
|     . o=o       |
|      +.S.       |
|     o +.o       |
|    . = +        |
|     + = .       |
|    . + o        |
|     . +         |
|      E          |
+----[SHA256]-----+
3072 SHA256:QqqRsaFp [db.example.com]:2222 (RSA)
+---[RSA 3072]----+
|       .         |
|      . .        |
|       o         |
|      . +        |
|     .oS=        |
|    .o.=+.       |
|    ..o+o        |
|   .  =*+        |
|    .E=*=        |
+----[SHA256]-----+
";
        // when
        let blocks = parse_keygen_lv_output(out);
        // then — one block per key, art bounded by its borders
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].bits, 256);
        assert_eq!(blocks[0].fingerprint, "SHA256:RlbEd25519Fp");
        assert_eq!(blocks[0].key_type, "ED25519");
        assert_eq!(blocks[0].randomart.len(), 11);
        assert_eq!(blocks[0].randomart[0], "+--[ED25519 256]--+");
        assert_eq!(blocks[0].randomart[10], "+----[SHA256]-----+");
        assert_eq!(blocks[1].bits, 3072);
        assert_eq!(blocks[1].fingerprint, "SHA256:QqqRsaFp");
        assert_eq!(blocks[1].key_type, "RSA");
        assert_eq!(blocks[1].randomart.len(), 11);
    }

    // -- pinned_lines (AC2: host token normalized to the TOFU lookup key) ---

    #[test]
    fn pinned_lines_rewrite_host_token_to_lookup_key() {
        // given — keys scanned from a bracketed non-22 target
        let keys = vec![sk("ssh-ed25519", "AAAAKEY1"), sk("ssh-rsa", "AAAAKEY2")];

        // when — pinned under the `[host]:port` lookup key
        let bracketed = pinned_lines(&keys, "[db.example.com]:2222");
        // then — one known_hosts line per key, lookup key as the host token
        assert_eq!(
            bracketed,
            vec![
                "[db.example.com]:2222 ssh-ed25519 AAAAKEY1".to_string(),
                "[db.example.com]:2222 ssh-rsa AAAAKEY2".to_string(),
            ]
        );

        // when — a HostKeyAlias lookup key is used verbatim
        let aliased = pinned_lines(&keys[..1], "web1ka");
        // then
        assert_eq!(aliased, vec!["web1ka ssh-ed25519 AAAAKEY1".to_string()]);
    }

    // -- classify_key (AC4 / AC5) -------------------------------------------

    #[test]
    fn classify_key_new_trusted_and_changed() {
        // given — existing pins: one ed25519, one ecdsa
        let existing: Vec<KnownHostEntry> = [
            "db.example ssh-ed25519 OLDKEYAAA",
            "db.example ecdsa-sha2-nistp256 OLDECDSA",
        ]
        .iter()
        .enumerate()
        .filter_map(|(i, l)| parse_line(l, i))
        .collect();

        // when / then — table-driven over (type, b64, expected class)
        let cases = [
            ("ssh-ed25519", "OLDKEYAAA", PinClass::AlreadyTrusted), // exact match
            ("ssh-ed25519", "EVILKEYBBB", PinClass::Changed),       // same type, new b64
            ("ssh-rsa", "BRANDNEWCCC", PinClass::New),              // type not pinned yet
        ];
        for (key_type, b64, expected) in cases {
            assert_eq!(
                classify_key(&sk(key_type, b64), &existing),
                expected,
                "({key_type}, {b64})"
            );
        }
    }

    #[test]
    fn classify_key_revoked_marker_is_never_trusted() {
        // given — the user explicitly revoked a compromised key (#46 review)
        let existing: Vec<KnownHostEntry> = ["@revoked db.example ssh-ed25519 EVILKEY"]
            .iter()
            .enumerate()
            .filter_map(|(i, l)| parse_line(l, i))
            .collect();
        // when — the scan returns that very key (attacker still in path)
        let class = classify_key(&sk("ssh-ed25519", "EVILKEY"), &existing);
        // then — surfaced as revoked, never as "already trusted"
        assert_eq!(class, PinClass::Revoked);
        assert!(class.poisons_result(), "a revoked key must block pinning");
    }

    #[test]
    fn classify_key_ignores_cert_authority_lines() {
        // given — a CA delegation line, which is not a host-key pin
        let existing: Vec<KnownHostEntry> = ["@cert-authority *.example ssh-ed25519 CAKEY"]
            .iter()
            .enumerate()
            .filter_map(|(i, l)| parse_line(l, i))
            .collect();
        // when / then — a scanned key of the same type is still unpinned (New),
        // and the CA's own key material must not read as trusted either
        assert_eq!(
            classify_key(&sk("ssh-ed25519", "HOSTKEY"), &existing),
            PinClass::New
        );
        assert_eq!(
            classify_key(&sk("ssh-ed25519", "CAKEY"), &existing),
            PinClass::New
        );
    }

    #[test]
    fn classify_key_match_beats_stale_mismatch_line() {
        // given — the file carries BOTH an old and the current ed25519 line
        let existing: Vec<KnownHostEntry> = [
            "db.example ssh-ed25519 STALEOLD",
            "db.example ssh-ed25519 CURRENTKEY",
        ]
        .iter()
        .enumerate()
        .filter_map(|(i, l)| parse_line(l, i))
        .collect();
        // when — the scan returns the current key
        let class = classify_key(&sk("ssh-ed25519", "CURRENTKEY"), &existing);
        // then — any exact match means trusted (never a false CHANGED alarm)
        assert_eq!(class, PinClass::AlreadyTrusted);
    }

    // -- worker plumbing (AC1: scan never blocks the UI thread) -------------

    #[test]
    fn keyscan_session_drain_is_nonblocking_and_empty_initially() {
        // given — a fresh session with no scan dispatched
        let mut session = KeyscanSession::open();
        // when — the UI tick drains
        let events = session.drain();
        // then — nothing to report and the call returned (did not block)
        assert!(events.is_empty());
    }

    #[test]
    fn keyscan_session_drain_collects_worker_events() {
        // given — a session and its worker-side sender (test seam)
        let mut session = KeyscanSession::open();
        let tx = session.sender();
        // when — a worker reports a completed (failed) scan
        tx.send(KeyscanEvent::Failed("connection timed out".into()))
            .unwrap();
        // then — the next drain hands the event to the UI
        assert_eq!(
            session.drain(),
            vec![KeyscanEvent::Failed("connection timed out".into())]
        );
    }
}
