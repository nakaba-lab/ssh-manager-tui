//! Read-only expansion of `Include` directives (#52).
//!
//! See CLAUDE.md "layering" and `docs/design/includes.md`. This module has
//! **zero** ratatui dependency and is headless-testable. It reads and parses
//! `Include`d files so their hosts can be *listed and viewed* (never edited):
//! the surgical single-file writer path is untouched, and included files are
//! never written back.
//!
//! Path resolution follows OpenSSH semantics: a leading `~` expands to the home
//! directory, a relative path resolves against the user config's `~/.ssh`, and
//! an absolute path is used verbatim. Recursion is bounded by [`MAX_DEPTH`]
//! (mirroring `os/keys.rs`) and a visited-path set guards cycles.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::config::model::{BodyLine, HostView, Item, SshConfig};
use crate::config::parser;

/// Maximum `Include` recursion depth, mirroring `os/keys.rs::MAX_DEPTH`.
const MAX_DEPTH: usize = 8;

/// A host surfaced (read-only) from an `Include`d file.
#[derive(Debug, Clone)]
pub struct IncludedHost {
    /// The file this host was read from (for the origin hint in the list).
    pub origin: PathBuf,
    /// Read-only projection of the host block.
    pub view: HostView,
    /// True when an earlier host (the main file or an earlier include) already
    /// claimed this alias — OpenSSH "first-wins", surfaced for the display.
    pub shadowed: bool,
}

/// Resolve one `Include` argument to a filesystem path (which may still contain
/// glob metacharacters). OpenSSH semantics: `~/…` expands against `home`, a
/// relative path resolves against `base_dir` (the user config's `~/.ssh`), and
/// an absolute path is returned verbatim.
pub fn resolve_include_arg(arg: &str, base_dir: &Path, home: &Path) -> PathBuf {
    // Only `~` and `~/…` are handled here. OpenSSH globs with `GLOB_TILDE`, so it
    // ALSO expands `~user/…`; that shape is rejected upstream by
    // [`is_unresolvable_arg`] rather than mis-resolved here.
    if arg == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = arg.strip_prefix("~/") {
        return home.join(rest);
    }
    let p = Path::new(arg);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base_dir.join(p)
    }
}

/// The full result of [`expand`].
#[derive(Debug, Clone, Default)]
pub struct Expansion {
    pub hosts: Vec<IncludedHost>,
    /// Lossy raw text of every included file read — backs the whole-config
    /// `Match exec` safety scan (including files with no `Host` block).
    pub texts: Vec<String>,
    /// True when some file `ssh -G` would read went **unscanned**: an `Include`
    /// form this expander cannot follow (block-nested, quote-spliced, past
    /// [`MAX_DEPTH`]), an argument shape it cannot resolve (see
    /// [`is_unresolvable_arg`] — `%` tokens, `${ENV}`, `~user/…`, a unix
    /// backslash, lossy `U+FFFD`), a glob pattern it cannot evaluate (see
    /// [`glob_matches`]), or a path it could not stat/read — including a
    /// non-regular file, which is never opened. A caller gating a
    /// security-sensitive action (autofill, the inspector) must fail safe when
    /// this is set, since a `Match exec` could hide in there.
    ///
    /// A missing path or a directory does NOT set it: `ssh` cannot read those
    /// either, so nothing is actually left unscanned.
    pub blind_spot: bool,
}

/// Expand every `Include` reachable from `main` (read-only), returning the hosts
/// found in included files plus the raw text of every file read (see
/// [`Expansion`]). `base_dir` is where relative includes resolve (OpenSSH:
/// `~/.ssh`); `home` expands a leading `~`. Unreadable includes are skipped
/// (fail-soft); recursion is bounded by [`MAX_DEPTH`] and a visited-path set
/// guards cycles. A host whose alias already appeared (in `main` or an earlier
/// include) is flagged `shadowed`. `texts` lets callers scan the *whole*
/// effective config for dangerous directives (e.g. `Match exec`) so an
/// include-only file cannot slip one past a main-file-only scan.
pub fn expand(main: &SshConfig, base_dir: &Path, home: &Path) -> Expansion {
    let mut seen: HashSet<String> = HashSet::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut acc = Expansion::default();
    // A blind-spot include in the MAIN file (block-nested or quote-spliced) means
    // `ssh -G` could pull in an un-scanned file — fail safe.
    if has_unfollowed_include(main) {
        acc.blind_spot = true;
    }
    // Walk the main items IN ORDER so alias "first-wins" is positional. A top-level
    // Include is honored at its position, and (because the parser folds any Include
    // after the first Host/Match into that block's body) it always precedes the
    // main hosts — so an included host is shadowed only by an alias seen earlier in
    // the walk, never by a main host that textually follows the Include.
    for item in &main.items {
        match item {
            Item::Host(b) => {
                // Every pattern is an alias, so each can shadow a later duplicate.
                for alias in &b.patterns {
                    seen.insert(alias.clone());
                }
            }
            Item::Include(inc) => {
                expand_paths(
                    &inc.paths,
                    base_dir,
                    home,
                    0,
                    &mut seen,
                    &mut visited,
                    &mut acc,
                );
            }
            _ => {}
        }
    }
    acc
}

/// Follow a set of `Include` path arguments at recursion `depth`, appending the
/// hosts (and file texts) found to `acc`. Stops once `depth` reaches [`MAX_DEPTH`].
fn expand_paths(
    paths: &[String],
    base_dir: &Path,
    home: &Path,
    depth: usize,
    seen: &mut HashSet<String>,
    visited: &mut HashSet<PathBuf>,
    acc: &mut Expansion,
) {
    if depth >= MAX_DEPTH {
        // `ssh -G` follows includes deeper than we do; an un-scanned file past our
        // limit could hide a `Match exec`, so flag the blind spot.
        if !paths.is_empty() {
            acc.blind_spot = true;
        }
        return;
    }
    for arg in paths {
        // An argument shape we cannot faithfully resolve would send us to a
        // different file than `ssh` opens — un-scannable, so fail safe.
        if is_unresolvable_arg(arg) {
            acc.blind_spot = true;
            continue;
        }
        let resolved = resolve_include_arg(arg, base_dir, home);
        for file in glob_matches(&resolved, acc) {
            read_included_file(&file, base_dir, home, depth, seen, visited, acc);
        }
    }
}

/// True for `Include` argument shapes `ssh` expands but [`resolve_include_arg`]
/// does not, so we would resolve to a different path (usually a nonexistent one)
/// and silently scan less than `ssh` reads. OpenSSH percent-expands the argument
/// and passes it to `glob(3)` with `GLOB_TILDE`, so all of these are live:
///
/// - percent tokens (`%d`, `%h`, …) and environment references (`${VAR}`);
/// - `~user/…` — `GLOB_TILDE` expands another user's home, we would treat it as a
///   relative path (this also covers the Windows-valid `~\path`);
/// - on unix, a backslash — `glob(3)` unescapes `\x` to `x` (OpenSSH does not set
///   `GLOB_NOESCAPE`), so `evil\.conf` reads `evil.conf`. On Windows `\` is the
///   path separator and must NOT be flagged, or every normal include would be;
/// - `U+FFFD`, which only appears because an including file was decoded lossily —
///   the real argument had bytes we cannot reproduce.
fn is_unresolvable_arg(arg: &str) -> bool {
    // `~` alone and `~/…` are the shapes resolve_include_arg handles.
    let unsupported_tilde = arg.starts_with('~') && arg != "~" && !arg.starts_with("~/");
    unsupported_tilde
        || arg.contains('%')
        || arg.contains("${")
        || arg.contains('\u{fffd}')
        || (cfg!(unix) && arg.contains('\\'))
}

/// Rewrite a pattern into the `glob(3)` dialect OpenSSH actually uses before
/// handing it to the `glob` crate: POSIX has no `**`, where the crate gives it
/// recursive component semantics. Left as-is, `config.d/**` matches **nothing**
/// in the crate over a flat directory while `glob(3)` matches every entry — a
/// silent "scanned nothing, looks clean". Collapsing runs of `*` restores POSIX
/// meaning.
fn to_posix_glob(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut prev_star = false;
    for c in pattern.chars() {
        if c == '*' {
            if !prev_star {
                out.push(c);
            }
            prev_star = true;
        } else {
            out.push(c);
            prev_star = false;
        }
    }
    out
}

/// Expand a resolved include path into concrete files. A path containing glob
/// metacharacters (`* ? [`) is globbed (existing matches, alphabetical); a
/// literal path is returned as-is (existence is checked when it is read, so a
/// missing literal include is skipped fail-soft).
///
/// A pattern this crate **refuses to evaluate** sets `blind_spot`: `glob` rejects
/// e.g. an unbalanced `[` as a pattern error, whereas the libc `glob(3)` that
/// OpenSSH uses treats it as a literal character and happily matches files — so
/// "we could not evaluate it" must never be confused with "it matched nothing".
/// An entry that errors mid-traversal (e.g. EACCES on a directory) is likewise
/// un-scannable and flags the blind spot. A pattern that evaluates cleanly and
/// matches zero files is NOT a blind spot (`glob(3)` yields nothing there too).
fn glob_matches(resolved: &Path, acc: &mut Expansion) -> Vec<PathBuf> {
    let s = resolved.to_string_lossy();
    if !s.contains(['*', '?', '[']) {
        return vec![resolved.to_path_buf()];
    }
    // `glob(3)` semantics: no `**`, and a leading dot is never matched by a
    // wildcard — matching hidden files would scan (and list hosts from) files
    // `ssh` never reads.
    let options = glob::MatchOptions {
        require_literal_leading_dot: true,
        ..glob::MatchOptions::new()
    };
    match glob::glob_with(&to_posix_glob(&s), options) {
        Ok(paths) => paths
            .filter_map(|r| {
                r.inspect_err(|_| acc.blind_spot = true) // unreadable while walking
                    .ok()
            })
            .collect(),
        Err(_) => {
            acc.blind_spot = true;
            Vec::new()
        }
    }
}

/// What [`read_config_text`] could make of a path.
pub enum ReadOutcome {
    /// The file's bytes, decoded lossily — `ssh`'s byte-oriented parser would
    /// honor an ASCII `Match exec` line even in a file with stray non-UTF-8 bytes.
    Text(String),
    /// Nothing to scan and nothing missed: `ssh` cannot read it either (the path
    /// does not exist, or it is a directory).
    Missing,
    /// We could not read it but `ssh` may still be able to — the caller must fail
    /// safe. Includes non-regular files (FIFO / socket / device), which are
    /// deliberately **never opened**: reading a FIFO would block forever.
    Unscannable,
}

/// Read a config file for scanning without ever blocking on it. Stats the path
/// first so a FIFO cannot wedge the caller, then reads it lossily.
///
/// The stat→read window is inherently racy (an attacker controlling the directory
/// could swap a regular file for a FIFO in between); this narrows it to a race
/// rather than a standing hazard.
pub fn read_config_text(path: &Path) -> ReadOutcome {
    match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ReadOutcome::Missing,
        Err(_) => return ReadOutcome::Unscannable, // e.g. permission: ssh might read it
        Ok(md) if md.is_dir() => return ReadOutcome::Missing, // ssh skips directories too
        Ok(md) if !md.is_file() => return ReadOutcome::Unscannable, // do NOT open
        Ok(_) => {}
    }
    match std::fs::read(path) {
        Ok(bytes) => ReadOutcome::Text(String::from_utf8_lossy(&bytes).into_owned()),
        Err(_) => ReadOutcome::Unscannable, // stat said regular, but we cannot read it
    }
}

/// Read and project one included file's hosts, recursing into its own includes.
/// The canonical path guards cycles.
///
/// A missing path or a directory is skipped silently — `ssh` cannot read those
/// either, so nothing is unscanned. Anything else we fail to read (a permission
/// error, or a non-regular file such as a FIFO, which we deliberately do NOT open
/// because that would block the UI thread forever) sets `blind_spot`: `ssh` may
/// still read it, so we must not report "clean" for a file we never saw.
fn read_included_file(
    file: &Path,
    base_dir: &Path,
    home: &Path,
    depth: usize,
    seen: &mut HashSet<String>,
    visited: &mut HashSet<PathBuf>,
    acc: &mut Expansion,
) {
    // Cycle guard on the canonical path (fall back to the raw path when the file
    // cannot be canonicalized, e.g. it does not exist yet — it will fail the read
    // below anyway).
    let key = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    if !visited.insert(key) {
        return;
    }
    let content = match read_config_text(file) {
        ReadOutcome::Text(t) => t,
        ReadOutcome::Missing => return,
        ReadOutcome::Unscannable => {
            acc.blind_spot = true;
            return;
        }
    };
    let cfg = parser::parse(file.to_path_buf(), &content);
    // Any include form we can't follow makes the safety scan incomplete.
    if has_unfollowed_include(&cfg) {
        acc.blind_spot = true;
    }
    // Keep the file's raw text for whole-config safety scans (Match exec), even
    // when the file has no Host block.
    acc.texts.push(content);
    for item in &cfg.items {
        match item {
            Item::Host(block) => {
                let view = HostView::from_block(block);
                // Shadow is judged on the primary alias (first pattern); the rest
                // of the patterns still register so a later duplicate is caught.
                let shadowed = !seen.insert(view.alias().to_string());
                for alias in block.patterns.iter().skip(1) {
                    seen.insert(alias.clone());
                }
                acc.hosts.push(IncludedHost {
                    origin: file.to_path_buf(),
                    view,
                    shadowed,
                });
            }
            Item::Include(inc) => {
                expand_paths(&inc.paths, base_dir, home, depth + 1, seen, visited, acc);
            }
            _ => {}
        }
    }
}

/// True when `cfg` uses an `Include` form that `ssh -G` honors but [`expand`]
/// does not follow: an `Include` inside a `Host`/`Match` block (conditional), or
/// a quote-spliced `Include` keyword the parser did not classify as an include.
fn has_unfollowed_include(cfg: &SshConfig) -> bool {
    cfg.items.iter().any(|it| match it {
        // Top-level quote-spliced include: kept as a Global option, not Item::Include.
        Item::Global(o) => is_spliced_include_keyword(&o.keyword),
        Item::Host(b) => b.body.iter().any(body_is_include),
        Item::Match(m) => m.body.iter().any(body_is_include),
        _ => false,
    })
}

fn body_is_include(line: &BodyLine) -> bool {
    match line {
        BodyLine::Option(o) => o.is("Include") || is_spliced_include_keyword(&o.keyword),
        _ => false,
    }
}

/// A keyword that de-quotes to `Include` but carries literal quotes — `ssh` honors
/// it via quote-splicing (`"Include"`, `Inc"lude"`), but the parser's exact
/// keyword compare did not recognize it as an include.
fn is_spliced_include_keyword(keyword: &str) -> bool {
    keyword.contains('"') && {
        let dequoted: String = keyword.chars().filter(|&c| c != '"').collect();
        dequoted.eq_ignore_ascii_case("Include")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(crate::secure_fs::temp_name(tag).unwrap());
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // --- #52 AC4: path resolution (pure) — tilde / relative / absolute ---

    #[test]
    fn resolve_absolute_path_unchanged() {
        let base = Path::new("/home/u/.ssh");
        let home = Path::new("/home/u");
        let abs = if cfg!(windows) {
            r"C:\etc\ssh\extra.conf"
        } else {
            "/etc/ssh/extra.conf"
        };
        assert_eq!(resolve_include_arg(abs, base, home), PathBuf::from(abs));
    }

    #[test]
    fn resolve_tilde_expands_to_home() {
        let home = Path::new("/home/u");
        let base = Path::new("/home/u/.ssh");
        // `~/x/y.conf` -> <home>/x/y.conf
        assert_eq!(
            resolve_include_arg("~/x/y.conf", base, home),
            home.join("x").join("y.conf")
        );
    }

    #[test]
    fn resolve_relative_resolves_against_base() {
        let base = Path::new("/home/u/.ssh");
        let home = Path::new("/home/u");
        // A bare relative path resolves against ~/.ssh (OpenSSH semantics).
        assert_eq!(
            resolve_include_arg("work/db.conf", base, home),
            base.join("work").join("db.conf")
        );
    }

    // --- #52 AC1: included hosts surface with origin ---

    #[test]
    fn expand_surfaces_hosts_from_included_file() {
        let dir = temp_dir(".inc-basic");
        let inc = dir.join("extra.conf");
        std::fs::write(&inc, "Host vault\n    HostName 10.0.1.9\n    User me\n").unwrap();
        let main_src = format!(
            "Include {}\n\nHost web\n    HostName 10.0.0.1\n",
            inc.display()
        );
        let main = parser::parse(dir.join("config"), &main_src);

        let hosts = expand(&main, &dir, &dir).hosts;
        assert_eq!(hosts.len(), 1, "the one included host must surface");
        assert_eq!(hosts[0].view.alias(), "vault");
        assert_eq!(hosts[0].view.host_name.as_deref(), Some("10.0.1.9"));
        assert_eq!(
            hosts[0].origin, inc,
            "origin must point at the included file"
        );
        assert!(!hosts[0].shadowed);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- #52 AC4: glob include (config.d/* — the 1Password/Ansible main use case) ---

    #[test]
    fn expand_glob_matches_multiple_files() {
        let dir = temp_dir(".inc-glob");
        let cd = dir.join("config.d");
        std::fs::create_dir_all(&cd).unwrap();
        std::fs::write(cd.join("a.conf"), "Host a\n    HostName 1.1.1.1\n").unwrap();
        std::fs::write(cd.join("b.conf"), "Host b\n    HostName 2.2.2.2\n").unwrap();
        // relative glob include, resolved against base_dir = dir
        let main = parser::parse(dir.join("config"), "Include config.d/*\n");

        let mut hosts = expand(&main, &dir, &dir).hosts;
        hosts.sort_by(|x, y| x.view.alias().cmp(y.view.alias()));
        let aliases: Vec<_> = hosts.iter().map(|h| h.view.alias().to_string()).collect();
        assert_eq!(aliases, vec!["a".to_string(), "b".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expand_relative_include_resolves_against_base() {
        let dir = temp_dir(".inc-rel");
        let sub = dir.join("work");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("db.conf"), "Host db\n    HostName 2.2.2.2\n").unwrap();
        // main uses a RELATIVE include; base_dir=dir stands in for ~/.ssh
        let main = parser::parse(dir.join("config"), "Include work/db.conf\n");

        let hosts = expand(&main, &dir, &dir).hosts;
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].view.alias(), "db");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- #52 AC5: nested includes recurse; cycles terminate; depth is bounded ---

    #[test]
    fn expand_recurses_into_nested_includes() {
        let dir = temp_dir(".inc-nest");
        std::fs::write(dir.join("b.conf"), "Host deep\n    HostName 3.3.3.3\n").unwrap();
        std::fs::write(
            dir.join("a.conf"),
            "Include b.conf\nHost mid\n    HostName 2.2.2.2\n",
        )
        .unwrap();
        let main = parser::parse(dir.join("config"), "Include a.conf\n");

        let mut hosts = expand(&main, &dir, &dir).hosts;
        hosts.sort_by(|x, y| x.view.alias().cmp(y.view.alias()));
        let aliases: Vec<_> = hosts.iter().map(|h| h.view.alias().to_string()).collect();
        assert_eq!(aliases, vec!["deep".to_string(), "mid".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expand_cycle_guard_terminates() {
        let dir = temp_dir(".inc-cycle");
        // a includes b, b includes a — must terminate; each host appears once.
        std::fs::write(
            dir.join("a.conf"),
            "Include b.conf\nHost hosta\n    HostName 1.1.1.1\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("b.conf"),
            "Include a.conf\nHost hostb\n    HostName 2.2.2.2\n",
        )
        .unwrap();
        let main = parser::parse(dir.join("config"), "Include a.conf\n");

        let mut aliases: Vec<_> = expand(&main, &dir, &dir)
            .hosts
            .iter()
            .map(|h| h.view.alias().to_string())
            .collect();
        aliases.sort();
        assert_eq!(aliases, vec!["hosta".to_string(), "hostb".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expand_stops_at_max_depth() {
        let dir = temp_dir(".inc-depth");
        // Linear chain deeper than MAX_DEPTH: f{i} includes f{i+1} and defines h{i}.
        // main is depth 0; files f1..f{MAX_DEPTH} are read, f{MAX_DEPTH+1}+ skipped.
        let chain = MAX_DEPTH + 3;
        for i in 1..=chain {
            let next = if i < chain {
                format!("Include f{}.conf\n", i + 1)
            } else {
                String::new()
            };
            std::fs::write(
                dir.join(format!("f{i}.conf")),
                format!("{next}Host h{i}\n    HostName 10.0.0.{i}\n"),
            )
            .unwrap();
        }
        let main = parser::parse(dir.join("config"), "Include f1.conf\n");

        let mut nums: Vec<usize> = expand(&main, &dir, &dir)
            .hosts
            .iter()
            .filter_map(|h| {
                h.view
                    .alias()
                    .strip_prefix('h')
                    .and_then(|n| n.parse().ok())
            })
            .collect();
        nums.sort();
        assert_eq!(
            nums,
            (1..=MAX_DEPTH).collect::<Vec<_>>(),
            "must include exactly the hosts within MAX_DEPTH include levels"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- #52 AC3: duplicate alias across includes is flagged shadowed (first-wins) ---

    #[test]
    fn expand_flags_duplicate_alias_across_includes() {
        let dir = temp_dir(".inc-shadow");
        std::fs::write(dir.join("a.conf"), "Host dup\n    HostName 1.1.1.1\n").unwrap();
        std::fs::write(
            dir.join("b.conf"),
            "Host dup\n    HostName 2.2.2.2\n\nHost only-b\n    HostName 3.3.3.3\n",
        )
        .unwrap();
        // a.conf is included before b.conf, so OpenSSH first-wins: a's `dup` is
        // effective and b's `dup` is the shadowed duplicate. (A top-level Include
        // always precedes the main hosts, so a main host never shadows an included
        // one — the shadow comes from an earlier include.)
        let main = parser::parse(dir.join("config"), "Include a.conf\nInclude b.conf\n");

        let hosts = expand(&main, &dir, &dir).hosts;
        let dups: Vec<_> = hosts.iter().filter(|h| h.view.alias() == "dup").collect();
        assert_eq!(dups.len(), 2);
        assert!(!dups[0].shadowed, "first occurrence (a.conf) wins");
        assert!(dups[1].shadowed, "second occurrence (b.conf) is shadowed");
        let only = hosts.iter().find(|h| h.view.alias() == "only-b").unwrap();
        assert!(!only.shadowed);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- #52 conservative fail-safe: include forms ssh -G honors but expand cannot
    // follow set `blind_spot`, so the autofill safety gate fails safe ---

    #[test]
    fn blind_spot_on_block_nested_include() {
        let dir = temp_dir(".inc-blind-nested");
        // An Include inside a Host block is conditional; expand can't follow it.
        let main = parser::parse(dir.join("config"), "Host gate\n    Include other.conf\n");
        assert!(expand(&main, &dir, &dir).blind_spot);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blind_spot_on_quote_spliced_include() {
        let dir = temp_dir(".inc-blind-quote");
        // `"Include"` is honored by ssh via quote-splicing but the parser does not
        // classify it as an include directive.
        let main = parser::parse(dir.join("config"), "\"Include\" other.conf\n");
        assert!(expand(&main, &dir, &dir).blind_spot);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blind_spot_on_depth_truncation() {
        let dir = temp_dir(".inc-blind-depth");
        let chain = MAX_DEPTH + 2;
        for i in 1..=chain {
            let next = if i < chain {
                format!("Include f{}.conf\n", i + 1)
            } else {
                String::new()
            };
            std::fs::write(
                dir.join(format!("f{i}.conf")),
                format!("{next}Host h{i}\n    HostName 10.0.0.{i}\n"),
            )
            .unwrap();
        }
        let main = parser::parse(dir.join("config"), "Include f1.conf\n");
        assert!(
            expand(&main, &dir, &dir).blind_spot,
            "recursing past MAX_DEPTH is a blind spot"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn secondary_pattern_shadows_later_duplicate() {
        // A multi-pattern host (`Host primary alt`) registers ALL its aliases, so a
        // later include defining `alt` is flagged shadowed (OpenSSH first-wins).
        let dir = temp_dir(".inc-multi-pat");
        std::fs::write(
            dir.join("a.conf"),
            "Host primary alt\n    HostName 1.1.1.1\n",
        )
        .unwrap();
        std::fs::write(dir.join("b.conf"), "Host alt\n    HostName 2.2.2.2\n").unwrap();
        let main = parser::parse(dir.join("config"), "Include a.conf\nInclude b.conf\n");
        let hosts = expand(&main, &dir, &dir).hosts;
        let alt = hosts.iter().find(|h| h.view.alias() == "alt").unwrap();
        assert!(
            alt.shadowed,
            "a later host duplicating a secondary alias is shadowed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn included_alias_wins_over_later_main_host() {
        // Corrected first-wins semantics: a top-level Include is honored before the
        // main hosts, so an alias defined in BOTH the include and the main file is
        // effective from the include — the included host is NOT shadowed.
        let dir = temp_dir(".inc-main-dup");
        std::fs::write(dir.join("a.conf"), "Host web\n    HostName 1.1.1.1\n").unwrap();
        let main = parser::parse(
            dir.join("config"),
            "Include a.conf\n\nHost web\n    HostName 2.2.2.2\n",
        );
        let hosts = expand(&main, &dir, &dir).hosts;
        let web = hosts.iter().find(|h| h.view.alias() == "web").unwrap();
        assert!(
            !web.shadowed,
            "the included alias precedes the main host, so it wins"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn equals_separated_include_is_followed_not_a_blind_spot() {
        // `Include=path` is a normal top-level include: ssh honors it and so do we,
        // so it is scanned (hosts surface) rather than flagged un-scannable.
        let dir = temp_dir(".inc-equals");
        std::fs::write(dir.join("a.conf"), "Host eq\n    HostName 1.1.1.1\n").unwrap();
        let main = parser::parse(dir.join("config"), "Include=a.conf\n");
        let expansion = expand(&main, &dir, &dir);
        assert!(
            !expansion.blind_spot,
            "an `=`-separated include is followable"
        );
        assert_eq!(expansion.hosts.len(), 1, "its hosts are surfaced");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commented_include_is_not_an_include() {
        // A commented-out `# Include` must neither be followed nor flagged.
        let dir = temp_dir(".inc-commented");
        let main = parser::parse(
            dir.join("config"),
            "# Include other.conf\nHost a\n    HostName 1.1.1.1\n",
        );
        let expansion = expand(&main, &dir, &dir);
        assert!(!expansion.blind_spot);
        assert!(expansion.hosts.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blind_spot_on_unevaluatable_glob_pattern() {
        // The `glob` crate rejects an unbalanced `[` as a pattern ERROR, while the
        // libc `glob(3)` OpenSSH uses treats it as a literal character and matches
        // files. "Could not evaluate" must never be read as "matched nothing", or a
        // `Match exec` in the matched file would go unscanned.
        let dir = temp_dir(".inc-badglob");
        let sub = dir.join("conf[.d");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("evil.conf"), "Match exec \"cmd\"\n").unwrap();
        let main = parser::parse(dir.join("config"), "Include conf[.d/*\n");
        assert!(
            expand(&main, &dir, &dir).blind_spot,
            "a pattern we cannot evaluate must fail safe"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blind_spot_on_unresolvable_include_arg() {
        // Argument shapes ssh expands but we do not (percent tokens, ${ENV}, and
        // the Windows-valid `~\path`) would send us to a different file than ssh
        // opens — fail safe rather than scan the wrong path.
        let dir = temp_dir(".inc-unresolvable");
        for arg in ["%d/.ssh/x.conf", "${HOME}/x.conf", "~\\.ssh\\x.conf"] {
            let main = parser::parse(dir.join("config"), &format!("Include {arg}\n"));
            assert!(
                expand(&main, &dir, &dir).blind_spot,
                "unresolvable include arg {arg:?} must fail safe"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn blind_spot_on_backslash_escaped_include_arg() {
        // `glob(3)` (no GLOB_NOESCAPE, which OpenSSH does not set) unescapes `\x`
        // to `x`, so `evil\.conf` reads `evil.conf`. We take the literal branch and
        // would stat a nonexistent path — silently "clean" — unless flagged.
        let dir = temp_dir(".inc-backslash");
        let cd = dir.join("config.d");
        std::fs::create_dir_all(&cd).unwrap();
        std::fs::write(cd.join("evil.conf"), "Match exec \"cmd\"\n").unwrap();
        let main = parser::parse(dir.join("config"), "Include config.d/evil\\.conf\n");
        assert!(
            expand(&main, &dir, &dir).blind_spot,
            "a backslash-escaped include arg must fail safe on unix"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn double_star_glob_matches_like_posix() {
        // `glob(3)` has no `**` — it is just `*`. The glob crate's component-wise
        // `**` matches NOTHING in a flat directory, which would silently scan no
        // files at all while ssh reads them.
        let dir = temp_dir(".inc-doublestar");
        let cd = dir.join("config.d");
        std::fs::create_dir_all(&cd).unwrap();
        std::fs::write(cd.join("a.conf"), "Host a\n    HostName 1.1.1.1\n").unwrap();
        std::fs::write(cd.join("b.conf"), "Match exec \"cmd\"\n").unwrap();
        let main = parser::parse(dir.join("config"), "Include config.d/**\n");
        let expansion = expand(&main, &dir, &dir);
        assert_eq!(expansion.texts.len(), 2, "`**` must behave like `*`");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blind_spot_on_tilde_user_include_arg() {
        // OpenSSH passes GLOB_TILDE, so `~user/…` expands to that user's home; we
        // would resolve it as a relative path and miss the file entirely.
        let dir = temp_dir(".inc-tildeuser");
        let main = parser::parse(dir.join("config"), "Include ~otheruser/.ssh/x.conf\n");
        assert!(
            expand(&main, &dir, &dir).blind_spot,
            "`~user` is expanded by ssh but not by us — fail safe"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hidden_files_are_not_globbed() {
        // `glob(3)` never matches a leading dot with a wildcard. Scanning one would
        // list phantom hosts (and could refuse autofill) for a file ssh never reads.
        let dir = temp_dir(".inc-hidden");
        let cd = dir.join("config.d");
        std::fs::create_dir_all(&cd).unwrap();
        std::fs::write(
            cd.join(".hidden.conf"),
            "Host ghost\n    HostName 9.9.9.9\n",
        )
        .unwrap();
        std::fs::write(cd.join("real.conf"), "Host real\n    HostName 1.1.1.1\n").unwrap();
        let main = parser::parse(dir.join("config"), "Include config.d/*\n");
        let expansion = expand(&main, &dir, &dir);
        let aliases: Vec<_> = expansion
            .hosts
            .iter()
            .map(|h| h.view.alias().to_string())
            .collect();
        assert_eq!(aliases, vec!["real".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn glob_matching_only_a_directory_is_not_a_blind_spot() {
        // `ssh` cannot read a directory either, so skipping one leaves nothing
        // unscanned — this must NOT over-block the common `config.d/*` setup.
        let dir = temp_dir(".inc-globdir");
        let cd = dir.join("config.d");
        std::fs::create_dir_all(cd.join("subdir")).unwrap();
        std::fs::write(cd.join("a.conf"), "Host a\n    HostName 1.1.1.1\n").unwrap();
        let main = parser::parse(dir.join("config"), "Include config.d/*\n");
        let expansion = expand(&main, &dir, &dir);
        assert!(
            !expansion.blind_spot,
            "a matched directory is skipped cleanly"
        );
        assert_eq!(expansion.hosts.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_include_is_not_a_blind_spot() {
        // A stale/absent include is invisible to ssh too, so it stays fail-soft.
        let dir = temp_dir(".inc-missing");
        let main = parser::parse(dir.join("config"), "Include gone.conf\n");
        assert!(!expand(&main, &dir, &dir).blind_spot);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn include_lookalikes_are_not_treated_as_includes() {
        // False-positive guard (kept from the deleted `has_include` tests): a host
        // named "include-server" and an IdentityFile path containing "Include" are
        // values, not directives — neither is followed nor flagged.
        let dir = temp_dir(".inc-lookalike");
        let main = parser::parse(
            dir.join("config"),
            "Host include-server\n    HostName 1.1.1.1\n    IdentityFile ~/Include/key\n",
        );
        let expansion = expand(&main, &dir, &dir);
        assert!(!expansion.blind_spot);
        assert!(expansion.hosts.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_blind_spot_for_clean_top_level_includes() {
        let dir = temp_dir(".inc-clean");
        std::fs::write(dir.join("a.conf"), "Host a\n    HostName 1.1.1.1\n").unwrap();
        let main = parser::parse(
            dir.join("config"),
            "Include a.conf\n\nHost main\n    HostName 2.2.2.2\n",
        );
        assert!(
            !expand(&main, &dir, &dir).blind_spot,
            "a plain top-level include is fully scannable"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- #52 AC6: unreadable include is skipped (fail-soft), others still load ---

    #[test]
    fn expand_skips_unreadable_include_fail_soft() {
        let dir = temp_dir(".inc-failsoft");
        std::fs::write(dir.join("good.conf"), "Host good\n    HostName 1.1.1.1\n").unwrap();
        // one missing path (skipped), one good path (loaded)
        let main = parser::parse(
            dir.join("config"),
            "Include does-not-exist.conf\nInclude good.conf\n",
        );

        let aliases: Vec<_> = expand(&main, &dir, &dir)
            .hosts
            .iter()
            .map(|h| h.view.alias().to_string())
            .collect();
        assert_eq!(aliases, vec!["good".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expand_returns_empty_when_no_includes() {
        // A config with no Include directives yields no included hosts.
        let dir = temp_dir(".inc-none");
        let main = parser::parse(dir.join("config"), "Host solo\n    HostName 1.1.1.1\n");
        assert!(expand(&main, &dir, &dir).hosts.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
