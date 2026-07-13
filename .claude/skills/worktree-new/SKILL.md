---
name: worktree-new
description: git worktree と feature/fix ブランチを作成するとき使用する（タスク敷設は伴わない／Issue 番号から命名）。「worktreeを作って」「新しいブランチを作って」「feature branchを作って」が発火ワード。開発フロー全体の着手（タスク敷設込み）は /dev-tasks を使う。
---

Issue番号をもとに git worktree とブランチを作成します（開発フロー全体の敷設・着手は `/dev-tasks`）。

$ARGUMENTS がある場合は `<type> <issue-number> <short-description>` として扱います。  
例: `feature 42 add-login-page` → ブランチ `feature/42-add-login-page`、ディレクトリ `../feature-42-add-login-page`

## 手順

### 1. リモート最新化

```bash
git fetch origin
```

### 2. ブランチ種別・命名の決定

$ARGUMENTS から種別・Issue番号・説明を取得する。指定がない場合はユーザーに確認する。

| ブランチ種別 | 用途 |
|-------------|------|
| `feature` | 新機能 |
| `fix` | バグ修正 |
| `refactor` | リファクタリング |
| `perf` | パフォーマンス改善 |
| `test` | テスト追加 |
| `chore` | ビルド・設定 |
| `docs` | ドキュメント |
| `hotfix` | 本番緊急修正（派生元 `main`） |
| `release` | リリース準備 |

変数を確定する：
```
TYPE=feature
ISSUE=42
DESC=add-login-page
BRANCH="${TYPE}/${ISSUE}-${DESC}"
WORKTREE_DIR="../${TYPE}-${ISSUE}-${DESC}"
BASE=origin/develop   # ← 型から既定（hotfix は origin/main）。下記のとおり確認・上書き可
```

> **派生元 `BASE` は型から既定を出す**（`.claude/rules/git-workflow.md` のブランチ表に従う: `feature`/`fix`/`docs`/`refactor`/`perf`/`test`/`chore` → `origin/develop`／`hotfix` → `origin/main`／`release` → `origin/develop`。`defaultBranch` がトランクベース運用なら全体を `origin/main` に読み替え）。**この既定を `AskUserQuestion` で利用者に提示し、確認・上書きさせてから** Step 3 で `git worktree add` する（派生元の取り違えは後段の rebase/マージに波及するため明示確認）。

### 3. worktree を作成する

```bash
git worktree add "$WORKTREE_DIR" -b "$BRANCH" "$BASE"
```

### 4. 作成確認

```bash
git worktree list
```

### 5. 完了報告

以下を出力して終了する：

```
✅ worktree 作成完了

ブランチ : feature/42-add-login-page
ディレクトリ: ../feature-42-add-login-page

次のステップ:
  ! cd ../feature-42-add-login-page          ← このセッションで移動する場合
  開発フロー（テスト先行→実装→…→片付け）を敷いて着手するなら `/dev-tasks`
```

> このスキルは worktree/ブランチの**作成のみ**を行う（開発タスクは敷設しない）。テスト先行〜マージ〜片付けの直列タスクを敷くのは `/dev-tasks`（既存 worktree 前提のタスクを敷設する）。

## トラブルシューティング

| エラー | 対処 |
|--------|------|
| `already exists` (ブランチ) | `-b` を外して `git worktree add "$WORKTREE_DIR" "$BRANCH"` で既存ブランチを使う |
| `already exists` (ディレクトリ) | ディレクトリ名を変更するか、`git worktree list` で重複を確認する |
| `fatal: not a git repository` | bare リポジトリ（`<repo>.git`）直下など作業ツリー外で実行していないか確認する |
