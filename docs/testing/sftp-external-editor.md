# External editor acceptance

Run formatter, all tests, and Clippy with `-D warnings` on both Windows and Linux.
The opt-in local protocol integration creates only temporary files:

```sh
SSHM_TEST_SFTP_SERVER=/usr/lib/openssh/sftp-server cargo test external_editor_real_sftp_roundtrip_conflict_and_modes -- --ignored
```

Windows: set `$env:SSHM_TEST_SFTP_SERVER` to the test `sftp-server.exe`. Optionally
set `SSHM_TEST_SFTP_CONFIG` to a throwaway SSH config with alias `sshm-test` to test
SSH transport instead of `-D`; both endpoints must access the same temporary directory.

Use a disposable Windows VM/user and temporary SSH server. Load a dedicated test
key with Windows `ssh-add`, put its public half on the server, and configure an alias
in that test user's `~/.ssh/config` with `IdentityFile ~/.ssh/sshm-test.pub` and
`IdentitiesOnly yes`. Confirm the host key interactively once. Move the private half
away after loading it to prove signing uses the agent. Do not use production keys.

Configure a blocking editor for the current PowerShell session:

```powershell
$env:EDITOR = '"C:\Program Files\Notepad++\notepad++.exe" -multiInst -nosession'
# Or a native executable that supports --wait:
# $env:EDITOR = '"C:\Program Files\Microsoft VS Code\Code.exe" --wait'
```

`VISUAL` overrides `EDITOR`; clear an existing VISUAL when testing the fallback.
Shell wrappers (`.cmd`, `.bat`), shell expansion and editors that detach immediately
are not the supported path. Use an executable and its blocking option.

- [ ] On the remote pane, F4 and `e` each open the cursor file; local-pane, directory
      and symlink selections do not start an editor.
- [ ] Save and close the editor: the original remote path contains the new bytes.
- [ ] Close without changes: no upload; the unique temporary directory disappears.
- [ ] Both VISUAL and EDITOR unset: a clear setup message appears.
- [ ] Editor executable paths and file names with spaces/Unicode work. Also test
      quotes in remote Unix filenames; the local copy has a Windows-safe spelling.
- [ ] While the editor is open, modify the server file: saving asks before overwrite.
      Decline: server contents stay unchanged and local recovery path remains visible.
- [ ] Change the server file again while confirmation is open: accepting asks again.
- [ ] Stop the test server or revoke write permission before saving: local changes
      remain accessible at the displayed path. Restore access and manually recover.
- [ ] Remote POSIX permission bits survive; failure during rename reports the
      `.sshm-part-*` / `.sshm-bak-*` recovery paths.
- [ ] A missing editor/nonzero exit restores the TUI and keeps the downloaded copy.
- [ ] Windows OpenSSH Agent + public IdentityFile + IdentitiesOnly works.

The conflict check is a byte comparison, not an atomic compare-and-swap. Another
writer can race the final check; ownership, ACLs and xattrs are not guaranteed.

Checked during implementation: Linux tests, local OpenSSH integration, real localhost
sshd with agent/public IdentityFile, Windows-target compilation and Clippy.
Windows runtime and interactive TUI acceptance remain unchecked.

Windows permissions regression: test against a Unix SFTP server with a file at mode
0640. Windows OpenSSH may print `-rw-******` for `ls -l /path/file`; the editor
now uses `cd /path` followed by bare `ls -la` and matches the exact filename to
read the server directory listing. F4 must open the file, and saving must keep
0640 (including group/other bits). Test a dotfile and a name containing spaces.
If even the directory listing hides permission bits, editing stops with the
actual permission token in the error; unknown bits must never become zero.
