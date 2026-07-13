//! Pure line-level diff between two rendered config texts, for the before-save
//! preview (issue #42).
//!
//! This mirrors what the atomic save will actually write: the `old` side is the
//! current on-disk file and the `new` side is [`SshConfig::render`](super::SshConfig::render)
//! of the config with the pending edit applied. It has **zero** ratatui
//! dependency — painting lives in `ui/diff.rs`; this module only produces the
//! tagged line list, so it is fully headless-testable (inline `#[cfg(test)]`,
//! the `config/` convention).
//!
//! The algorithm trims the common prefix and suffix (the whole unchanged file
//! around a surgical edit) and runs a classic longest-common-subsequence diff on
//! only the changed middle. Trimming keeps the `O(n·m)` LCS table tiny for the
//! common case — a one-field edit in a large config — while still producing a
//! correct minimal diff of whatever genuinely differs.

/// One line of a unified-style diff, in output order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    /// Unchanged: present, identical, in both texts.
    Context(String),
    /// Added: present only in the new text (`+`).
    Add(String),
    /// Removed: present only in the old text (`-`).
    Del(String),
}

impl DiffLine {
    /// The line's text, without any diff marker.
    pub fn text(&self) -> &str {
        match self {
            DiffLine::Context(s) | DiffLine::Add(s) | DiffLine::Del(s) => s,
        }
    }
}

/// Count of `(added, removed)` lines in a diff — drives the preview's `+N -M`
/// summary and lets tests assert "no change" without inspecting every line.
pub fn stats(diff: &[DiffLine]) -> (usize, usize) {
    let mut added = 0;
    let mut removed = 0;
    for line in diff {
        match line {
            DiffLine::Add(_) => added += 1,
            DiffLine::Del(_) => removed += 1,
            DiffLine::Context(_) => {}
        }
    }
    (added, removed)
}

/// Diff two texts by logical line. Line endings are normalized away
/// ([`str::lines`] drops `\n`/`\r\n`), so this compares *content*, not the byte
/// exact newline style — a CRLF file and its LF re-render show no spurious change.
pub fn diff(old: &str, new: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    diff_lines(&a, &b)
}

/// Core diff over pre-split line slices: trim the common prefix/suffix, LCS the
/// middle, then re-attach the trimmed suffix as context.
fn diff_lines(a: &[&str], b: &[&str]) -> Vec<DiffLine> {
    let mut out = Vec::new();

    // Common prefix: identical leading lines are emitted verbatim as context.
    let mut lo = 0;
    while lo < a.len() && lo < b.len() && a[lo] == b[lo] {
        out.push(DiffLine::Context(a[lo].to_string()));
        lo += 1;
    }

    // Common suffix: shrink both ends inward while they match, but never past the
    // prefix boundary `lo` (so a line is never claimed by both prefix and suffix).
    let mut hi_a = a.len();
    let mut hi_b = b.len();
    while hi_a > lo && hi_b > lo && a[hi_a - 1] == b[hi_b - 1] {
        hi_a -= 1;
        hi_b -= 1;
    }

    // The genuinely-differing middle gets a full LCS diff.
    out.extend(lcs_diff(&a[lo..hi_a], &b[lo..hi_b]));

    // Re-attach the common suffix as context (identical in both, take it from `a`).
    for &line in &a[hi_a..] {
        out.push(DiffLine::Context(line.to_string()));
    }
    out
}

/// Longest-common-subsequence diff of two line slices. Lines not on the LCS are
/// deletions (from `a`) or additions (from `b`); shared lines are context.
fn lcs_diff(a: &[&str], b: &[&str]) -> Vec<DiffLine> {
    let (n, m) = (a.len(), b.len());
    // Fast paths avoid allocating a DP table when one side is empty (pure
    // insert/delete) — the common shape after prefix/suffix trimming.
    if n == 0 {
        return b.iter().map(|s| DiffLine::Add(s.to_string())).collect();
    }
    if m == 0 {
        return a.iter().map(|s| DiffLine::Del(s.to_string())).collect();
    }

    // dp[i*w + j] = LCS length of a[i..] and b[j..], row stride w = m + 1. A
    // single flat allocation (not `Vec<Vec<_>>`) keeps the table contiguous and
    // cache-friendly and avoids n+1 separate heap allocations. Filled bottom-up
    // so the forward backtrack below can greedily reproduce a minimal edit script.
    let w = m + 1;
    let mut dp = vec![0usize; (n + 1) * w];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i * w + j] = if a[i] == b[j] {
                dp[(i + 1) * w + (j + 1)] + 1
            } else {
                dp[(i + 1) * w + j].max(dp[i * w + (j + 1)])
            };
        }
    }

    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(DiffLine::Context(a[i].to_string()));
            i += 1;
            j += 1;
        } else if dp[(i + 1) * w + j] >= dp[i * w + (j + 1)] {
            // Deleting a[i] is at least as good as inserting b[j]: prefer it so
            // deletions render before the additions that replace them.
            out.push(DiffLine::Del(a[i].to_string()));
            i += 1;
        } else {
            out.push(DiffLine::Add(b[j].to_string()));
            j += 1;
        }
    }
    // Drain whichever side is left once the other is exhausted.
    for &line in &a[i..] {
        out.push(DiffLine::Del(line.to_string()));
    }
    for &line in &b[j..] {
        out.push(DiffLine::Add(line.to_string()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parser;
    use std::path::PathBuf;

    /// Collapse a diff into a compact string of tagged lines for readable asserts:
    /// ` ` context, `+` add, `-` del.
    fn sketch(diff: &[DiffLine]) -> String {
        diff.iter()
            .map(|l| match l {
                DiffLine::Context(s) => format!(" {s}"),
                DiffLine::Add(s) => format!("+{s}"),
                DiffLine::Del(s) => format!("-{s}"),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn empty_vs_empty_is_no_diff() {
        assert!(diff("", "").is_empty());
    }

    #[test]
    fn identical_text_is_all_context_no_changes() {
        let s = "Host a\n    HostName 1.1.1.1\n    User deploy\n";
        let d = diff(s, s);
        assert_eq!(stats(&d), (0, 0));
        assert!(d.iter().all(|l| matches!(l, DiffLine::Context(_))));
    }

    #[test]
    fn add_into_empty_is_all_additions() {
        let d = diff("", "Host a\n    HostName 1.1.1.1\n");
        assert_eq!(stats(&d), (2, 0));
        assert_eq!(sketch(&d), "+Host a\n+    HostName 1.1.1.1");
    }

    #[test]
    fn delete_to_empty_is_all_deletions() {
        let d = diff("Host a\n    HostName 1.1.1.1\n", "");
        assert_eq!(stats(&d), (0, 2));
        assert_eq!(sketch(&d), "-Host a\n-    HostName 1.1.1.1");
    }

    #[test]
    fn single_field_change_touches_only_that_line() {
        // The surrounding lines survive as context; only the changed value is a
        // -/+ pair. This is the core before-save experience.
        let old = "Host a\n    HostName old.example.com\n    User deploy\n    Port 22\n";
        let new = "Host a\n    HostName new.example.com\n    User deploy\n    Port 22\n";
        let d = diff(old, new);
        assert_eq!(stats(&d), (1, 1));
        assert_eq!(
            sketch(&d),
            " Host a\n-    HostName old.example.com\n+    HostName new.example.com\n     User deploy\n     Port 22"
        );
    }

    #[test]
    fn appended_block_keeps_prefix_as_context() {
        let old = "Host a\n    HostName 1.1.1.1\n";
        let new = "Host a\n    HostName 1.1.1.1\n\nHost b\n    HostName 2.2.2.2\n";
        let d = diff(old, new);
        assert_eq!(stats(&d), (3, 0));
        assert_eq!(
            sketch(&d),
            " Host a\n     HostName 1.1.1.1\n+\n+Host b\n+    HostName 2.2.2.2"
        );
    }

    #[test]
    fn common_suffix_is_preserved_as_context() {
        // A change high in the file must not turn the identical tail into churn.
        let old = "Port 22\nHost a\n    HostName 1.1.1.1\n";
        let new = "Port 2222\nHost a\n    HostName 1.1.1.1\n";
        let d = diff(old, new);
        assert_eq!(stats(&d), (1, 1));
        assert_eq!(
            sketch(&d),
            "-Port 22\n+Port 2222\n Host a\n     HostName 1.1.1.1"
        );
    }

    #[test]
    fn crlf_vs_lf_same_content_is_no_diff() {
        // Line endings are normalized away, so re-rendering a CRLF file to LF (or
        // vice versa) reports no change — content is what the user reviews. The
        // lines still round-trip as context; "no change" means zero add/del.
        let d = diff(
            "Host a\r\n    HostName 1.1.1.1\r\n",
            "Host a\n    HostName 1.1.1.1\n",
        );
        assert_eq!(stats(&d), (0, 0), "content is identical: {}", sketch(&d));
        assert!(d.iter().all(|l| matches!(l, DiffLine::Context(_))));
    }

    // The load-bearing invariant from issue #42: a byte-for-byte round-trip must
    // introduce no change, so an unedited save previews as "no changes" (zero
    // add/del lines — the file still shows in full as context).
    #[test]
    fn roundtrip_of_parsed_config_reports_no_change() {
        let samples = [
            "",
            "Host web1\n    HostName 10.0.0.1\n    User deploy\n    Port 22\n",
            "# global note\n\nHost a\n    # inner comment\n    HostName 1.1.1.1\n\n# describes b\nHost b\n    HostName 2.2.2.2\n",
            "Host x\n    Port=22\n    HostName = example.com\n    User =git\n",
            "Match host x user y\n    ForwardAgent yes\n\nHost a\n    HostName 1.1.1.1\n",
            "Host a\n    IdentityFile \"C:\\path with space\\id\"\n    HostName 1.1.1.1\n",
        ];
        for s in samples {
            let cfg = parser::parse(PathBuf::from("t"), s);
            let d = diff(s, &cfg.render());
            assert_eq!(
                stats(&d),
                (0, 0),
                "diff(s, render(parse(s))) must report no change for {s:?}, got:\n{}",
                sketch(&d)
            );
            assert!(
                d.iter().all(|l| matches!(l, DiffLine::Context(_))),
                "every round-trip line must be context for {s:?}"
            );
        }
    }

    #[test]
    fn reordered_lines_diff_minimally() {
        // LCS keeps the shared line as context rather than rewriting both lines.
        let old = "A\nB\nC\n";
        let new = "A\nC\nB\n";
        let d = diff(old, new);
        // One line moves: exactly one add and one delete, A stays context.
        assert_eq!(stats(&d), (1, 1));
        assert!(matches!(&d[0], DiffLine::Context(s) if s == "A"));
    }
}
