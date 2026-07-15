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

use crate::config::model::{HostView, Item, SshConfig};
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
    // OpenSSH only expands a leading `~` (not `~user`, which sshm does not support).
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

/// The full result of [`expand`]: the included hosts plus the raw text of every
/// included file that was read. The texts back whole-config safety
/// scans (e.g. `Match exec` detection) across *all* files — including included
/// files that contain no `Host` block and so contribute no [`IncludedHost`].
#[derive(Debug, Clone, Default)]
pub struct Expansion {
    pub hosts: Vec<IncludedHost>,
    pub texts: Vec<String>,
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
    // Seed "seen" with the main file's own aliases: main hosts are listed first,
    // so a duplicate found in an include is the shadowed (non-effective) one.
    let mut seen: HashSet<String> = main
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Host(b) => b.patterns.first().cloned(),
            _ => None,
        })
        .collect();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut acc = Expansion::default();
    for item in &main.items {
        if let Item::Include(inc) = item {
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
        return;
    }
    for arg in paths {
        let resolved = resolve_include_arg(arg, base_dir, home);
        for file in glob_matches(&resolved) {
            read_included_file(&file, base_dir, home, depth, seen, visited, acc);
        }
    }
}

/// Expand a resolved include path into concrete files. A path containing glob
/// metacharacters (`* ? [`) is globbed (existing matches, alphabetical); a
/// literal path is returned as-is (existence is checked when it is read, so a
/// missing literal include is skipped fail-soft). An invalid glob pattern yields
/// nothing (fail-soft).
fn glob_matches(resolved: &Path) -> Vec<PathBuf> {
    let s = resolved.to_string_lossy();
    if s.contains(['*', '?', '[']) {
        match glob::glob(&s) {
            Ok(paths) => paths.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
    } else {
        vec![resolved.to_path_buf()]
    }
}

/// Read and project one included file's hosts, recursing into its own includes.
/// The canonical path guards cycles; an unreadable/missing file (or a directory
/// matched by a glob) is skipped fail-soft.
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
    let Ok(content) = std::fs::read_to_string(file) else {
        return; // fail-soft: missing / unreadable / a directory
    };
    let cfg = parser::parse(file.to_path_buf(), &content);
    // Keep the file's raw text for whole-config safety scans (Match exec), even
    // when the file has no Host block.
    acc.texts.push(content);
    for item in &cfg.items {
        match item {
            Item::Host(block) => {
                let view = HostView::from_block(block);
                let shadowed = !seen.insert(view.alias().to_string());
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

    // --- #52 AC3: duplicate alias across files is flagged shadowed (first-wins) ---

    #[test]
    fn expand_flags_alias_shadowed_by_main() {
        let dir = temp_dir(".inc-shadow");
        std::fs::write(
            dir.join("extra.conf"),
            "Host web\n    HostName 9.9.9.9\n\nHost only-inc\n    HostName 8.8.8.8\n",
        )
        .unwrap();
        // main defines `web`, so the included `web` is a shadowed duplicate.
        // (Include must be top-level — before the first Host block — to be an
        // unconditional include; inside a block it is a conditional include,
        // which read-only v1 intentionally does not expand.)
        let main = parser::parse(
            dir.join("config"),
            "Include extra.conf\n\nHost web\n    HostName 1.1.1.1\n",
        );

        let hosts = expand(&main, &dir, &dir).hosts;
        let web = hosts.iter().find(|h| h.view.alias() == "web").unwrap();
        assert!(web.shadowed, "alias also defined in main must be shadowed");
        let only = hosts.iter().find(|h| h.view.alias() == "only-inc").unwrap();
        assert!(!only.shadowed, "unique included alias is not shadowed");

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
