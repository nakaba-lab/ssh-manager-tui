---
name: worktree-cleanup
description: フェーズ・Issue完了後にマージ済みworktreeとブランチを一括削除するとき使用する。「worktreeを掃除して」「worktreeを削除して」「後片付けして」が発火ワード。
disable-model-invocation: true
---

マージ済みの worktree とローカルブランチを一括削除します。

## 手順

### 1. 現状確認

```bash
git fetch origin
git worktree list
```

削除対象（`main`・`develop`（デフォルトブランチ）と bare リポジトリ以外）を特定する。

### 2. リモートへのマージ確認

削除前に、対象ブランチがデフォルトブランチ（CLAUDE.md「プロジェクト設定」の `defaultBranch`、未設定なら `develop`）にマージ済みかを確認する（`release/*` / `hotfix/*` は `origin/main`）：

```bash
TARGET=develop   # ← CLAUDE.md の defaultBranch に合わせる
git branch -r --merged "origin/$TARGET" | grep <branch-name>
```

出力があればマージ済み。出力がなくても Squash Merge の場合はマージ済みの可能性があるため、`git log "origin/$TARGET" --oneline` でコミット内容を目視確認する。

### 3. worktree とブランチの削除

```bash
git worktree remove <worktree-path> --force
```

worktree 削除後、ローカルブランチを削除する：

```bash
# 通常削除（マージ済みと認識される場合）
git branch -d <branch-name>

# 強制削除（Squash Merge のためローカルで「未マージ」と判定される場合）
git branch -D <branch-name>
```

**Squash Merge 運用では `git branch -d` が "not fully merged" エラーになることが多い。**  
リモートへのマージを確認済みであれば `git branch -D` で強制削除してよい。

### 4. 完了確認

```bash
git worktree list
```

メインの worktree（`main`/`develop`）と bare リポジトリのみが残っていることを確認する。

## まとめてクリーンアップするコマンド例

同一 Issue の複数 worktree（例: Issue #42 の `feature/42-foo`・`feature/42-bar`）を一括削除する場合：

```bash
for slug in foo bar baz; do
  git worktree remove ../feature-42-$slug --force 2>/dev/null && echo "worktree削除: feature-42-$slug"
  git branch -d feature/42-$slug 2>/dev/null || git branch -D feature/42-$slug && echo "ブランチ削除: feature/42-$slug"
done
git worktree list
```

ブランチ名のパターンに合わせてリストを適宜変更すること。`/clean_gone`（commit-commands プラグイン）でリモート削除済み（gone）ブランチを一括掃除することもできる。
