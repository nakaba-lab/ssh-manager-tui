//! SSH key discovery and generation via `ssh-keygen`.
//!
//! Private key bodies are never displayed — we only ever pass their *path* to
//! ssh via `-i`. The one exception is classification: to decide whether a file
//! is a private key we sniff only its first header line (`-----BEGIN ... PRIVATE
//! KEY-----`); the body is never read. Listing uses `ssh-keygen -l -f <file>`.

use std::collections::BTreeMap;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::binaries::{ssh_dir, tools};

/// How deep below `~/.ssh` to recurse when discovering keys.
const MAX_DEPTH: usize = 8;

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
    let mut bases: BTreeMap<PathBuf, (bool, bool)> = BTreeMap::new();
    for p in pubs {
        let mut base = p.clone();
        base.set_extension(""); // strip ".pub"
        bases.entry(base).or_default().0 = true;
    }
    for p in privs {
        bases.entry(p).or_default().1 = true;
    }

    let mut keys: Vec<KeyInfo> = bases
        .into_iter()
        .map(|(base, (has_pub, has_priv))| {
            let pub_path = has_pub.then(|| {
                let mut p = base.clone();
                let ext = match p.extension() {
                    Some(e) => format!("{}.pub", e.to_string_lossy()),
                    None => "pub".to_string(),
                };
                p.set_extension(ext);
                p
            });
            fingerprint_of(base, pub_path, has_priv)
        })
        .collect();
    keys.sort_by(|a, b| a.path.cmp(&b.path));
    keys
}

/// Recursively collect `.pub` files and private-key files under `dir`.
/// Symlinked directories are not followed (loop-safe); depth is capped.
fn walk_keys(dir: &Path, depth: usize, pubs: &mut Vec<PathBuf>, privs: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH {
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
        // `file_type()` does not follow symlinks: a symlinked dir reports
        // neither is_dir nor is_file, so it is skipped here.
        if ft.is_dir() {
            walk_keys(&path, depth + 1, pubs, privs);
        } else if ft.is_file() {
            if path.extension().and_then(|e| e.to_str()) == Some("pub") {
                pubs.push(path);
            } else if looks_like_private_key(&path) {
                privs.push(path);
            }
        }
    }
}

/// Classify a file as an SSH private key by sniffing only its first header
/// line (`-----BEGIN ... PRIVATE KEY-----`). The key body is never read.
fn looks_like_private_key(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut first = String::new();
    if io::BufReader::new(file).read_line(&mut first).is_err() {
        return false;
    }
    let t = first.trim();
    t.starts_with("-----BEGIN") && t.contains("PRIVATE KEY-----")
}

/// Resolve a key's fingerprint metadata. Prefers the `.pub` file; falls back to
/// the private key (OpenSSH keys carry the public half in cleartext, so this
/// needs no passphrase). `stdin` is nulled so an older encrypted PEM that would
/// otherwise prompt fails instead of hanging the UI thread.
fn fingerprint_of(base: PathBuf, pub_path: Option<PathBuf>, has_priv: bool) -> KeyInfo {
    let fp_source = pub_path.clone().unwrap_or_else(|| base.clone());
    let output = Command::new(&tools().ssh_keygen)
        .arg("-l")
        .arg("-f")
        .arg(&fp_source)
        .stdin(Stdio::null())
        .output();

    let (bits, fingerprint, comment, key_type) = match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            parse_fingerprint_line(text.lines().next().unwrap_or("")).unwrap_or((
                0,
                String::new(),
                String::new(),
                String::new(),
            ))
        }
        _ => (0, String::new(), String::new(), String::new()),
    };

    let has_private = has_priv || base.is_file();

    KeyInfo {
        path: base,
        pub_path,
        bits,
        fingerprint,
        comment,
        key_type,
        has_private,
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

    let mut cmd = Command::new(&tools().ssh_keygen);
    cmd.arg("-t");
    match key_type {
        KeyType::Ed25519 => {
            cmd.arg("ed25519");
        }
        KeyType::Rsa4096 => {
            cmd.arg("rsa").arg("-b").arg("4096");
        }
    }
    cmd.arg("-f").arg(out_path);
    cmd.arg("-C").arg(comment);
    cmd.arg("-N").arg("");
    cmd.arg("-q");

    let out = cmd.output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "ssh-keygen failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
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

    use std::fs;

    fn tmp_dir() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "sshm-keytest-{}-{:?}",
            std::process::id(),
            now_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn now_nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    #[test]
    fn detects_private_key_headers() {
        let dir = tmp_dir();
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

        assert!(looks_like_private_key(&openssh));
        assert!(looks_like_private_key(&rsa));
        assert!(!looks_like_private_key(&cfg));
        assert!(!looks_like_private_key(&kh));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn walk_groups_pairs_and_lone_keys_recursively() {
        let dir = tmp_dir();
        let sub = dir.join("work");
        fs::create_dir_all(&sub).unwrap();

        // Top-level pair.
        fs::write(
            dir.join("id_ed25519"),
            "-----BEGIN OPENSSH PRIVATE KEY-----\n",
        )
        .unwrap();
        fs::write(dir.join("id_ed25519.pub"), "ssh-ed25519 AAAA top\n").unwrap();
        // Lone public key (orphan .pub) at top level.
        fs::write(dir.join("orphan.pub"), "ssh-ed25519 AAAA orphan\n").unwrap();
        // Lone private key in a subdirectory.
        fs::write(
            sub.join("only_priv"),
            "-----BEGIN OPENSSH PRIVATE KEY-----\n",
        )
        .unwrap();
        // A non-key file that must be ignored.
        fs::write(dir.join("config"), "Host x\n").unwrap();

        let mut pubs = Vec::new();
        let mut privs = Vec::new();
        walk_keys(&dir, 0, &mut pubs, &mut privs);

        assert_eq!(pubs.len(), 2, "id_ed25519.pub + orphan.pub");
        assert_eq!(privs.len(), 2, "top id_ed25519 + work/only_priv");
        assert!(privs.iter().any(|p| p.ends_with("work/only_priv")));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_keys_in_reports_lone_private_and_orphan_public() {
        let dir = tmp_dir();
        let sub = dir.join("work");
        fs::create_dir_all(&sub).unwrap();
        fs::write(
            sub.join("only_priv"),
            "-----BEGIN OPENSSH PRIVATE KEY-----\n",
        )
        .unwrap();
        fs::write(dir.join("orphan.pub"), "ssh-ed25519 AAAA orphan\n").unwrap();

        let keys = list_keys_in(&dir);
        assert_eq!(keys.len(), 2);

        let lone = keys
            .iter()
            .find(|k| k.path.ends_with("work/only_priv"))
            .unwrap();
        assert!(lone.has_private);
        assert!(lone.pub_path.is_none());

        let orphan = keys.iter().find(|k| k.path.ends_with("orphan")).unwrap();
        assert!(!orphan.has_private);
        assert!(orphan.pub_path.is_some());
        assert_eq!(
            orphan.pub_path.as_deref(),
            Some(dir.join("orphan.pub").as_path())
        );

        fs::remove_dir_all(&dir).ok();
    }
}
