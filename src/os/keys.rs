//! SSH key discovery and generation via `ssh-keygen`.
//!
//! Private key bodies are never displayed — we only ever pass their *path* to
//! ssh via `-i`. The one exception is classification: to decide whether a file
//! is a private key we sniff only its first header line (`-----BEGIN ... PRIVATE
//! KEY-----`); the body is never read. Listing uses `ssh-keygen -l -f <file>`.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::binaries::{ssh_dir, tools};
use crate::config::model::HostView;
use crate::os::vault::{SecretKind, Vault};

/// How many directory levels to descend when discovering keys. The root
/// (`~/.ssh`) is level 0, so the deepest files found live `MAX_DEPTH - 1`
/// subdirectories down.
const MAX_DEPTH: usize = 8;

/// Whether a key's two halves were proven to belong to the same keypair.
///
/// We never load private key bodies to check this — instead each half is
/// fingerprinted independently (`ssh-keygen -l -f`) and the SHA256 public
/// fingerprints are compared. A matching fingerprint means the `.pub` really is
/// the public half of this private key, i.e. they sign/verify (and so
/// encrypt/decrypt) as a pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairStatus {
    /// Only one half exists (private-only or public-only) — nothing to match.
    NotApplicable,
    /// Both halves present and their public fingerprints agree.
    Matched,
    /// Both halves present but their fingerprints differ — the `.pub` does not
    /// belong to this private key.
    Mismatched,
    /// Both halves present but a fingerprint could not be read (e.g. a legacy
    /// encrypted PEM private key whose public half ssh-keygen won't surface
    /// without the passphrase), so the pairing can't be confirmed.
    Unverified,
}

#[derive(Debug, Clone)]
pub struct KeyInfo {
    /// Private key path (the base). The public key, when present, is at
    /// `path` + `.pub`. May not exist on disk when only a `.pub` is found.
    pub path: PathBuf,
    /// Path of the `.pub` file, when one exists.
    pub pub_path: Option<PathBuf>,
    pub bits: u32,
    pub fingerprint: String,
    pub comment: String,
    pub key_type: String,
    /// True if the private key file itself exists on disk.
    pub has_private: bool,
    /// Whether the private and public halves were verified to be the same
    /// keypair (only meaningful when both halves exist).
    pub pair: PairStatus,
}

impl KeyInfo {
    /// Path of the private key (the base path).
    pub fn private_path(&self) -> PathBuf {
        self.path.clone()
    }

    /// Name shown in the UI: the path relative to `~/.ssh` when the key lives
    /// in a subdirectory (so same-named keys in different folders stay
    /// distinct), else just the file name.
    pub fn name(&self) -> String {
        if let Some(dir) = ssh_dir()
            && let Ok(rel) = self.path.strip_prefix(&dir)
        {
            return rel.to_string_lossy().to_string();
        }
        self.path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    }
}

/// Parse one line of `ssh-keygen -l -f` output, e.g.
/// `256 SHA256:Rlb... user@host (ED25519)`.
pub fn parse_fingerprint_line(line: &str) -> Option<(u32, String, String, String)> {
    let line = line.trim();
    let mut it = line.splitn(3, ' ');
    let bits: u32 = it.next()?.parse().ok()?;
    let fingerprint = it.next()?.to_string();
    let rest = it.next().unwrap_or("");
    let (comment, key_type) = match rest.rfind('(') {
        Some(idx) => (
            rest[..idx].trim().to_string(),
            rest[idx..].trim_matches(['(', ')']).to_string(),
        ),
        None => (rest.trim().to_string(), String::new()),
    };
    Some((bits, fingerprint, comment, key_type))
}

/// Read the public key text (safe to display / copy). Errors when this entry
/// has no `.pub` file (e.g. a private-key-only key).
pub fn read_public_key(info: &KeyInfo) -> io::Result<String> {
    let Some(pub_path) = &info.pub_path else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no public key file for this key",
        ));
    };
    std::fs::read_to_string(pub_path).map(|s| s.trim_end().to_string())
}

/// Discover SSH keys under `~/.ssh`, recursing into subdirectories, and resolve
/// their fingerprints. Both halves of a pair and lone private/public keys are
/// reported.
pub fn list_keys() -> Vec<KeyInfo> {
    let Some(dir) = ssh_dir() else {
        return Vec::new();
    };
    list_keys_in(&dir)
}

/// Discovery against an explicit root (used by `list_keys` and tests).
fn list_keys_in(dir: &Path) -> Vec<KeyInfo> {
    let mut pubs = Vec::new();
    let mut privs = Vec::new();
    walk_keys(dir, 0, &mut pubs, &mut privs);

    // Group both halves under a common base path (the private key path).
    // The value carries the public file's real path (preserved from discovery,
    // never reconstructed) and whether a private key file exists.
    let mut bases: BTreeMap<PathBuf, (Option<PathBuf>, bool)> = BTreeMap::new();
    for p in pubs {
        let mut base = p.clone();
        base.set_extension(""); // strip ".pub"
        bases.entry(base).or_default().0 = Some(p);
    }
    for p in privs {
        bases.entry(p).or_default().1 = true;
    }

    // `bases` is a BTreeMap keyed by the base path, and each KeyInfo's `path`
    // is exactly that key, so iterating it already yields entries sorted by
    // path — no explicit sort needed.
    bases
        .into_iter()
        .map(|(base, (pub_path, has_priv))| fingerprint_of(base, pub_path, has_priv))
        .collect()
}

/// Recursively collect `.pub` files and private-key files under `dir`. Symlinked
/// directories are not followed (loop-safe); symlinked *files* are resolved and
/// classified. Recursion stops once `depth` reaches `MAX_DEPTH`.
fn walk_keys(dir: &Path, depth: usize, pubs: &mut Vec<PathBuf>, privs: &mut Vec<PathBuf>) {
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if ft.is_dir() {
            // Real directory only — `file_type()` never reports a symlinked dir
            // as a dir, so symlink loops cannot recurse here.
            walk_keys(&path, depth + 1, pubs, privs);
        } else if ft.is_file() {
            classify_file(path, pubs, privs);
        } else if ft.is_symlink() {
            // Resolve the link target: a file symlink is a common way to point
            // at a key in a vault. A symlinked dir is skipped to stay loop-safe.
            if let Ok(meta) = std::fs::metadata(&path)
                && meta.is_file()
            {
                classify_file(path, pubs, privs);
            }
        }
    }
}

/// Sort a regular file into the public or private bucket. A `.pub` file is
/// treated as public *only* when its header is not a private-key header — this
/// guards against a private key mis-named `*.pub` (which would otherwise leak
/// its body via `read_public_key` / copy).
fn classify_file(path: PathBuf, pubs: &mut Vec<PathBuf>, privs: &mut Vec<PathBuf>) {
    // Sniff once: a private-key header wins over the `.pub` extension.
    let is_priv = looks_like_private_key(&path);
    let is_pub_ext = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pub"));
    if is_pub_ext && !is_priv {
        pubs.push(path);
    } else if is_priv {
        privs.push(path);
    }
}

/// Classify a file as an SSH private key by sniffing only the first ~128 bytes
/// of its header (`-----BEGIN ... PRIVATE KEY-----`). The body is never read —
/// the bounded `take` also stops a newline-less blob from being slurped whole.
fn looks_like_private_key(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut first = String::new();
    if io::BufReader::new(file.take(128))
        .read_line(&mut first)
        .is_err()
    {
        return false;
    }
    let t = first.trim();
    t.starts_with("-----BEGIN") && t.contains("PRIVATE KEY-----")
}

/// Resolve a key's fingerprint metadata. Prefers the `.pub` file; falls back to
/// the private key (OpenSSH keys carry the public half in cleartext, so this
/// needs no passphrase). `stdin` is nulled so an older encrypted PEM that would
/// otherwise prompt fails instead of hanging the UI thread (note: `ssh-keygen`
/// may still try the console directly for legacy encrypted PEM, in which case it
/// simply fails and the fields stay empty — never a hang here).
fn fingerprint_of(base: PathBuf, pub_path: Option<PathBuf>, has_priv: bool) -> KeyInfo {
    // Display metadata comes from the public file when present (it carries the
    // comment), else from the private key.
    let fp_source = pub_path.clone().unwrap_or_else(|| base.clone());
    let (bits, fingerprint, comment, key_type) = read_fingerprint(&fp_source).unwrap_or_default();

    // Verify the two halves are genuinely the same keypair. Only meaningful when
    // both exist. We must derive the public key from the private key's *secret
    // material* (`ssh-keygen -y`) — comparing fingerprints would be circular,
    // because `ssh-keygen -l -f <priv>` just reads the sibling `.pub` and so
    // never disagrees with it.
    let pair = match (&pub_path, has_priv) {
        (Some(pub_path), true) => {
            let derived = derive_public_key(&base);
            let stored = read_pub_body(pub_path);
            classify_pair(derived.as_deref(), stored.as_deref())
        }
        _ => PairStatus::NotApplicable,
    };

    KeyInfo {
        path: base,
        pub_path,
        bits,
        fingerprint,
        comment,
        key_type,
        has_private: has_priv,
        pair,
    }
}

/// Run `ssh-keygen -l -f <path>` and parse its first line. `None` on spawn
/// failure, a non-zero exit, or an unparseable line. `stdin` is nulled so an
/// encrypted PEM that would otherwise prompt fails fast instead of hanging.
fn read_fingerprint(path: &Path) -> Option<(u32, String, String, String)> {
    let output = Command::new(&tools().ssh_keygen)
        .arg("-l")
        .arg("-f")
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_fingerprint_line(text.lines().next().unwrap_or(""))
}

/// Derive the public key from a private key via `ssh-keygen -y -f`. Unlike
/// `-l -f`, this reads the private key's *actual* secret material (not a sibling
/// `.pub`), so it is the source of truth for which public key the private key
/// corresponds to. Returns the normalized `<algo> <blob>` body, or `None` on
/// failure — e.g. an encrypted key whose passphrase we don't have.
///
/// `-P ""` is essential: `-y` on an *encrypted* key prompts for the passphrase
/// straight on the console, which nulling `stdin` does **not** suppress, so it
/// would hang the UI thread. Supplying an (empty, wrong) passphrase makes it
/// fail fast instead; unencrypted keys ignore it and derive normally.
fn derive_public_key(priv_path: &Path) -> Option<String> {
    let output = Command::new(&tools().ssh_keygen)
        .arg("-y")
        .arg("-P")
        .arg("")
        .arg("-f")
        .arg(priv_path)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    pub_body(text.lines().next().unwrap_or(""))
}

/// Read a `.pub` file and return its normalized `<algo> <blob>` body.
fn read_pub_body(pub_path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(pub_path).ok()?;
    pub_body(text.lines().next().unwrap_or(""))
}

/// Normalize a public-key line to its identity: `<algorithm> <base64-blob>`,
/// dropping any trailing comment (which differs freely between a stored `.pub`
/// and a freshly derived key). `None` if the line lacks the two required fields.
fn pub_body(line: &str) -> Option<String> {
    let mut it = line.split_whitespace();
    let algo = it.next()?;
    let blob = it.next()?;
    Some(format!("{algo} {blob}"))
}

/// Decide a [`PairStatus`] from the private key's derived public-key body and
/// the stored `.pub` body. A readable, non-empty body on both sides lets us
/// compare; anything missing means we couldn't confirm and report
/// [`PairStatus::Unverified`] rather than a false mismatch. Pure (no I/O) so the
/// matching rule is unit-testable.
fn classify_pair(derived: Option<&str>, stored: Option<&str>) -> PairStatus {
    match (derived, stored) {
        (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() => {
            if a == b {
                PairStatus::Matched
            } else {
                PairStatus::Mismatched
            }
        }
        _ => PairStatus::Unverified,
    }
}

/// Key type choices offered by the generate wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyType {
    Ed25519,
    Rsa4096,
}

/// Generate a new key non-interactively with an empty passphrase. Refuses to
/// clobber an existing file (ssh-keygen would otherwise prompt and hang).
pub fn generate_key(key_type: KeyType, out_path: &Path, comment: &str) -> io::Result<()> {
    if out_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", out_path.display()),
        ));
    }
    if let Some(parent) = out_path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)?;
    }

    let out = Command::new(&tools().ssh_keygen)
        .args(generate_key_args(
            key_type,
            out_path,
            comment,
            GenPassphrase::NoPassphrase,
        ))
        .output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "ssh-keygen failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

/// Passphrase mode for key generation (Issue #47).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenPassphrase {
    NoPassphrase,
    Interactive,
}

/// Pure arg builder for `ssh-keygen -p` (passphrase add/change). See CLAUDE.md layering.
/// The current/new passphrases are prompted by ssh-keygen itself — never passed
/// as arguments, so no secret ever appears on a command line.
pub fn change_passphrase_args(private_key: &Path) -> Vec<OsString> {
    vec!["-p".into(), "-f".into(), private_key.as_os_str().to_owned()]
}

/// Pure arg builder for key generation; `generate_key` and the interactive path share it.
/// `NoPassphrase` keeps the captured non-interactive shape (`-N "" -q`);
/// `Interactive` omits both so ssh-keygen prompts on the inherited console.
pub fn generate_key_args(
    key_type: KeyType,
    out_path: &Path,
    comment: &str,
    passphrase: GenPassphrase,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = match key_type {
        KeyType::Ed25519 => vec!["-t".into(), "ed25519".into()],
        KeyType::Rsa4096 => vec!["-t".into(), "rsa".into(), "-b".into(), "4096".into()],
    };
    args.extend([
        "-f".into(),
        out_path.as_os_str().to_owned(),
        "-C".into(),
        comment.into(),
    ]);
    if passphrase == GenPassphrase::NoPassphrase {
        args.extend(["-N".into(), "".into(), "-q".into()]);
    }
    args
}

/// Identity files OpenSSH tries when a host declares no `IdentityFile` of its
/// own. A host on the defaults still auto-fills its passphrase at connect time,
/// so its stored secret goes stale like any other — leaving them out would miss
/// the most common config of all (`Host x` with just a `HostName`).
const DEFAULT_IDENTITY_FILES: &[&str] = &[
    "id_rsa",
    "id_ecdsa",
    "id_ecdsa_sk",
    "id_ed25519",
    "id_ed25519_sk",
    "id_dsa",
];

/// Expand a leading `~/` against `home`; anything else is taken verbatim.
fn expand_tilde(raw: &str, home: &Path) -> PathBuf {
    match raw.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(raw),
    }
}

/// Whether two key paths name the same file. Windows folds case and treats
/// `/`≡`\`, so a hand-written lower-case `c:\users\…` config entry really does
/// reach the same key — the connect-time auto-fill already compares this way
/// (`askpass::paths_equal`), and a stricter rule here would detect nothing while
/// the stale secret kept being served.
fn same_key_path(a: &Path, b: &Path) -> bool {
    fn norm(p: &Path) -> String {
        let s = p.to_string_lossy();
        if cfg!(windows) {
            s.replace('\\', "/").to_ascii_lowercase()
        } else {
            s.into_owned()
        }
    }
    norm(a) == norm(b)
}

/// The vault lookup keys of every host that uses `key_path` — i.e. each non-glob
/// pattern of a matching `Host` line, not just its first (alias). This mirrors
/// how the connect-time auto-fill resolves secrets (`vault::match_vault_kinds`
/// and `update::gather_secrets` both scan all patterns), so a secret registered
/// under a secondary name is detected here exactly when it would be served
/// there. Glob patterns are skipped: they can never equal a vault entry's host.
/// Pure.
pub fn hosts_using_key(key_path: &Path, home: &Path, hosts: &[HostView]) -> Vec<String> {
    let uses_key = |h: &HostView| {
        if h.identity_files.is_empty() {
            // No IdentityFile declared — OpenSSH falls back to the defaults in
            // ~/.ssh, so this host uses `key_path` iff it is one of them.
            let ssh_dir = home.join(".ssh");
            return DEFAULT_IDENTITY_FILES
                .iter()
                .any(|name| same_key_path(&ssh_dir.join(name), key_path));
        }
        h.identity_files
            .iter()
            .any(|id| same_key_path(&expand_tilde(id, home), key_path))
    };
    hosts
        .iter()
        .filter(|h| uses_key(h))
        .flat_map(|h| h.patterns.iter())
        .filter(|pat| !pat.contains(['*', '?', '!']))
        .map(|pat| pat.to_string())
        .collect()
}

/// `hosts_using_key` ∩ the lookup keys that actually have a vault `Passphrase`
/// entry — the hosts whose stored secret just went stale. Pure.
pub fn stale_passphrase_hosts(
    key_path: &Path,
    home: &Path,
    hosts: &[HostView],
    vault: &Vault,
) -> Vec<String> {
    hosts_using_key(key_path, home, hosts)
        .into_iter()
        .filter(|host| {
            vault
                .entries
                .iter()
                .any(|e| e.kind == SecretKind::Passphrase && e.host == *host)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ed25519_with_comment() {
        let line = "256 SHA256:Rlbabc123 user@host.example (ED25519)";
        let (bits, fp, comment, ktype) = parse_fingerprint_line(line).unwrap();
        assert_eq!(bits, 256);
        assert_eq!(fp, "SHA256:Rlbabc123");
        assert_eq!(comment, "user@host.example");
        assert_eq!(ktype, "ED25519");
    }

    #[test]
    fn parse_comment_with_spaces() {
        let line = "3072 SHA256:abc my laptop key (RSA)";
        let (bits, _fp, comment, ktype) = parse_fingerprint_line(line).unwrap();
        assert_eq!(bits, 3072);
        assert_eq!(comment, "my laptop key");
        assert_eq!(ktype, "RSA");
    }

    #[test]
    fn parse_no_comment() {
        let line = "256 SHA256:xyz  (ED25519)";
        let (_b, _f, comment, ktype) = parse_fingerprint_line(line).unwrap();
        assert_eq!(comment, "");
        assert_eq!(ktype, "ED25519");
    }

    #[test]
    fn classify_pair_rules() {
        // Same fingerprint on both halves -> a confirmed pair.
        assert_eq!(
            classify_pair(Some("SHA256:abc"), Some("SHA256:abc")),
            PairStatus::Matched
        );
        // Different fingerprints -> the `.pub` is not this key's pair.
        assert_eq!(
            classify_pair(Some("SHA256:abc"), Some("SHA256:zzz")),
            PairStatus::Mismatched
        );
        // A missing or blank half -> unverifiable, never a false mismatch.
        assert_eq!(
            classify_pair(Some("SHA256:abc"), None),
            PairStatus::Unverified
        );
        assert_eq!(
            classify_pair(None, Some("SHA256:abc")),
            PairStatus::Unverified
        );
        assert_eq!(classify_pair(Some(""), Some("")), PairStatus::Unverified);
        assert_eq!(
            classify_pair(Some("SHA256:abc"), Some("")),
            PairStatus::Unverified
        );
    }

    #[test]
    fn pub_body_drops_comment_and_keeps_algo_blob() {
        // The comment must be ignored so a derived key (often comment-less)
        // still matches the stored `.pub` of the same key.
        assert_eq!(
            pub_body("ssh-ed25519 AAAAC3Nz user@host"),
            Some("ssh-ed25519 AAAAC3Nz".to_string())
        );
        assert_eq!(
            pub_body("ssh-ed25519 AAAAC3Nz"),
            Some("ssh-ed25519 AAAAC3Nz".to_string())
        );
        // A comment-only difference must compare equal.
        assert_eq!(
            pub_body("ssh-rsa AAAAB3 a@a"),
            pub_body("ssh-rsa AAAAB3 b@b")
        );
        // Malformed lines (no blob) yield None.
        assert_eq!(pub_body("ssh-ed25519"), None);
        assert_eq!(pub_body(""), None);
    }

    use std::fs;

    /// Unique throwaway dir per test (label keeps parallel tests from colliding).
    fn tmp_dir(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("sshm-keytest-{}-{label}", std::process::id()));
        fs::remove_dir_all(&p).ok();
        fs::create_dir_all(&p).unwrap();
        p
    }

    const OPENSSH_HEADER: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\n";

    #[test]
    fn detects_private_key_headers() {
        let dir = tmp_dir("headers");
        let openssh = dir.join("id_ed25519");
        fs::write(
            &openssh,
            "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----\n",
        )
        .unwrap();
        let rsa = dir.join("id_rsa");
        fs::write(&rsa, "-----BEGIN RSA PRIVATE KEY-----\nxyz\n").unwrap();
        let cfg = dir.join("config");
        fs::write(&cfg, "Host example\n  User me\n").unwrap();
        let kh = dir.join("known_hosts");
        fs::write(&kh, "github.com ssh-ed25519 AAAA...\n").unwrap();
        // A newline-less blob must not be slurped whole, and must not match.
        let blob = dir.join("blob.bin");
        fs::write(&blob, "x".repeat(1_000_000)).unwrap();

        assert!(looks_like_private_key(&openssh));
        assert!(looks_like_private_key(&rsa));
        assert!(!looks_like_private_key(&cfg));
        assert!(!looks_like_private_key(&kh));
        assert!(!looks_like_private_key(&blob));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn walk_groups_pairs_and_lone_keys_recursively() {
        let dir = tmp_dir("walk");
        let sub = dir.join("work");
        fs::create_dir_all(&sub).unwrap();

        // Top-level pair.
        fs::write(dir.join("id_ed25519"), OPENSSH_HEADER).unwrap();
        fs::write(dir.join("id_ed25519.pub"), "ssh-ed25519 AAAA top\n").unwrap();
        // Lone public key (orphan .pub) at top level.
        fs::write(dir.join("orphan.pub"), "ssh-ed25519 AAAA orphan\n").unwrap();
        // Lone private key in a subdirectory.
        fs::write(sub.join("only_priv"), OPENSSH_HEADER).unwrap();
        // A non-key file that must be ignored.
        fs::write(dir.join("config"), "Host x\n").unwrap();
        // A private key mis-named `*.pub` must be classed private (not public),
        // else its body could leak via the copy-public-key path.
        fs::write(sub.join("trap.pub"), OPENSSH_HEADER).unwrap();

        let mut pubs = Vec::new();
        let mut privs = Vec::new();
        walk_keys(&dir, 0, &mut pubs, &mut privs);

        assert_eq!(pubs.len(), 2, "id_ed25519.pub + orphan.pub");
        assert_eq!(
            privs.len(),
            3,
            "top id_ed25519 + work/{{only_priv,trap.pub}}"
        );
        assert!(privs.iter().any(|p| p.ends_with("work/only_priv")));
        assert!(privs.iter().any(|p| p.ends_with("work/trap.pub")));
        assert!(!pubs.iter().any(|p| p.ends_with("work/trap.pub")));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_keys_in_projects_pairs_and_lone_keys() {
        let dir = tmp_dir("project");
        let sub = dir.join("work");
        fs::create_dir_all(&sub).unwrap();
        // Matched pair.
        fs::write(dir.join("id_ed25519"), OPENSSH_HEADER).unwrap();
        fs::write(dir.join("id_ed25519.pub"), "ssh-ed25519 AAAA top\n").unwrap();
        // Lone private key in a subdir.
        fs::write(sub.join("only_priv"), OPENSSH_HEADER).unwrap();
        // Orphan public key.
        fs::write(dir.join("orphan.pub"), "ssh-ed25519 AAAA orphan\n").unwrap();

        let keys = list_keys_in(&dir);
        assert_eq!(keys.len(), 3);

        let pair = keys
            .iter()
            .find(|k| k.path.ends_with("id_ed25519"))
            .unwrap();
        assert!(pair.has_private);
        assert_eq!(
            pair.pub_path.as_deref(),
            Some(dir.join("id_ed25519.pub").as_path())
        );

        let lone = keys
            .iter()
            .find(|k| k.path.ends_with("work/only_priv"))
            .unwrap();
        assert!(lone.has_private);
        assert!(lone.pub_path.is_none());

        let orphan = keys.iter().find(|k| k.path.ends_with("orphan")).unwrap();
        assert!(!orphan.has_private);
        assert_eq!(
            orphan.pub_path.as_deref(),
            Some(dir.join("orphan.pub").as_path())
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn walk_respects_max_depth() {
        let dir = tmp_dir("depth");
        // Build d0/d1/.../d(MAX_DEPTH-1). The deepest scannable directory is
        // d(MAX_DEPTH-2) (visited at recursion depth MAX_DEPTH-1); the next one
        // down, d(MAX_DEPTH-1), is entered at depth MAX_DEPTH and returns before
        // listing. Placing a key on each side tightly pins the boundary.
        let mut path = dir.clone();
        let mut at_limit = dir.clone();
        for i in 0..MAX_DEPTH {
            path = path.join(format!("d{i}"));
            if i == MAX_DEPTH - 2 {
                at_limit = path.clone();
            }
        }
        fs::create_dir_all(&path).unwrap(); // through d(MAX_DEPTH-1)
        fs::write(at_limit.join("at_limit"), OPENSSH_HEADER).unwrap();
        fs::write(path.join("too_deep"), OPENSSH_HEADER).unwrap();

        let mut pubs = Vec::new();
        let mut privs = Vec::new();
        walk_keys(&dir, 0, &mut pubs, &mut privs);

        assert!(
            privs.iter().any(|p| p.ends_with("at_limit")),
            "key at the deepest scannable level must be discovered"
        );
        assert!(
            !privs.iter().any(|p| p.ends_with("too_deep")),
            "key one level past the cap must not be discovered"
        );

        fs::remove_dir_all(&dir).ok();
    }

    // --- Issue #47: passphrase add/change (`ssh-keygen -p`) + vault pairing ---

    use crate::config::model::HostView;
    use crate::os::vault::{SecretKind, Vault, VaultEntry};
    use std::ffi::OsString;

    fn os_args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn host_view(alias: &str, identity_files: &[&str]) -> HostView {
        host_view_multi(&[alias], identity_files)
    }

    /// A `Host` line with several patterns (`Host prod prod-old`), which the
    /// connect-time auto-fill treats as several vault lookup keys.
    fn host_view_multi(patterns: &[&str], identity_files: &[&str]) -> HostView {
        HostView {
            patterns: patterns.iter().map(|s| s.to_string()).collect(),
            identity_files: identity_files.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    /// change_passphrase_args — 秘密鍵パスから `-p -f <path>` を組み立てる
    #[test]
    fn change_passphrase_args_builds_p_f_path() {
        // given
        let key = Path::new("/home/u/.ssh/id_ed25519");

        // when
        let args = change_passphrase_args(key);

        // then
        assert_eq!(args, os_args(&["-p", "-f", "/home/u/.ssh/id_ed25519"]));
    }

    /// generate_key_args — NoPassphrase の ed25519 は `-N ""` と `-q` を含む
    #[test]
    fn generate_key_args_no_passphrase_ed25519_includes_n_empty_and_q() {
        // given
        let out = Path::new("/tmp/k/id_ed25519");

        // when
        let args = generate_key_args(
            KeyType::Ed25519,
            out,
            "me@host",
            GenPassphrase::NoPassphrase,
        );

        // then
        assert_eq!(
            args,
            os_args(&[
                "-t",
                "ed25519",
                "-f",
                "/tmp/k/id_ed25519",
                "-C",
                "me@host",
                "-N",
                "",
                "-q",
            ])
        );
    }

    /// generate_key_args — NoPassphrase の rsa4096 は `-b 4096` も含む
    #[test]
    fn generate_key_args_no_passphrase_rsa4096_includes_b_4096() {
        // given
        let out = Path::new("/tmp/k/id_rsa");

        // when
        let args = generate_key_args(
            KeyType::Rsa4096,
            out,
            "me@host",
            GenPassphrase::NoPassphrase,
        );

        // then
        assert_eq!(
            args,
            os_args(&[
                "-t",
                "rsa",
                "-b",
                "4096",
                "-f",
                "/tmp/k/id_rsa",
                "-C",
                "me@host",
                "-N",
                "",
                "-q",
            ])
        );
    }

    /// generate_key_args — Interactive では `-N`/`-q` を発行しない（ssh-keygen に対話させる）
    #[test]
    fn generate_key_args_interactive_omits_n_and_q() {
        // given
        let out = Path::new("/tmp/k/id_ed25519");

        // when
        let args = generate_key_args(KeyType::Ed25519, out, "me@host", GenPassphrase::Interactive);

        // then
        assert!(
            !args.contains(&OsString::from("-N")),
            "must omit -N: {args:?}"
        );
        assert!(
            !args.contains(&OsString::from("-q")),
            "must omit -q: {args:?}"
        );
        assert!(args.contains(&OsString::from("-t")));
        assert!(args.contains(&OsString::from("-f")));
        assert!(args.contains(&OsString::from("-C")));
    }

    /// hosts_using_key — `~/` 展開と絶対パスの IdentityFile が key_path に一致する
    #[test]
    fn hosts_using_key_matches_tilde_and_absolute_identity() {
        // given: (alias, identity_files, マッチ期待)
        let home = Path::new("/home/u");
        let key = Path::new("/home/u/.ssh/id_a");
        let cases: &[(&str, &[&str], bool)] = &[
            ("tilde", &["~/.ssh/id_a"], true),
            ("absolute", &["/home/u/.ssh/id_a"], true),
            ("other-key", &["~/.ssh/id_b"], false),
            ("no-identity", &[], false),
        ];
        let hosts: Vec<HostView> = cases
            .iter()
            .map(|(alias, ids, _)| host_view(alias, ids))
            .collect();

        // when
        let matched = hosts_using_key(key, home, &hosts);

        // then
        for (alias, _, expect) in cases {
            assert_eq!(
                matched.iter().any(|m| m == alias),
                *expect,
                "alias {alias} (matched: {matched:?})"
            );
        }
    }

    /// hosts_using_key — `Host` 行の非 glob パターンを全て返す（接続時オートフィルと同じ照合鍵）
    #[test]
    fn hosts_using_key_matches_every_non_glob_pattern() {
        // given: 1 つの Host 行が別名を複数持ち、glob パターンも混ざる
        let home = Path::new("/home/u");
        let key = Path::new("/home/u/.ssh/id_a");
        let hosts = vec![host_view_multi(
            &["prod", "prod-old", "*.internal"],
            &["~/.ssh/id_a"],
        )];

        // when
        let matched = hosts_using_key(key, home, &hosts);

        // then: 先頭パターンだけでなく副パターンも返る。glob は vault エントリに
        // 一致しえないので候補にしない（gather_secrets と同じ規則）。
        assert_eq!(
            matched,
            vec!["prod".to_string(), "prod-old".to_string()],
            "every non-glob pattern must be a lookup key"
        );
    }

    /// hosts_using_key — IdentityFile 未宣言のホストは OpenSSH の既定 identity を暗黙候補にする
    #[test]
    fn hosts_using_key_falls_back_to_default_identities() {
        // given: IdentityFile 行が無い（＝既定 identity を使う典型構成）
        let home = Path::new("/home/u");
        let hosts = vec![host_view("prod", &[])];

        // when / then: 既定 identity の 1 つならマッチする
        assert_eq!(
            hosts_using_key(Path::new("/home/u/.ssh/id_ed25519"), home, &hosts),
            vec!["prod".to_string()]
        );
        // when / then: 既定でない鍵にはマッチしない
        assert!(
            hosts_using_key(Path::new("/home/u/.ssh/deploy_key"), home, &hosts).is_empty(),
            "a non-default key must not match a host that declares no IdentityFile"
        );
        // when / then: IdentityFile を明示したホストには既定を足さない
        let explicit = vec![host_view("prod", &["~/.ssh/deploy_key"])];
        assert!(
            hosts_using_key(Path::new("/home/u/.ssh/id_ed25519"), home, &explicit).is_empty(),
            "an explicit IdentityFile replaces the defaults"
        );
    }

    /// hosts_using_key — Windows ではパスの大小・区切りを畳んで比較する（unix は厳密）
    #[test]
    fn hosts_using_key_folds_case_and_separators_on_windows() {
        // given: 手書き config によくある全小文字・バックスラッシュ表記
        let home = Path::new("C:/Users/taro");
        let key = Path::new("C:/Users/taro/.ssh/id_a");
        let hosts = vec![host_view("prod", &["c:\\users\\taro\\.ssh\\id_a"])];

        // when
        let matched = hosts_using_key(key, home, &hosts);

        // then: Windows は case-insensitive かつ `/`≡`\` なので接続でき、
        // 検出もそれに揃える。unix ではこれらは別パスなので一致しない。
        if cfg!(windows) {
            assert_eq!(matched, vec!["prod".to_string()]);
        } else {
            assert!(matched.is_empty(), "unix paths are compared exactly");
        }
    }

    /// stale_passphrase_hosts — 副パターンで登録された vault エントリも陳腐化として拾う
    #[test]
    fn stale_passphrase_hosts_covers_secondary_patterns() {
        // given: vault のエントリは Host 行の 2 つ目のパターン名で登録されている
        let home = Path::new("/home/u");
        let key = Path::new("/home/u/.ssh/id_a");
        let hosts = vec![host_view_multi(&["prod", "prod-old"], &["~/.ssh/id_a"])];
        let mut vault = Vault::create("pw").unwrap();
        vault.upsert(
            None,
            VaultEntry {
                host: "prod-old".into(),
                kind: SecretKind::Passphrase,
                secret: "s".into(),
                note: String::new(),
            },
        );

        // when
        let stale = stale_passphrase_hosts(key, home, &hosts, &vault);

        // then: 接続時は prod-old のエントリが使われるので、検出も拾わねばならない
        assert_eq!(stale, vec!["prod-old".to_string()]);
    }

    /// stale_passphrase_hosts — 鍵一致かつ vault に Passphrase がある host だけを返す
    #[test]
    fn stale_passphrase_hosts_requires_both_key_match_and_vault_entry() {
        // given
        let home = Path::new("/home/u");
        let key = Path::new("/home/u/.ssh/id_a");
        let hosts = vec![
            host_view("with-pass", &["~/.ssh/id_a"]), // 鍵一致 + Passphrase → 含む
            host_view("pw-only", &["~/.ssh/id_a"]),   // 鍵一致 + Password のみ → 含まない
            host_view("other-key", &["~/.ssh/id_b"]), // 鍵不一致 + Passphrase → 含まない
        ];
        let entry = |host: &str, kind| VaultEntry {
            host: host.into(),
            kind,
            secret: "s".into(),
            note: String::new(),
        };
        let mut vault = Vault::create("pw").unwrap();
        vault.upsert(None, entry("with-pass", SecretKind::Passphrase));
        vault.upsert(None, entry("pw-only", SecretKind::Password));
        vault.upsert(None, entry("other-key", SecretKind::Passphrase));

        // when
        let stale = stale_passphrase_hosts(key, home, &hosts, &vault);

        // then
        assert_eq!(stale, vec!["with-pass".to_string()]);

        // given: 何も無い
        let empty_vault = Vault::create("pw").unwrap();
        // when
        let stale = stale_passphrase_hosts(key, home, &[], &empty_vault);
        // then
        assert!(stale.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn walk_resolves_symlinked_key_file() {
        let dir = tmp_dir("symlink");
        let target = dir.join("real_key");
        fs::write(&target, OPENSSH_HEADER).unwrap();
        let link = dir.join("linked_key");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let mut pubs = Vec::new();
        let mut privs = Vec::new();
        walk_keys(&dir, 0, &mut pubs, &mut privs);

        assert!(privs.iter().any(|p| p.ends_with("real_key")));
        assert!(
            privs.iter().any(|p| p.ends_with("linked_key")),
            "a symlinked private key file should be discovered"
        );

        fs::remove_dir_all(&dir).ok();
    }
}
