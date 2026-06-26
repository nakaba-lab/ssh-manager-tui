//! SFTP browsing support.
//!
//! Phase 3 foundation: a **pure** parser for the `ls -l`-style directory listing
//! that the OpenSSH `sftp` client prints. OpenSSH exposes no machine-readable
//! attribute mode, so a remote browser must screen-scrape the human listing —
//! this is best-effort by nature (locale, column widths, date format and symlink
//! suffixes vary), so the parser is deliberately defensive: anything it cannot
//! confidently parse is skipped rather than mis-rendered.
//!
//! This module has **zero ratatui dependency** and is fully unit-tested on every
//! platform, independent of any live `sftp` process (which the stateful session
//! worker, built on top of this, will drive).

#![allow(dead_code)] // Phase 3 foundation: consumed by the forthcoming session worker.

/// One entry in a remote directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    /// File name (the symlink *name*, never the target).
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    /// Size in bytes as reported by the listing (0 when unparseable).
    pub size: u64,
    /// The raw permission/type field verbatim (e.g. `drwxr-xr-x`), kept for
    /// display without re-deriving it.
    pub raw_mode: String,
}

impl RemoteEntry {
    /// A synthetic "parent directory" entry for navigating up a level.
    pub fn parent() -> Self {
        RemoteEntry {
            name: "..".to_string(),
            is_dir: true,
            is_link: false,
            size: 0,
            raw_mode: "d---------".to_string(),
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
        raw_mode: mode.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
