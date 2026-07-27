//! Reading and removing `~/.ssh/known_hosts` entries.
//!
//! Removal rewrites the file without the selected line (with a `.old` backup),
//! which works for hashed entries too — unlike `ssh-keygen -R`, which needs the
//! plaintext hostname we don't have for hashed lines.

use std::io;
use std::path::{Path, PathBuf};

use super::binaries::ssh_dir;

/// The host field of a known_hosts entry.
#[derive(Debug, Clone)]
pub enum HostSpec {
    Plain(String),
    /// Hashed (`|1|...`) host — the digest is retained but displayed as `(hashed)`.
    Hashed(#[allow(dead_code)] String),
}

impl HostSpec {
    pub fn display(&self) -> String {
        match self {
            HostSpec::Plain(s) => s.clone(),
            HostSpec::Hashed(_) => "(hashed)".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct KnownHostEntry {
    /// 0-based index of this line within the file (entries only; comments and
    /// blanks are skipped but counted so removal targets the right line).
    pub line_no: usize,
    pub marker: Option<String>,
    pub host: HostSpec,
    pub key_type: String,
    /// Base-64 key body (retained for a future detail view).
    #[allow(dead_code)]
    pub key_b64: String,
    /// The verbatim line, used to content-address removal.
    pub raw: String,
}

pub fn known_hosts_path() -> Option<PathBuf> {
    ssh_dir().map(|d| d.join("known_hosts"))
}

/// Parse one known_hosts line. Returns `None` for blanks/comments/malformed.
pub fn parse_line(line: &str, line_no: usize) -> Option<KnownHostEntry> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    let mut fields = trimmed.split_whitespace();
    let mut first = fields.next()?;

    let marker = if first == "@cert-authority" || first == "@revoked" {
        let m = first.to_string();
        first = fields.next()?;
        Some(m)
    } else {
        None
    };

    let host = if first.starts_with("|1|") {
        HostSpec::Hashed(first.to_string())
    } else {
        HostSpec::Plain(first.to_string())
    };

    let key_type = fields.next()?.to_string();
    let key_b64 = fields.next()?.to_string();

    Some(KnownHostEntry {
        line_no,
        marker,
        host,
        key_type,
        key_b64,
        raw: line.to_string(),
    })
}

/// Parse the whole known_hosts file into entries.
pub fn parse_known_hosts() -> Vec<KnownHostEntry> {
    let Some(path) = known_hosts_path() else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    content
        .lines()
        .enumerate()
        .filter_map(|(i, line)| parse_line(line, i))
        .collect()
}

/// Remove a known_hosts entry, rewriting the file without that physical line
/// (after a `known_hosts.old` backup). The removal is **content-addressed**:
/// `raw` must still match the line at `line_no`, otherwise we fall back to the
/// first line equal to `raw`. If neither matches (the file changed on disk), the
/// write is aborted so we never delete the wrong key.
pub fn remove_entry(line_no: usize, raw: &str) -> io::Result<()> {
    let path = known_hosts_path().ok_or_else(|| io::Error::other("cannot resolve ~/.ssh"))?;
    let content = std::fs::read_to_string(&path)?;

    let lines: Vec<&str> = content.lines().collect();
    let target = if lines.get(line_no) == Some(&raw) {
        line_no
    } else {
        match lines.iter().position(|l| *l == raw) {
            Some(i) => i,
            None => {
                return Err(io::Error::other(
                    "known_hosts changed on disk — press r to reload",
                ));
            }
        }
    };

    let backup = path.with_extension("old");
    std::fs::copy(&path, &backup)?;

    let newline = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let trailing = content.ends_with('\n');

    let kept: Vec<&str> = lines
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != target)
        .map(|(_, l)| *l)
        .collect();

    let mut out = kept.join(newline);
    if trailing && !out.is_empty() {
        out.push_str(newline);
    }

    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let tmp = dir.join(".known_hosts.tmp");
    std::fs::write(&tmp, out.as_bytes())?;
    #[cfg(windows)]
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Append entry lines to a known_hosts file (#46 host-key pinning).
///
/// Preservation contract (mirrors `remove_entry`'s newline handling):
/// - the file's existing newline flavor is detected and reused (CRLF file →
///   CRLF appends, LF file → LF appends);
/// - a missing trailing newline on the existing content is repaired BEFORE
///   appending (so the first appended entry never glues onto the last line);
/// - a missing file is created (LF, trailing newline);
/// - the write replaces the file atomically (no torn known_hosts on crash).
pub fn append_entries(path: &Path, lines: &[String]) -> io::Result<()> {
    let existing = match std::fs::read_to_string(path) {
        Ok(c) => Some(c),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let newline = match &existing {
        Some(c) if c.contains("\r\n") => "\r\n",
        _ => "\n",
    };
    let mut out = existing.unwrap_or_default();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(newline);
    }
    for line in lines {
        out.push_str(line);
        out.push_str(newline);
    }

    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("known_hosts");
    let tmp = dir.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, out.as_bytes())?;
    #[cfg(windows)]
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_entry() {
        let e = parse_line("github.com ssh-ed25519 AAAAC3Nz...", 0).unwrap();
        assert!(matches!(e.host, HostSpec::Plain(ref s) if s == "github.com"));
        assert_eq!(e.key_type, "ssh-ed25519");
        assert!(e.marker.is_none());
    }

    #[test]
    fn host_with_ip() {
        let e = parse_line("host,10.0.0.1 ssh-rsa AAAAB3...", 3).unwrap();
        assert!(matches!(e.host, HostSpec::Plain(ref s) if s == "host,10.0.0.1"));
        assert_eq!(e.line_no, 3);
    }

    #[test]
    fn bracketed_port() {
        let e = parse_line("[example.com]:2222 ssh-ed25519 AAAA...", 0).unwrap();
        assert!(matches!(e.host, HostSpec::Plain(ref s) if s == "[example.com]:2222"));
    }

    #[test]
    fn hashed_entry() {
        let e = parse_line("|1|abcd=|efgh= ssh-ed25519 AAAA...", 0).unwrap();
        assert!(matches!(e.host, HostSpec::Hashed(_)));
        assert_eq!(e.host.display(), "(hashed)");
    }

    #[test]
    fn cert_authority_marker() {
        let e = parse_line("@cert-authority *.example.com ssh-rsa AAAA...", 0).unwrap();
        assert_eq!(e.marker.as_deref(), Some("@cert-authority"));
        assert!(matches!(e.host, HostSpec::Plain(ref s) if s == "*.example.com"));
    }

    #[test]
    fn malformed_and_comments_skipped() {
        assert!(parse_line("# a comment", 0).is_none());
        assert!(parse_line("   ", 0).is_none());
        assert!(parse_line("onlyhost", 0).is_none());
    }

    // -- append_entries (#46 AC2: CRLF / trailing-newline preservation) -----

    /// Scratch file seeded with `content` (`None` = no file), unique per test.
    fn append_fixture(name: &str, content: Option<&str>) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sshm-append-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("known_hosts");
        let _ = std::fs::remove_file(&path);
        if let Some(c) = content {
            std::fs::write(&path, c).unwrap();
        }
        path
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn append_entries_lf_file_appends_with_lf() {
        // given — an LF file with a trailing newline
        let path = append_fixture("lf", Some("a ssh-ed25519 AAA\nb ssh-rsa BBB\n"));
        // when
        append_entries(&path, &["h ssh-ed25519 CCC".to_string()]).unwrap();
        // then — appended with LF, existing bytes untouched, trailing \n kept
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "a ssh-ed25519 AAA\nb ssh-rsa BBB\nh ssh-ed25519 CCC\n"
        );
        cleanup(&path);
    }

    #[test]
    fn append_entries_crlf_file_appends_with_crlf() {
        // given — a CRLF file (Windows-managed known_hosts)
        let path = append_fixture("crlf", Some("a ssh-ed25519 AAA\r\n"));
        // when — two entries appended in one call
        append_entries(
            &path,
            &["h ssh-ed25519 CCC".to_string(), "h ssh-rsa DDD".to_string()],
        )
        .unwrap();
        // then — every appended line uses the file's CRLF flavor
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "a ssh-ed25519 AAA\r\nh ssh-ed25519 CCC\r\nh ssh-rsa DDD\r\n"
        );
        cleanup(&path);
    }

    #[test]
    fn append_entries_repairs_missing_trailing_newline() {
        // given — the existing content lacks a trailing newline
        let path = append_fixture("notrail", Some("a ssh-ed25519 AAA"));
        // when
        append_entries(&path, &["h ssh-ed25519 CCC".to_string()]).unwrap();
        // then — a newline is inserted first so the entries never merge
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "a ssh-ed25519 AAA\nh ssh-ed25519 CCC\n"
        );
        cleanup(&path);
    }

    #[test]
    fn append_entries_creates_file_when_absent() {
        // given — no known_hosts yet (fresh machine, first pin)
        let path = append_fixture("create", None);
        // when
        append_entries(&path, &["h ssh-ed25519 CCC".to_string()]).unwrap();
        // then — file created, LF, trailing newline
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "h ssh-ed25519 CCC\n"
        );
        cleanup(&path);
    }
}
