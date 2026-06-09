//! Lossless, round-trip-preserving model of `~/.ssh/config`.
//!
//! The core invariant is `render(parse(s)) == s` byte-for-byte for any unedited
//! file. To achieve that, the document is stored as an ordered list of [`Item`]s
//! where every line keeps its original indentation, keyword casing, separator
//! and argument text. Editing mutates a [`HostBlock`]'s body at *line*
//! granularity so comments / ordering / formatting survive.

use std::path::PathBuf;

/// Line ending detected for the file (we render uniformly with it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Newline {
    #[default]
    Lf,
    Crlf,
}

impl Newline {
    pub fn as_str(self) -> &'static str {
        match self {
            Newline::Lf => "\n",
            Newline::Crlf => "\r\n",
        }
    }
}

/// The whole file as an ordered list of top-level items.
#[derive(Debug, Clone, Default)]
pub struct SshConfig {
    pub items: Vec<Item>,
    pub newline: Newline,
    /// True if the original file ended with a trailing newline.
    pub trailing_newline: bool,
    /// Absolute path this was loaded from (for writing back).
    pub path: PathBuf,
    /// True once any in-memory edit has happened this session.
    pub dirty: bool,
    /// True if a UTF-8 BOM was present (stripped on parse, NOT re-added).
    #[allow(dead_code)] // surfaced in tests / future "file info" view
    pub had_bom: bool,
    /// True once the one-time session backup (`config.bak`) has been written.
    pub bak_done: bool,
}

/// A top-level structural node.
#[derive(Debug, Clone)]
pub enum Item {
    Host(HostBlock),
    Match(MatchBlock),
    /// An option line appearing before the first Host/Match (applies globally).
    Global(OptionLine),
    /// `Include path...` kept verbatim; included files are not parsed in v1.
    Include(IncludeLine),
    /// A full-line `# comment` at top level.
    Comment(RawLine),
    /// A blank / whitespace-only line at top level.
    Blank(RawLine),
}

/// A line preserved verbatim. `text` excludes the line ending.
#[derive(Debug, Clone)]
pub struct RawLine {
    pub text: String,
    /// 1-based source line (provenance; retained for diagnostics).
    #[allow(dead_code)]
    pub src_line: Option<usize>,
}

impl RawLine {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            src_line: None,
        }
    }
}

/// One `Keyword args` directive, lossless but with parsed parts.
#[derive(Debug, Clone)]
pub struct OptionLine {
    /// Leading whitespace exactly as written.
    pub indent: String,
    /// Original casing, e.g. "HostName".
    pub keyword: String,
    /// Separator as written: " ", " = ", "=", etc.
    pub sep: String,
    /// Argument text as written (quoting preserved).
    pub args: String,
    /// 1-based source line (provenance; retained for diagnostics).
    #[allow(dead_code)]
    pub src_line: Option<usize>,
}

impl OptionLine {
    pub fn is(&self, kw: &str) -> bool {
        self.keyword.eq_ignore_ascii_case(kw)
    }

    pub fn render(&self) -> String {
        format!("{}{}{}{}", self.indent, self.keyword, self.sep, self.args)
    }

    /// Build a fresh option line with canonical formatting.
    pub fn new(indent: &str, keyword: &str, args: impl Into<String>) -> Self {
        Self {
            indent: indent.to_string(),
            keyword: keyword.to_string(),
            sep: " ".to_string(),
            args: args.into(),
            src_line: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IncludeLine {
    pub option: OptionLine,
    /// Parsed include paths, retained for a future read-only includes view.
    #[allow(dead_code)]
    pub paths: Vec<String>,
}

/// A `Host pattern...` block.
#[derive(Debug, Clone)]
pub struct HostBlock {
    /// Comment lines immediately preceding the header (no blank gap). These are
    /// rendered before the header and are considered "owned" by this block.
    pub pre: Vec<RawLine>,
    /// Header line: indent + "Host" + sep + patterns text.
    pub header: OptionLine,
    /// Parsed patterns, e.g. `["web1", "web*", "!web9"]`.
    pub patterns: Vec<String>,
    /// Options / comments / blanks in original order (the source of truth).
    pub body: Vec<BodyLine>,
}

#[derive(Debug, Clone)]
pub struct MatchBlock {
    pub pre: Vec<RawLine>,
    pub header: OptionLine,
    /// Everything after "Match", opaque (mirrors `header.args`; kept for clarity).
    #[allow(dead_code)]
    pub criteria_raw: String,
    pub body: Vec<BodyLine>,
}

#[derive(Debug, Clone)]
pub enum BodyLine {
    Option(OptionLine),
    Comment(RawLine),
    Blank(RawLine),
}

impl BodyLine {
    pub fn text(&self) -> String {
        match self {
            BodyLine::Option(o) => o.render(),
            BodyLine::Comment(r) | BodyLine::Blank(r) => r.text.clone(),
        }
    }
}

/// Flattened, editable projection of a [`HostBlock`] for the TUI form.
///
/// Built on demand from `body`; edits are written BACK surgically. `port` is a
/// `String` on purpose: round-trip whatever's there, validate on save.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostView {
    pub patterns: Vec<String>,
    pub host_name: Option<String>,
    pub user: Option<String>,
    pub port: Option<String>,
    pub identity_files: Vec<String>,
    pub proxy_jump: Option<String>,
    pub local_forwards: Vec<String>,
    pub remote_forwards: Vec<String>,
    pub dynamic_forwards: Vec<String>,
    /// Every other option line (keyword, value), order-preserving.
    pub extras: Vec<(String, String)>,
}

impl HostView {
    /// Primary alias = first pattern (used as ssh destination & liveness key).
    pub fn alias(&self) -> &str {
        self.patterns.first().map(String::as_str).unwrap_or("")
    }

    /// True when reaching this host goes through a proxy — either `ProxyJump`
    /// or a `ProxyCommand` (which lives in [`extras`](Self::extras), having no
    /// dedicated form field). A direct TCP probe to `HostName` is meaningless in
    /// that case, so liveness skips it instead of reporting a false "down".
    ///
    /// `none` disables either directive, so it does not count as proxied.
    pub fn is_proxied(&self) -> bool {
        fn is_active(v: &str) -> bool {
            let v = v.trim();
            !v.is_empty() && !v.eq_ignore_ascii_case("none")
        }
        self.proxy_jump.as_deref().is_some_and(is_active)
            || self
                .extras
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("ProxyCommand") && is_active(v))
    }

    /// Project a parsed [`HostBlock`] into an editable view.
    pub fn from_block(block: &HostBlock) -> HostView {
        let mut v = HostView {
            patterns: block.patterns.clone(),
            ..Default::default()
        };
        for line in &block.body {
            let BodyLine::Option(opt) = line else {
                continue;
            };
            let val = crate::config::tokens::detok_value(&opt.args);
            if opt.is("HostName") {
                v.host_name.get_or_insert(val);
            } else if opt.is("User") {
                v.user.get_or_insert(val);
            } else if opt.is("Port") {
                v.port.get_or_insert(val);
            } else if opt.is("IdentityFile") {
                v.identity_files.push(val);
            } else if opt.is("ProxyJump") {
                v.proxy_jump.get_or_insert(val);
            } else if opt.is("LocalForward") {
                v.local_forwards.push(val);
            } else if opt.is("RemoteForward") {
                v.remote_forwards.push(val);
            } else if opt.is("DynamicForward") {
                v.dynamic_forwards.push(val);
            } else {
                v.extras.push((opt.keyword.clone(), val));
            }
        }
        v
    }
}

#[cfg(test)]
mod proxied_tests {
    use super::HostView;

    fn view_with_extra(keyword: &str, value: &str) -> HostView {
        HostView {
            extras: vec![(keyword.to_string(), value.to_string())],
            ..Default::default()
        }
    }

    #[test]
    fn plain_host_is_not_proxied() {
        assert!(!HostView::default().is_proxied());
    }

    #[test]
    fn proxy_jump_counts_as_proxied() {
        let v = HostView {
            proxy_jump: Some("bastion".into()),
            ..Default::default()
        };
        assert!(v.is_proxied());
    }

    #[test]
    fn proxy_command_in_extras_counts_as_proxied() {
        // ProxyCommand has no dedicated field, so it round-trips through extras.
        assert!(view_with_extra("ProxyCommand", "ssh -W %h:%p bastion").is_proxied());
        // Keyword casing must not matter.
        assert!(view_with_extra("proxycommand", "cloudflared access ssh").is_proxied());
    }

    #[test]
    fn none_disables_either_directive() {
        let jump_none = HostView {
            proxy_jump: Some("none".into()),
            ..Default::default()
        };
        assert!(!jump_none.is_proxied());
        assert!(!view_with_extra("ProxyCommand", "none").is_proxied());
        assert!(!view_with_extra("ProxyCommand", "  None  ").is_proxied());
    }

    #[test]
    fn unrelated_extras_do_not_count() {
        assert!(!view_with_extra("ForwardAgent", "yes").is_proxied());
    }
}
