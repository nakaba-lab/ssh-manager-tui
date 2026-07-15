//! Rendering, atomic save, and surgical (line-granularity) editing of the
//! lossless config model.

use std::fs;
use std::path::Path;

use crate::error::ConfigError;

use super::model::{BodyLine, HostBlock, Item, OptionLine, RawLine, SshConfig};
use super::tokens::{detok_value, quote_if_needed};

impl SshConfig {
    /// Flatten the document into physical lines (without endings).
    fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for item in &self.items {
            match item {
                Item::Host(b) => {
                    for c in &b.pre {
                        out.push(c.text.clone());
                    }
                    out.push(b.header.render());
                    for l in &b.body {
                        out.push(l.text());
                    }
                }
                Item::Match(b) => {
                    for c in &b.pre {
                        out.push(c.text.clone());
                    }
                    out.push(b.header.render());
                    for l in &b.body {
                        out.push(l.text());
                    }
                }
                Item::Global(o) => out.push(o.render()),
                Item::Include(i) => out.push(i.option.render()),
                Item::Comment(r) | Item::Blank(r) => out.push(r.text.clone()),
            }
        }
        out
    }

    /// Render the whole document back to text, byte-for-byte for unedited input.
    pub fn render(&self) -> String {
        if self.items.is_empty() {
            return String::new();
        }
        let nl = self.newline.as_str();
        let mut s = self.lines().join(nl);
        if self.trailing_newline {
            s.push_str(nl);
        }
        s
    }

    /// Write the document back to disk atomically, creating a one-time session
    /// backup (`config.bak`) before the first overwrite.
    pub fn save(&mut self) -> Result<(), ConfigError> {
        use std::io::Write;
        let rendered = self.render();
        let path = self.path.clone();
        let dir = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();

        // Owner-private dir on both platforms (Windows ACL gap, #3).
        crate::secure_fs::create_dir_private(&dir).map_err(|source| ConfigError::Io {
            path: dir.clone(),
            source,
        })?;

        // One-time session backup of the CURRENT config, made owner-private (#3).
        if !self.bak_done && path.exists() {
            let bak = path.with_extension("bak");
            fs::copy(&path, &bak).map_err(|source| ConfigError::Io {
                path: bak.clone(),
                source,
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&bak, fs::Permissions::from_mode(0o600));
            }
            #[cfg(windows)]
            crate::secure_fs::restrict_acl(&bak);
            self.bak_done = true;
        }

        // Write into an unpredictable, O_EXCL, owner-private temp, fsync it, then
        // atomically replace the destination. No predictable temp name
        // (symlink/pre-creation TOCTOU, CWE-377, #2). On Windows, ReplaceFileW
        // swaps the temp over the existing config in ONE atomic OS operation — no
        // delete-before-rename window and no orphan left behind (#4); on unix the
        // rename is already atomic. fsync of the temp file + the parent dir makes
        // the swap crash-durable (#4).
        let tmp = dir.join(crate::secure_fs::temp_name(".ssh_manager_config").map_err(
            |source| ConfigError::Io {
                path: dir.clone(),
                source,
            },
        )?);
        let write_res = (|| -> std::io::Result<()> {
            let mut f = crate::secure_fs::create_new_private(&tmp)?;
            f.write_all(rendered.as_bytes())?;
            f.sync_all()?;
            Ok(())
        })();
        if let Err(source) = write_res {
            let _ = fs::remove_file(&tmp);
            return Err(ConfigError::Io { path: tmp, source });
        }

        #[cfg(windows)]
        {
            if path.exists() {
                // Atomic replace: no window where the config is missing, no orphan.
                // ReplaceFileW requires the destination to exist (the `else` arm
                // covers first-time creation). It preserves the destination's
                // existing ACL/attributes; we re-assert owner-only after the block.
                use std::os::windows::ffi::OsStrExt;
                use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
                let wide = |p: &Path| -> Vec<u16> {
                    p.as_os_str()
                        .encode_wide()
                        .chain(std::iter::once(0))
                        .collect()
                };
                let (wdest, wsrc) = (wide(&path), wide(&tmp));
                // SAFETY: both pointers are valid, NUL-terminated UTF-16 paths; the
                // three optional params are null/zero per the Win32 contract.
                let ok = unsafe {
                    ReplaceFileW(
                        wdest.as_ptr(),
                        wsrc.as_ptr(),
                        std::ptr::null(),
                        0,
                        std::ptr::null(),
                        std::ptr::null(),
                    )
                };
                if ok == 0 {
                    let source = std::io::Error::last_os_error();
                    let _ = fs::remove_file(&tmp);
                    return Err(ConfigError::Io {
                        path: path.clone(),
                        source,
                    });
                }
            } else {
                fs::rename(&tmp, &path).map_err(|source| ConfigError::Io {
                    path: path.clone(),
                    source,
                })?;
            }
            // ReplaceFileW preserves the destination's PRIOR ACL, so re-assert
            // owner-only here to tighten a pre-existing loose config too (#3).
            crate::secure_fs::restrict_acl(&path);
        }
        #[cfg(not(windows))]
        {
            fs::rename(&tmp, &path).map_err(|source| ConfigError::Io {
                path: path.clone(),
                source,
            })?;
        }

        crate::secure_fs::fsync_parent_dir(&dir);
        self.dirty = false;
        Ok(())
    }
}

/// Detect the indentation new body lines should use for a block: reuse the first
/// existing option line's indent, else default to four spaces.
pub fn block_indent(block: &HostBlock) -> String {
    for line in &block.body {
        if let BodyLine::Option(o) = line {
            return o.indent.clone();
        }
    }
    "    ".to_string()
}

/// Index just past the last non-blank body line (so new options insert before
/// any trailing blank lines).
fn insert_pos(body: &[BodyLine]) -> usize {
    let mut pos = body.len();
    while pos > 0 && matches!(body[pos - 1], BodyLine::Blank(_)) {
        pos -= 1;
    }
    pos
}

/// Surgically set a single-valued option. `value == None` removes every
/// occurrence. Unchanged values are left byte-for-byte intact.
pub fn set_single(block: &mut HostBlock, keyword: &str, value: Option<&str>, quote: bool) {
    let indent = block_indent(block);
    match value {
        None => {
            block
                .body
                .retain(|l| !matches!(l, BodyLine::Option(o) if o.is(keyword)));
        }
        Some(v) => {
            // Update the first existing occurrence; remove any later duplicates.
            let mut found = false;
            let mut to_remove = Vec::new();
            for (idx, line) in block.body.iter_mut().enumerate() {
                if let BodyLine::Option(o) = line {
                    if !o.is(keyword) {
                        continue;
                    }
                    if !found {
                        found = true;
                        if detok_value(&o.args) != v {
                            o.args = if quote {
                                quote_if_needed(v)
                            } else {
                                v.to_string()
                            };
                        }
                    } else {
                        to_remove.push(idx);
                    }
                }
            }
            for idx in to_remove.into_iter().rev() {
                block.body.remove(idx);
            }
            if !found {
                let args = if quote {
                    quote_if_needed(v)
                } else {
                    v.to_string()
                };
                let opt = OptionLine::new(&indent, keyword, args);
                let pos = insert_pos(&block.body);
                block.body.insert(pos, BodyLine::Option(opt));
            }
        }
    }
}

/// Surgically reconcile a repeated option (IdentityFile / *Forward) to `values`.
/// Shared-prefix entries that already match are left intact; surplus lines are
/// removed; extra values are appended.
pub fn set_multi(block: &mut HostBlock, keyword: &str, values: &[String], quote: bool) {
    let indent = block_indent(block);
    // Indices of existing option lines for this keyword, in order.
    let existing: Vec<usize> = block
        .body
        .iter()
        .enumerate()
        .filter_map(|(i, l)| match l {
            BodyLine::Option(o) if o.is(keyword) => Some(i),
            _ => None,
        })
        .collect();

    // Overwrite in place where values differ.
    for (slot, &idx) in existing.iter().enumerate() {
        if slot >= values.len() {
            break;
        }
        if let BodyLine::Option(o) = &mut block.body[idx] {
            let v = &values[slot];
            if detok_value(&o.args) != *v {
                o.args = if quote { quote_if_needed(v) } else { v.clone() };
            }
        }
    }

    // Remove surplus existing lines (back to front to keep indices valid).
    if values.len() < existing.len() {
        for &idx in existing[values.len()..].iter().rev() {
            block.body.remove(idx);
        }
    }

    // Append any extra values after the last existing slot (or before trailing
    // blanks if there were none).
    if values.len() > existing.len() {
        let base = match existing.last() {
            Some(&last) => last + 1,
            None => insert_pos(&block.body),
        };
        for (offset, v) in values[existing.len()..].iter().enumerate() {
            let args = if quote { quote_if_needed(v) } else { v.clone() };
            block.body.insert(
                base + offset,
                BodyLine::Option(OptionLine::new(&indent, keyword, args)),
            );
        }
    }
}

/// Keywords managed by dedicated edit-form fields; everything else is an "extra".
pub const MANAGED_KEYWORDS: [&str; 8] = [
    "HostName",
    "User",
    "Port",
    "IdentityFile",
    "ProxyJump",
    "LocalForward",
    "RemoteForward",
    "DynamicForward",
];

fn is_managed(keyword: &str) -> bool {
    MANAGED_KEYWORDS
        .iter()
        .any(|m| keyword.eq_ignore_ascii_case(m))
}

/// Surgically reconcile the host's "extra" (non-managed) options to `extras`,
/// preserving the formatting of unchanged lines. Values are written verbatim
/// (the extras field is a raw escape hatch, like forward specs).
pub fn set_extras(block: &mut HostBlock, extras: &[(String, String)]) {
    // Distinct desired keywords (case-insensitive), first-seen order & casing,
    // skipping any that belong to a dedicated field.
    let mut desired: Vec<String> = Vec::new();
    for (k, _) in extras {
        if !is_managed(k) && !desired.iter().any(|d| d.eq_ignore_ascii_case(k)) {
            desired.push(k.clone());
        }
    }
    // Apply each desired keyword's values.
    for kw in &desired {
        let vals: Vec<String> = extras
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(kw))
            .map(|(_, v)| v.clone())
            .collect();
        set_multi(block, kw, &vals, false);
    }
    // Remove any existing non-managed option no longer present in `desired`.
    let mut to_clear: Vec<String> = Vec::new();
    for line in &block.body {
        if let BodyLine::Option(o) = line
            && !is_managed(&o.keyword)
            && !desired.iter().any(|d| d.eq_ignore_ascii_case(&o.keyword))
            && !to_clear.iter().any(|c| c.eq_ignore_ascii_case(&o.keyword))
        {
            to_clear.push(o.keyword.clone());
        }
    }
    for kw in to_clear {
        set_multi(block, &kw, &[], false);
    }
}

/// Surgically reconcile the host's `# sshm:` metadata directives (#45) — tags
/// and description — living in the block's owned preceding comments (`pre`).
///
/// Mirrors `set_single`/`set_multi`'s discipline: a directive whose *parsed*
/// value is unchanged is left byte-for-byte intact (so a hand-written,
/// non-canonical directive round-trips), and every non-`sshm:` comment line in
/// `pre` is never touched. Only when a value actually changes is its directive
/// line rewritten canonically; emptying a value removes its line; a fresh value
/// is appended at the end of `pre`, directly above the header.
pub fn set_pre(block: &mut HostBlock, tags: &[String], description: Option<&str>) {
    let indent = block.header.indent.clone();

    let tags_desired: Option<String> = (!tags.is_empty()).then(|| tags.join(","));
    reconcile_directive(block, &indent, "tags", tags_desired.as_deref(), |rest| {
        super::model::parse_sshm_tags(rest).as_slice() == tags
    });

    let desc_desired: Option<String> = description
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string);
    reconcile_directive(block, &indent, "desc", desc_desired.as_deref(), |rest| {
        rest.trim() == desc_desired.as_deref().unwrap_or("")
    });
}

/// Shared core for [`set_pre`]: reconcile the first `# sshm:<key>` line in `pre`
/// toward `desired` (canonical remainder, or `None` to remove all), leaving it
/// byte-identical when `unchanged` reports the parsed value already matches.
fn reconcile_directive(
    block: &mut HostBlock,
    indent: &str,
    key: &str,
    desired: Option<&str>,
    unchanged: impl Fn(&str) -> bool,
) {
    let existing: Vec<usize> = block
        .pre
        .iter()
        .enumerate()
        .filter_map(|(i, r)| super::model::sshm_directive(&r.text, key).map(|_| i))
        .collect();

    match desired {
        None => {
            for &i in existing.iter().rev() {
                block.pre.remove(i);
            }
        }
        Some(val) => match existing.first() {
            Some(&first) => {
                let is_unchanged = {
                    let rest =
                        super::model::sshm_directive(&block.pre[first].text, key).unwrap_or("");
                    unchanged(rest)
                };
                if !is_unchanged {
                    block.pre[first].text = format!("{indent}# sshm:{key} {val}");
                }
                // Drop any later duplicates of the same directive.
                for &i in existing[1..].iter().rev() {
                    block.pre.remove(i);
                }
            }
            None => block
                .pre
                .push(RawLine::new(format!("{indent}# sshm:{key} {val}"))),
        },
    }
}

/// Replace the header patterns, preserving the header's indent and separator.
pub fn set_patterns(block: &mut HostBlock, patterns: &[String]) {
    block.patterns = patterns.to_vec();
    block.header.args = patterns
        .iter()
        .map(|p| quote_if_needed(p))
        .collect::<Vec<_>>()
        .join(" ");
}

/// Build a fresh, well-formatted host block from a primary alias.
pub fn new_host_block(patterns: &[String]) -> HostBlock {
    let header = OptionLine {
        indent: String::new(),
        keyword: "Host".to_string(),
        sep: " ".to_string(),
        args: patterns
            .iter()
            .map(|p| quote_if_needed(p))
            .collect::<Vec<_>>()
            .join(" "),
        src_line: None,
    };
    HostBlock {
        pre: Vec::new(),
        header,
        patterns: patterns.to_vec(),
        body: Vec::new(),
    }
}

/// Append a top-level blank line item if the document doesn't already end with
/// a blank (keeps exactly one blank between blocks).
pub fn ensure_trailing_separator(config: &mut SshConfig) {
    let needs = match config.items.last() {
        None => false,
        Some(Item::Blank(_)) => false,
        Some(_) => true,
    };
    if needs {
        config.items.push(Item::Blank(RawLine::new(String::new())));
    }
}
