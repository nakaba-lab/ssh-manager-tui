---
name: release
description: Cut a new sshm release end-to-end across all four channels — version bump, quality gates, tag/CI build, GitHub release notes, crates.io publish, Scoop bucket, and winget. User-invoked only; has irreversible side effects (tag push, GitHub release, cargo publish, winget PR).
disable-model-invocation: true
---

# release runbook (end-to-end, all 4 channels)

Cuts a tagged release of `sshm` and propagates it to **GitHub Releases,
crates.io, Scoop, and winget**. **Never run unprompted.** Several steps are
irreversible (pushing a tag, the GitHub release, `cargo publish`, the winget
PR) — keep going once the user has said "release", but state each irreversible
action as you take it.

Distribution facts (see the `distribution-setup` memory):

- Crate is **`sshm-tui`** on crates.io (the bare `sshm` was taken); the binary
  stays **`sshm`** (`[[bin]]` in `Cargo.toml`).
- Version lives in `Cargo.toml`; tag is `vX.Y.Z`; `.github/workflows/release.yml`
  fires on `v*` tags and builds 4 targets, attaching `<asset>` + `<asset>.sha256`.
- Asset base name: `sshm-X.Y.Z-<target>` (version has **no** leading `v`); the
  download **path** segment is `vX.Y.Z` (with `v`).
- Windows asset: `sshm-X.Y.Z-x86_64-pc-windows-msvc.zip`, containing `sshm.exe`
  at the archive root.
- Scoop = dedicated bucket repo **`nakaba-lab/scoop-bucket`** (`bucket/sshm-tui.json`).
- winget = **`NakabaLab.sshm`**; submissions go through the fork
  **`nakata5577/winget-pkgs`** to `microsoft/winget-pkgs`.

> **crates.io token policy (user decision):** publish runs **locally** with the
> token already saved in `~/.cargo/credentials.toml`. Do **NOT** put the token in
> GitHub Actions / Secrets and do **NOT** add a CI publish step — the user
> rejected that over leakage risk. Keep `cargo publish` a local step.

> **Shell:** command blocks are POSIX (run them in **Git Bash**) unless marked.
> On this Windows-first machine `mktemp`/`sha256sum`/`unzip` exist only in Git
> Bash, not PowerShell (PS equivalents: `Test-Path`, `Get-FileHash -Algorithm
> SHA256`, `Expand-Archive`).

## 0. Preflight

- Working tree clean, on `main`, up to date with origin (`git pull --ff-only`).
- **Repo-local git identity** — the global config leaks a corporate email. Must
  commit as `nakata5577` with the GitHub noreply address:
  ```sh
  git config user.name    # expect: nakata5577
  git config user.email   # expect: 6749435+nakata5577@users.noreply.github.com
  ```
  If unset, set it **repo-local** (never `--global`).
- `gh auth status` ok. Confirm the crates.io token is present (existence only):
  `ls ~/.cargo/credentials.toml`. If absent, the user must `cargo login` in a
  real terminal first.

## 1. Version bump

- Pick the new SemVer `X.Y.Z`. Edit `version` in `Cargo.toml`.
- `cargo build` so `Cargo.lock` records the new version (`grep -A1 'name = "sshm-tui"' Cargo.lock`).

## 2. Quality gates (mirror CI exactly — all must pass)

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all
```

The Linux-only `dead_code` trap on `#[cfg(windows)]`-only symbols can pass on a
local Windows build yet fail CI — if a release touched `os/` or cfg-gated code,
dispatch the `windows-first-reviewer` subagent.

## 3. Release build sanity

```sh
cargo build --release
target/release/sshm --version    # expect: sshm X.Y.Z
```

## 4. Land the version bump (PR — direct push to `main` is restricted)

Direct `git push origin main` is blocked by the auto-mode classifier. Use a PR:

```sh
git checkout -b release/vX.Y.Z
git add Cargo.toml Cargo.lock
git commit -m "chore: release vX.Y.Z"   # end body with the Claude Co-Authored-By trailer
git push -u origin release/vX.Y.Z
gh pr create --base main --title "chore: release vX.Y.Z" --body "..."
gh pr merge --squash --delete-branch
git checkout main && git pull --ff-only origin main
```

## 5. Tag & push  ← triggers release.yml (IRREVERSIBLE)

```sh
git log -1 --oneline             # confirm HEAD is the "chore: release vX.Y.Z" squash commit
git tag -a vX.Y.Z -m "vX.Y.Z — <one-line summary>"
git push origin vX.Y.Z
```

The user invoking `/release` is the explicit go-ahead for this tag push.

## 6. Watch CI and verify the assets

```sh
runid=$(gh run list --workflow=release.yml --event push --limit 1 --json databaseId -q '.[0].databaseId')
gh run watch "$runid" --exit-status         # Node.js-deprecation annotations are noise
gh release view vX.Y.Z --json assets -q '.assets[].name'   # expect 8: 4 archives + 4 .sha256
```

Verify the Windows hash end-to-end (this hash feeds Scoop + winget):

```sh
tmp=$(mktemp -d)
gh release download vX.Y.Z --pattern 'sshm-X.Y.Z-x86_64-pc-windows-msvc.zip*' --dir "$tmp"
sha256sum "$tmp/sshm-X.Y.Z-x86_64-pc-windows-msvc.zip"          # must equal the .sha256 sidecar
unzip -l "$tmp/sshm-X.Y.Z-x86_64-pc-windows-msvc.zip"           # sshm.exe must be at the root
```

Record the **lowercase** hash (for Scoop) and its **UPPERCASE** form (for winget).

## 7. GitHub release notes

```sh
gh release edit vX.Y.Z --title "vX.Y.Z — <summary>" --notes "<markdown notes>"
```

Include Cargo/Scoop/winget install snippets and a short "what changed".

## 8. Publish to crates.io  (IRREVERSIBLE — confirm the version first)

Runs locally with the saved token (per the token policy above). A published
version can never be replaced (only yanked), so **guard before uploading**:

```sh
# guard: X.Y.Z must NOT already be on crates.io
curl -s -H 'User-Agent: sshm-release (email)' https://crates.io/api/v1/crates/sshm-tui | grep -o '"max_version":"[^"]*"'
cargo publish --dry-run      # packages + verify-builds, no upload
cargo publish                # publishes as `sshm-tui`
```

`Cargo.toml`'s `exclude` already drops `.github/`, `docs/`, `CLAUDE.md`,
`packaging/`. After: re-run the `curl` and confirm `max_version` = X.Y.Z.

## 9. Scoop — update the live bucket (this is what `scoop install` uses)

The bucket is a **separate multi-app repo** (also hosts `wslm`): `nakaba-lab/scoop-bucket`,
cloned locally (path varies per machine — e.g. `C:/Users/takah/work/scoop-bucket`; if it
isn't on this machine, `gh repo clone nakaba-lab/scoop-bucket` first). **Sync first on a
clean tree, *then* edit** — editing before the rebase makes `pull --rebase` abort on
unstaged changes:

```sh
B=C:/Users/takah/work/scoop-bucket
git -C "$B" pull --rebase            # FIRST, on a clean tree
# now edit bucket/sshm-tui.json: version → X.Y.Z, url → new vX.Y.Z zip, hash → LOWERCASE sha256
git -C "$B" add -A && git -C "$B" commit -m "Update sshm-tui to X.Y.Z"
git -C "$B" push origin HEAD
# if push is rejected (non-fast-forward): `git -C "$B" pull --rebase` again, retry.
# NEVER force-push the shared bucket.
```

Then mirror the same version/url/hash into the in-repo `packaging/scoop/sshm.json`
(reference copy) for the manifest PR (step 11).

Verify: `scoop update; scoop install sshm-tui` → hash check ok, `sshm --version` = X.Y.Z.

## 10. winget — MANUAL update (wingetcreate cannot handle this installer)

⚠️ `wingetcreate update` **fails** on the zip-nested-portable installer
("could not parse package"). Do it by hand:

1. Edit `packaging/winget/NakabaLab.sshm.*.yaml`: `PackageVersion` → `X.Y.Z`
   (all 3), and in `*.installer.yaml` set `InstallerUrl` to the new vX.Y.Z zip and
   `InstallerSha256` to the **UPPERCASE** hash. Keep the schema at the latest:
   `gh api repos/microsoft/winget-pkgs/contents/doc/manifest/schema -q '.[].name'`
   lists dirs that sort **lexically**, so pick the highest by **SemVer**
   (`1.12.0` > `1.9.0` — not "the last line"). Set both `ManifestVersion` **and**
   the `# yaml-language-server: $schema=…<ver>.schema.json` comment in all 3 files
   to it (`1.12.0` as of v1.0.2).
2. Validate: `winget validate --manifest packaging/winget`.
3. Submit to `microsoft/winget-pkgs` via the fork:
   ```sh
   R=nakata5577/winget-pkgs; BR=NakabaLab.sshm-X.Y.Z; DIR=manifests/n/NakabaLab/sshm/X.Y.Z
   gh repo sync "$R" --source microsoft/winget-pkgs
   sha=$(gh api "repos/$R/git/ref/heads/master" -q '.object.sha')
   gh api --method POST "repos/$R/git/refs" -f ref="refs/heads/$BR" -f sha="$sha"
   for f in NakabaLab.sshm.yaml NakabaLab.sshm.installer.yaml NakabaLab.sshm.locale.en-US.yaml; do
     b64=$(tr -d '\r' < "packaging/winget/$f" | base64 -w0)
     gh api --method PUT "repos/$R/contents/$DIR/$f" -f message="New version: NakabaLab.sshm version X.Y.Z" -f branch="$BR" -f content="$b64"
   done
   gh pr create --repo microsoft/winget-pkgs --base master --head "nakata5577:$BR" --title "New version: NakabaLab.sshm version X.Y.Z" --body "..."
   ```
   CLA is already signed; a **version update** auto-validates and usually
   auto-merges (no New-Package moderator gate). No further action unless a
   `Needs-Author-Feedback` label appears.

## 11. Land the in-repo manifest updates

Branch from **fresh** `main` (after step 4 merged: `git checkout main && git pull
--ff-only origin main`), then PR the `packaging/scoop/sshm.json` +
`packaging/winget/*.yaml` edits → squash-merge, so the reference copies match
what shipped.

## 12. Done

Final state: crates.io / Scoop / GitHub Release live at X.Y.Z; winget PR open and
auto-merging. `main` clean, no open PRs in this repo. If any step changed, update
this runbook.
