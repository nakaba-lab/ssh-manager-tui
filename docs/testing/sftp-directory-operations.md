# SFTP browser directory operations

Use disposable local and remote folders. In the browser, Tab chooses the pane.

- [ ] Press F7 in each pane, enter a directory name with spaces and Unicode, and check it appears there. Esc cancels the name prompt.
- [ ] Press n in each pane, enter a filename, and check a zero-byte file appears. An existing name must be refused.
- [ ] Select a directory and press F6 in each pane; check the old name disappears and contents appear under the new name. Rename to an existing name must be refused.
- [ ] Select a local directory with nested files, press F5, and check the full tree on the remote side. Repeat remote to local. An existing destination must be refused without changing it.
- [ ] Select one file and press F5 on each pane; check it transfers to the other pane and asks before overwrite. During a large transfer, the browser remains visible and shows elapsed time instead of printing `sftp>` to the console. A download also shows copied bytes and percent.
- [ ] Mark a folder with Insert and check its `*`, row fill, selected count and cursor moving down. Mark files and folders with `*`, then press c; check they copy sequentially in both directions. `..` and symlinks must not be marked.
- [ ] Select a nonempty directory and press F8. Cancel the confirmation, then accept it; verify all nested contents are removed. Repeat on both panes. Check remote symlinks in the tree are removed as links without changing their targets.
- [ ] Select a file and press F8; verify confirmation and removal on both panes.
- [ ] While a long copy/delete runs, the UI still renders and does not start another mutating operation. A failed transfer reports the error; the source remains in place.

The remote operations use the existing system OpenSSH sftp client and browser authentication. Recursive copies stage under a temporary name before the final rename. A failed remote upload can leave a staged .sshm-part-* directory; the error names it. Deleting a directory is recursive and irreversible after confirmation. The tool never traverses symlinks while collecting a remote deletion, but SFTP has no atomic tree snapshot, so other writers can race an operation.

The letter aliases are `e` edit, `c` copy, `r` rename, `m` mkdir, `d` delete, and `n` new file. `Shift+R` refreshes the focused pane. `C`, `N`, and `Delete` also remain available.

Opt-in local protocol check:

```sh
SSHM_TEST_SFTP_SERVER=/usr/lib/openssh/sftp-server cargo test browser_directory_management_real_sftp -- --ignored
```
