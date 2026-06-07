//! SSH key discovery and generation via `ssh-keygen`.
//!
//! Private key bodies are never read or displayed — we only ever pass their
//! *path* to ssh via `-i`. Listing uses `ssh-keygen -l -f <pubfile>`.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::binaries::{ssh_dir, tools};

#[derive(Debug, Clone)]
pub struct PublicKeyInfo {
    /// Path of the `.pub` file.
    pub path: PathBuf,
    pub bits: u32,
    pub fingerprint: String,
    pub comment: String,
    pub key_type: String,
    /// True if a sibling private key (same name without `.pub`) exists.
    pub has_private: bool,
}

impl PublicKeyInfo {
    /// Path of the private key (the `.pub` path with the extension removed).
    pub fn private_path(&self) -> PathBuf {
        let mut s = self.path.clone();
        s.set_extension("");
        s
    }

    /// Name shown in the UI (file stem of the private key).
    pub fn name(&self) -> String {
        self.private_path()
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

/// Read the public key text (safe to display / copy).
pub fn read_public_key(info: &PublicKeyInfo) -> io::Result<String> {
    std::fs::read_to_string(&info.path).map(|s| s.trim_end().to_string())
}

/// Enumerate `*.pub` files in `~/.ssh` and resolve their fingerprints.
pub fn list_public_keys() -> Vec<PublicKeyInfo> {
    let Some(dir) = ssh_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut keys = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pub") {
            continue;
        }
        let info = fingerprint_of(&path);
        keys.push(info);
    }
    keys.sort_by(|a, b| a.path.cmp(&b.path));
    keys
}

fn fingerprint_of(pubfile: &Path) -> PublicKeyInfo {
    let output = Command::new(&tools().ssh_keygen)
        .arg("-l")
        .arg("-f")
        .arg(pubfile)
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

    let mut priv_path = pubfile.to_path_buf();
    priv_path.set_extension("");

    PublicKeyInfo {
        path: pubfile.to_path_buf(),
        bits,
        fingerprint,
        comment,
        key_type,
        has_private: priv_path.is_file(),
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
}
