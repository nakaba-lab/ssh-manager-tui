# SFTP multiple selection acceptance

Run `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`
and `cargo test --all` on Linux and Windows. The opt-in integration test only uses
temporary files with a local OpenSSH protocol server:

```sh
SSHM_TEST_SFTP_SERVER=/usr/lib/openssh/sftp-server cargo test batch_worker_real_sftp_partial_failure -- --ignored
```

On Windows, set `$env:SSHM_TEST_SFTP_SERVER` to your test `sftp-server.exe` and run
the same cargo test. To test SSH transport instead of `-D`, additionally set
`SSHM_TEST_SFTP_CONFIG` to a throwaway config containing the alias `sshm-test`.
Both sides must access the same temporary directory (localhost test server).
The protocol test covers a success, a missing middle file, and another success;
failed downloads leave the existing destination untouched.

## Windows OpenSSH Agent setup

Use a disposable Windows VM/user and a temporary SSH server, never production.
Install Windows OpenSSH Client. In an elevated PowerShell, if the agent is disabled:

```powershell
Set-Service ssh-agent -StartupType Manual
Start-Service ssh-agent
```

In the test user's PowerShell, generate a dedicated test key and add it to the
Windows agent. Put its public key in the temporary server's authorized_keys:

```powershell
$testKey = Join-Path $env:USERPROFILE '.ssh\sshm-test'
ssh-keygen -t ed25519 -f $testKey
ssh-add $testKey
ssh-add -l
```

Add a unique test alias to that disposable user's `~/.ssh/config`:

```sshconfig
Host sshm-acceptance
    HostName TEST_SERVER
    User TEST_USER
    IdentityFile ~/.ssh/sshm-test.pub
    IdentitiesOnly yes
```

Confirm the server host key interactively with `ssh sshm-acceptance` before opening
sshm. Move the private key out of its original pathname after loading it into the
agent, so the test demonstrates agent-based signing with a `.pub` IdentityFile.
Restore/remove only these test files afterward, and unload the test key with ssh-add.

## Manual checklist (repeat on Windows and Linux)

- [ ] Open `b`, transfer one unmarked file with Enter; existing overwrite prompt works.
- [ ] Mark three files with Insert: each marked row shows `*` and a full-width fill, the cursor advances one row, and the count updates. Toggle a mark off and check the cursor advances again.
- [ ] `*` marks files and real directories, but not `..`, symlinks or special files.
- [ ] Enter copies all marked files and directories sequentially in both directions.
- [ ] Existing destinations prompt individually; declining preserves the mark.
- [ ] Make one source unreadable or remove it after marking: the other files finish,
      successful files unmark, failures remain marked, counts are correct. Move the
      cursor onto each failed file to see its diagnostic.
- [ ] During a slow transfer move the cursor, switch panes and open help; UI responds.
      Directory navigation and closing wait until the batch completes.
- [ ] Esc clears both sets first; navigating away clears that directory's marks.
- [ ] Authentication failure stops remaining attempts; a retry requires an explicit Enter.
- [ ] Names containing spaces and Unicode transfer correctly.
- [ ] Windows Agent + public IdentityFile + IdentitiesOnly works without a private key
      at the IdentityFile pathname.

Checked during implementation: Linux tests, local protocol integration, real localhost
sshd with ssh-agent/public IdentityFile, and Windows-target compilation/Clippy.
Windows runtime and interactive TUI acceptance remain unchecked.
