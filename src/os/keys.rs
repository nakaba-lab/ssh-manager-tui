//! SSH key discovery and generation via `ssh-keygen`.
//!
//! Private key bodies are never displayed — we only ever pass their *path* to
//! ssh via `-i`. The one exception is classification: to decide whether a file
//! is a private key we sniff only its first header line (`-----BEGIN ... PRIVATE
//! KEY-----`); the body is never read. Listing uses `ssh-keygen -l -f <file>`.

use std::collections::BTreeMap;
use std::io::{self, BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::binaries::{ssh_dir, tools};

/// How many directory levels to descend when discovering keys. The root
/// (`~/.ssh`) is level 0, so the deepest files found live `MAX_DEPTH - 1`
/// subdirectories down.
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

    let mut keys: Vec<KeyInfo> = bases
        .into_iter()
        .map(|(base, (pub_path, has_priv))| fingerprint_of(base, pub_path, has_priv))
        .collect();
    keys.sort_by(|a, b| a.path.cmp(&b.path));
    keys
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

    KeyInfo {
        path: base,
        pub_path,
        bits,
        fingerprint,
        comment,
        key_type,
        has_private: has_priv,
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
