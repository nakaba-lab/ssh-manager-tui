---
name: scratch-config
description: Create a throwaway ~/.ssh/config-style file and run sshm against it, so manual testing of parse/edit/save behavior never touches the real config. Use whenever you are about to manually test sshm's listing, editing, or saving — sshm writes to whatever --config path it is given, so it must be a scratch file.
---

# scratch-config

`sshm` reads **and writes** the real `~/.ssh/config`. The defining rule for
manual testing (CLAUDE.md, README): **always point `--config` at a throwaway
file** — the app writes to whatever path it is given. This skill scaffolds that
throwaway file and exercises it.

Use the scratch path **`target/scratch/ssh_config`** — `target/` is already
git-ignored, so the throwaway can never be committed and never collides with the
real config.

## Steps

1. **Create the scratch config** at `target/scratch/ssh_config` with a
   representative sample that covers the form fields and the round-trip edge
   cases (extras, a ProxyJump host, a forward, a bare Windows IdentityFile path,
   mixed keyword casing, a comment, and a blank line). Write exactly:

   ```text
   # scratch config for manual sshm testing — safe to delete

   Host bastion
       HostName bastion.example.com
       User jump

   Host web-prod
       HostName 10.0.1.20
       User deploy
       Port 22
       ProxyJump bastion
       IdentityFile ~/.ssh/id_ed25519
       LocalForward 8080 localhost:80

   host WIN-box
       HostName 192.168.1.50
       IdentityFile C:\Users\me\.ssh\id_rsa
       ServerAliveInterval 30
   ```

   (The lowercase `host`, the `ServerAliveInterval` extra, and the bare
   `C:\...` path are deliberate — they exercise casing preservation, the
   `extras` round-trip, and the no-backslash-escape rule.)

2. **Verify non-interactively** — confirm parsing works without launching the
   TUI:

   ```sh
   cargo run -- --config target/scratch/ssh_config --list
   ```

3. **Hand off the interactive command.** The TUI is interactive and can't be
   driven from here, so print this for the user to run and observe edit/save
   behavior live:

   ```sh
   cargo run -- --config target/scratch/ssh_config
   ```

4. **(Optional) Round-trip check.** After the user edits & saves in the TUI,
   diff against a pristine copy to confirm the lossless invariant held for
   untouched lines — e.g. keep a `target/scratch/ssh_config.orig` copy and
   `diff` it.

## Never

- Never run `sshm` (or `cargo run`) **without** `--config <scratch path>` while
  testing save behavior — the default target is the real `~/.ssh/config`.
- Never point `--config` at the user's real `~/.ssh/config` to "try an edit".
