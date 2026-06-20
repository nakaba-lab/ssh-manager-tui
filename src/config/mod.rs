//! `~/.ssh/config` domain: lossless parse + surgical round-trip write.
//!
//! The submodules have **zero** ratatui dependency, so the whole layer is
//! unit-testable headless. See [`parser`] for the parse side and [`writer`] for
//! rendering + editing.

pub mod model;
pub mod parser;
pub mod tokens;
pub mod writer;

use std::path::PathBuf;

use crate::error::ConfigError;

pub use model::{HostBlock, HostView, Item, SshConfig};

impl SshConfig {
    /// Load and parse a config file. A missing file yields an empty document
    /// bound to `path` (so the first save creates it).
    pub fn load(path: PathBuf) -> Result<SshConfig, ConfigError> {
        match std::fs::read_to_string(&path) {
            Ok(content) => Ok(parser::parse(path, &content)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SshConfig {
                path,
                ..Default::default()
            }),
            Err(source) => Err(ConfigError::Io { path, source }),
        }
    }

    /// Editable projections of every Host block, paired with their item index.
    pub fn host_views(&self) -> Vec<(usize, HostView)> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| match it {
                Item::Host(b) => Some((i, HostView::from_block(b))),
                _ => None,
            })
            .collect()
    }

    /// Number of `Include` directives (their hosts are not parsed in v1).
    pub fn include_count(&self) -> usize {
        self.items
            .iter()
            .filter(|it| matches!(it, Item::Include(_)))
            .count()
    }

    /// Mutable access to the Host block at `item_index`, if it is one.
    pub fn host_block_mut(&mut self, item_index: usize) -> Option<&mut HostBlock> {
        match self.items.get_mut(item_index) {
            Some(Item::Host(b)) => Some(b),
            _ => None,
        }
    }

    /// Apply an edited [`HostView`] back onto the block at `item_index`,
    /// surgically (only changed lines are rewritten).
    pub fn apply_view(&mut self, item_index: usize, view: &HostView) -> Result<(), ConfigError> {
        let Some(block) = self.host_block_mut(item_index) else {
            return Err(ConfigError::Validation {
                field: "host".into(),
                reason: "target is not a host block".into(),
            });
        };
        // Only rewrite the header when patterns actually change, so unedited
        // headers keep their original spacing.
        if view.patterns != block.patterns {
            writer::set_patterns(block, &view.patterns);
        }
        writer::set_single(block, "HostName", view.host_name.as_deref(), true);
        writer::set_single(block, "User", view.user.as_deref(), true);
        writer::set_single(block, "Port", view.port.as_deref(), true);
        writer::set_multi(block, "IdentityFile", &view.identity_files, true);
        writer::set_single(block, "ProxyJump", view.proxy_jump.as_deref(), true);
        writer::set_multi(block, "LocalForward", &view.local_forwards, false);
        writer::set_multi(block, "RemoteForward", &view.remote_forwards, false);
        writer::set_multi(block, "DynamicForward", &view.dynamic_forwards, false);
        writer::set_extras(block, &view.extras);
        self.dirty = true;
        Ok(())
    }

    /// Append a brand-new host block built from `view`. Returns its item index.
    pub fn add_host(&mut self, view: &HostView) -> Result<usize, ConfigError> {
        if view.alias().trim().is_empty() {
            return Err(ConfigError::Validation {
                field: "Host".into(),
                reason: "alias must not be empty".into(),
            });
        }
        if self.alias_exists(view.alias()) {
            return Err(ConfigError::DuplicateAlias(view.alias().to_string()));
        }

        writer::ensure_trailing_separator(self);
        let mut block = writer::new_host_block(&view.patterns);
        // Populate the body via the same surgical setters used for edits.
        writer::set_single(&mut block, "HostName", view.host_name.as_deref(), true);
        writer::set_single(&mut block, "User", view.user.as_deref(), true);
        writer::set_single(&mut block, "Port", view.port.as_deref(), true);
        writer::set_multi(&mut block, "IdentityFile", &view.identity_files, true);
        writer::set_single(&mut block, "ProxyJump", view.proxy_jump.as_deref(), true);
        writer::set_multi(&mut block, "LocalForward", &view.local_forwards, false);
        writer::set_multi(&mut block, "RemoteForward", &view.remote_forwards, false);
        writer::set_multi(&mut block, "DynamicForward", &view.dynamic_forwards, false);
        writer::set_extras(&mut block, &view.extras);

        let idx = self.items.len();
        self.items.push(Item::Host(block));
        self.trailing_newline = true;
        self.dirty = true;
        Ok(idx)
    }

    /// Delete the host block at `item_index`, claiming its owned preceding
    /// comments (`pre`) and collapsing any double blank left behind.
    pub fn delete_host(&mut self, item_index: usize) -> Result<(), ConfigError> {
        if !matches!(self.items.get(item_index), Some(Item::Host(_))) {
            return Err(ConfigError::Validation {
                field: "host".into(),
                reason: "target is not a host block".into(),
            });
        }
        self.items.remove(item_index);

        // Remove one adjacent blank separator: prefer the one that followed the
        // block, else the one that preceded it. This keeps spacing balanced
        // without disturbing blocks elsewhere in the file.
        if item_index < self.items.len() && matches!(self.items[item_index], Item::Blank(_)) {
            self.items.remove(item_index);
        } else if item_index > 0 && matches!(self.items[item_index - 1], Item::Blank(_)) {
            self.items.remove(item_index - 1);
        }
        self.dirty = true;
        Ok(())
    }

    fn alias_exists(&self, alias: &str) -> bool {
        self.items.iter().any(|it| match it {
            Item::Host(b) => b.patterns.iter().any(|p| p == alias),
            _ => false,
        })
    }
}

/// Default config path: `~/.ssh/config`.
pub fn default_config_path() -> Result<PathBuf, ConfigError> {
    let home = dirs::home_dir().ok_or(ConfigError::NoHome)?;
    Ok(home.join(".ssh").join("config"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trips(s: &str) {
        let cfg = parser::parse(PathBuf::from("test"), s);
        assert_eq!(cfg.render(), s, "round-trip mismatch for:\n{s:?}");
    }

    #[test]
    fn empty_file_zero_items() {
        let cfg = parser::parse(PathBuf::from("c"), "");
        assert!(cfg.items.is_empty());
        assert_eq!(cfg.render(), "");
    }

    #[test]
    fn roundtrip_basic() {
        round_trips("Host web1\n    HostName 10.0.0.1\n    User deploy\n    Port 22\n");
    }

    #[test]
    fn roundtrip_no_trailing_newline() {
        round_trips("Host web1\n    HostName 10.0.0.1");
    }

    #[test]
    fn roundtrip_crlf() {
        round_trips("Host web1\r\n    HostName 10.0.0.1\r\n    User deploy\r\n");
    }

    #[test]
    fn roundtrip_tabs_indent() {
        round_trips("Host web1\n\tHostName 10.0.0.1\n\tUser deploy\n");
    }

    #[test]
    fn roundtrip_equals_separators() {
        round_trips("Host x\n    Port=22\n    HostName = example.com\n    User =git\n");
    }

    #[test]
    fn roundtrip_multiple_patterns_wildcard_negation() {
        round_trips("Host web1 web* !web9 prod\n    HostName 10.0.0.1\n");
    }

    #[test]
    fn roundtrip_repeated_identityfile_and_forwards() {
        round_trips(
            "Host bastioned\n    HostName 1.2.3.4\n    IdentityFile ~/.ssh/a\n    IdentityFile ~/.ssh/b\n    LocalForward 8080 localhost:80\n    RemoteForward 9090 localhost:90\n    DynamicForward 1080\n    ProxyJump jump.example.com\n",
        );
    }

    #[test]
    fn roundtrip_comments_and_blanks() {
        round_trips(
            "# global note\n\nHost a\n    # inner comment\n    HostName 1.1.1.1\n\n# describes b\nHost b\n    HostName 2.2.2.2\n",
        );
    }

    #[test]
    fn roundtrip_match_block() {
        round_trips("Match host x user y\n    ForwardAgent yes\n\nHost a\n    HostName 1.1.1.1\n");
    }

    #[test]
    fn roundtrip_include() {
        round_trips("Include ~/.ssh/conf.d/*\n\nHost a\n    HostName 1.1.1.1\n");
    }

    #[test]
    fn roundtrip_quoted_windows_path() {
        round_trips("Host a\n    IdentityFile \"C:\\path with space\\id\"\n    HostName 1.1.1.1\n");
    }

    #[test]
    fn roundtrip_trailing_whitespace() {
        round_trips("Host a\n    HostName 1.1.1.1   \n");
    }

    #[test]
    fn roundtrip_bom_stripped() {
        let s = "\u{feff}Host a\n    HostName 1.1.1.1\n";
        let cfg = parser::parse(PathBuf::from("c"), s);
        assert!(cfg.had_bom);
        // BOM intentionally NOT re-added.
        assert_eq!(cfg.render(), "Host a\n    HostName 1.1.1.1\n");
    }

    #[test]
    fn surgical_edit_touches_only_one_field() {
        let src = "Host a\n    HostName old.example.com\n    User deploy\n    Port 22\n";
        let mut cfg = parser::parse(PathBuf::from("c"), src);
        let (idx, mut view) = cfg.host_views().into_iter().next().unwrap();
        view.host_name = Some("new.example.com".into());
        cfg.apply_view(idx, &view).unwrap();
        assert_eq!(
            cfg.render(),
            "Host a\n    HostName new.example.com\n    User deploy\n    Port 22\n"
        );
    }

    #[test]
    fn surgical_edit_preserves_equals_style() {
        let src = "Host a\n    Port=2222\n";
        let mut cfg = parser::parse(PathBuf::from("c"), src);
        let (idx, mut view) = cfg.host_views().into_iter().next().unwrap();
        assert_eq!(view.port.as_deref(), Some("2222"));
        view.port = Some("2200".into());
        cfg.apply_view(idx, &view).unwrap();
        assert_eq!(cfg.render(), "Host a\n    Port=2200\n");
    }

    #[test]
    fn add_first_host_to_empty() {
        let mut cfg = parser::parse(PathBuf::from("c"), "");
        let view = HostView {
            patterns: vec!["web1".into()],
            host_name: Some("10.0.0.1".into()),
            user: Some("deploy".into()),
            ..Default::default()
        };
        cfg.add_host(&view).unwrap();
        assert_eq!(
            cfg.render(),
            "Host web1\n    HostName 10.0.0.1\n    User deploy\n"
        );
    }

    #[test]
    fn add_host_rejects_duplicate_alias() {
        let mut cfg = parser::parse(PathBuf::from("c"), "Host web1\n    HostName 1.1.1.1\n");
        let view = HostView {
            patterns: vec!["web1".into()],
            ..Default::default()
        };
        assert!(matches!(
            cfg.add_host(&view),
            Err(ConfigError::DuplicateAlias(_))
        ));
    }

    #[test]
    fn delete_middle_host_no_double_blank() {
        let src = "Host a\n    HostName 1.1.1.1\n\nHost b\n    HostName 2.2.2.2\n\nHost c\n    HostName 3.3.3.3\n";
        let mut cfg = parser::parse(PathBuf::from("c"), src);
        // Find item index of host "b".
        let b_idx = cfg
            .items
            .iter()
            .position(|it| matches!(it, Item::Host(h) if h.patterns == ["b"]))
            .unwrap();
        cfg.delete_host(b_idx).unwrap();
        assert_eq!(
            cfg.render(),
            "Host a\n    HostName 1.1.1.1\n\nHost c\n    HostName 3.3.3.3\n"
        );
    }

    #[test]
    fn delete_claims_owned_preceding_comment() {
        let src = "Host a\n    HostName 1.1.1.1\n\n# describes b\nHost b\n    HostName 2.2.2.2\n";
        let mut cfg = parser::parse(PathBuf::from("c"), src);
        let b_idx = cfg
            .items
            .iter()
            .position(|it| matches!(it, Item::Host(h) if h.patterns == ["b"]))
            .unwrap();
        cfg.delete_host(b_idx).unwrap();
        // "# describes b" was the block's `pre`, so it goes with the block.
        assert_eq!(cfg.render(), "Host a\n    HostName 1.1.1.1\n");
    }

    #[test]
    fn save_lifecycle_on_disk() {
        // Exercise the real atomic-save path: add → save → reload → edit → save
        // → reload → delete → save → reload, plus the one-time .bak backup.
        let dir = std::env::temp_dir().join(format!(
            "ssh-manager-test-{}-{}",
            std::process::id(),
            "lifecycle"
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("bak"));

        // Seed an existing file so the .bak backup has something to copy.
        std::fs::write(&path, "Host seed\n    HostName seed.example.com\n").unwrap();

        // Add a host and save.
        let mut cfg = SshConfig::load(path.clone()).unwrap();
        cfg.add_host(&HostView {
            patterns: vec!["web1".into()],
            host_name: Some("10.0.0.1".into()),
            user: Some("deploy".into()),
            ..Default::default()
        })
        .unwrap();
        cfg.save().unwrap();
        assert!(path.with_extension("bak").exists(), ".bak must be written");

        // Reload and confirm both hosts are present.
        let cfg = SshConfig::load(path.clone()).unwrap();
        let views = cfg.host_views();
        assert_eq!(views.len(), 2);
        assert_eq!(views[1].1.alias(), "web1");
        assert_eq!(views[1].1.host_name.as_deref(), Some("10.0.0.1"));

        // Edit the added host, save, reload.
        let mut cfg = cfg;
        let (idx, mut view) = (views[1].0, views[1].1.clone());
        view.port = Some("2222".into());
        cfg.apply_view(idx, &view).unwrap();
        cfg.save().unwrap();
        let cfg = SshConfig::load(path.clone()).unwrap();
        assert_eq!(cfg.host_views()[1].1.port.as_deref(), Some("2222"));

        // Delete the seed host, save, reload.
        let mut cfg = cfg;
        let seed_idx = cfg
            .items
            .iter()
            .position(|it| matches!(it, Item::Host(h) if h.patterns == ["seed"]))
            .unwrap();
        cfg.delete_host(seed_idx).unwrap();
        cfg.save().unwrap();
        let cfg = SshConfig::load(path.clone()).unwrap();
        let views = cfg.host_views();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].1.alias(), "web1");

        // Cleanup.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_roundtrips_content_to_a_fresh_path() {
        let src = "Host web1\n    HostName 10.0.0.1\n    User deploy\n";
        let dir = std::env::temp_dir().join(crate::secure_fs::temp_name(".cfgtest").unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("config");
        let mut cfg = parser::parse(target.clone(), src);
        cfg.save().unwrap();
        let written = std::fs::read_to_string(&target).unwrap();
        assert_eq!(written, src, "save must be byte-for-byte lossless");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn saved_config_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(crate::secure_fs::temp_name(".cfgperm").unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("config");
        let mut cfg = parser::parse(target.clone(), "Host a\n");
        cfg.save().unwrap();
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Exercises the OVERWRITE/swap branch the rewrite introduces (B3): the one-time
    // .bak, the rename/ReplaceFileW swap, and the .bak owner-only perm.
    #[test]
    fn save_overwrite_creates_owner_only_bak_and_swaps() {
        let dir = std::env::temp_dir().join(crate::secure_fs::temp_name(".cfgswap").unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("config");
        // v1: create + first save (no .bak yet, path did not exist).
        let mut cfg = parser::parse(target.clone(), "Host a\n");
        cfg.save().unwrap();
        // v2: re-parse from the now-existing target, edit, save again -> .bak + swap.
        let v2 = "Host a\n    HostName 2.2.2.2\n";
        let mut cfg2 = parser::parse(target.clone(), v2);
        cfg2.save().unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), v2);
        let bak = target.with_extension("bak");
        assert!(
            bak.exists(),
            "one-time session .bak must exist after overwrite"
        );
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            "Host a\n",
            ".bak holds the pre-edit content"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&bak).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, ".bak must be owner-only");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_extra_option_is_persisted() {
        // Regression for B1: editing/removing an "extra" option on an existing
        // host must round-trip to the file, not be silently dropped.
        let src =
            "Host a\n    HostName 1.1.1.1\n    ServerAliveInterval 60\n    ForwardAgent yes\n";
        let mut cfg = parser::parse(PathBuf::from("c"), src);
        let (idx, mut view) = cfg.host_views().into_iter().next().unwrap();
        assert_eq!(
            view.extras,
            vec![
                ("ServerAliveInterval".to_string(), "60".to_string()),
                ("ForwardAgent".to_string(), "yes".to_string())
            ]
        );
        // Change one extra, drop the other.
        view.extras = vec![("ServerAliveInterval".to_string(), "120".to_string())];
        cfg.apply_view(idx, &view).unwrap();
        assert_eq!(
            cfg.render(),
            "Host a\n    HostName 1.1.1.1\n    ServerAliveInterval 120\n"
        );
    }

    #[test]
    fn add_host_with_extra_option() {
        let mut cfg = parser::parse(PathBuf::from("c"), "");
        cfg.add_host(&HostView {
            patterns: vec!["a".into()],
            host_name: Some("1.1.1.1".into()),
            extras: vec![("ServerAliveInterval".into(), "30".into())],
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            cfg.render(),
            "Host a\n    HostName 1.1.1.1\n    ServerAliveInterval 30\n"
        );
    }

    #[test]
    fn edit_unrelated_field_preserves_header_spacing() {
        // Regression for B5: editing HostName must not reformat the Host header.
        let src = "Host  web1   web2  !web9\n    HostName old\n";
        let mut cfg = parser::parse(PathBuf::from("c"), src);
        let (idx, mut view) = cfg.host_views().into_iter().next().unwrap();
        view.host_name = Some("new".into());
        cfg.apply_view(idx, &view).unwrap();
        assert_eq!(cfg.render(), "Host  web1   web2  !web9\n    HostName new\n");
    }

    #[test]
    fn set_multi_appends_and_trims() {
        let src = "Host a\n    IdentityFile ~/.ssh/a\n    IdentityFile ~/.ssh/b\n";
        let mut cfg = parser::parse(PathBuf::from("c"), src);
        let (idx, mut view) = cfg.host_views().into_iter().next().unwrap();
        assert_eq!(view.identity_files, ["~/.ssh/a", "~/.ssh/b"]);
        // Drop one, add one.
        view.identity_files = vec!["~/.ssh/a".into(), "~/.ssh/c".into(), "~/.ssh/d".into()];
        cfg.apply_view(idx, &view).unwrap();
        assert_eq!(
            cfg.render(),
            "Host a\n    IdentityFile ~/.ssh/a\n    IdentityFile ~/.ssh/c\n    IdentityFile ~/.ssh/d\n"
        );
    }
}
