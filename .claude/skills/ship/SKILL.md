---
name: ship
description: コミット済みブランチを push → PR/MR 作成 → マージ（feature/fix 等は squash、release/hotfix は Merge Commit＋戻しマージ確認）して develop/main に反映するとき使用する。「mainに反映して」「shipして」「マージして」「プッシュしてマージして」が発火ワード。コミット作業のみの依頼では使わない。
disable-model-invocation: true
---

現在のブランチを push し、PR（GitHub）または MR（GitLab）を作成して squash merge します（`release/*`・`hotfix/*` → `main` のみ Merge Commit。手順 6 の注記）。
リモート URL からプラットフォームを自動判定します。

## 手順

### 1. 現状確認 / ターゲットブランチ決定

ターゲットブランチの**既定**を次の順で決める：`$ARGUMENTS` 指定が最優先 → ブランチ型（`.claude/rules/git-workflow.md` のブランチ表: `feature`/`fix`/`docs`/`refactor`/`perf`/`test`/`chore` → `develop`／`release/*`・`hotfix/*` → `main`）→ CLAUDE.md「プロジェクト設定」の `defaultBranch`（未設定なら `develop`）。**この既定を `AskUserQuestion` で利用者に提示し、確認・上書きさせてから** push/PR/merge に進む（マージ先は不可逆なので明示確認）。

> **`release/*`・`hotfix/*` は二重マージ**: `.claude/rules/git-workflow.md`「マージ戦略」上、`release/*`・`hotfix/*` のマージ先は `main` + `develop` の**二重**。本スキルは**一次ターゲット（`main`）**を反映したのち、`develop` への戻しマージの扱いを手順 7 で確認する（その場で実行するか `/release-tasks` に委ねる）。

```bash
TARGET=develop   # ← 型から既定（release/hotfix は main）。AskUserQuestion 確認後に確定（確認前に push/PR へ進まない）
git branch --show-current
git log "$TARGET..HEAD" --oneline
git diff "$TARGET...HEAD" --stat
```

### 2. プラットフォーム判定

```bash
REMOTE_URL=$(git remote get-url origin 2>/dev/null)
if echo "$REMOTE_URL" | grep -q "github.com"; then
  PLATFORM="github"
else
  PLATFORM="gitlab"
fi
echo "プラットフォーム: $PLATFORM"
```

### 3. PR/MR タイトル・説明文を生成する

コミット一覧と変更ファイルを分析し、以下を作成する。

**タイトル**（Conventional Commits 形式）:
```
<type>(<scope>): <description>
```

**説明文テンプレート**:
```markdown
## 概要
<!-- この変更の背景・目的を1〜3文で -->

## 変更内容
- 

## テスト方法
- [ ] 

## レビュー記録
<!-- 「コードレビュー」タスク（code-reviewer）の指摘一覧と対応結果をここに転記する。
     接頭辞は .claude/rules/code-review.md に従う（must:/should:/nit:/question:）。
     must: は全件対応済み（0 件）であること。指摘が無ければ「指摘なし」と 1 行書く -->
| 指摘（接頭辞付き） | 対応結果 |
|--------------------|----------|
|                    |          |

## スクリーンショット（UI 変更時は必須）
<!-- UI を変更した場合は Before / After を貼る。画像は「ビジュアル/UX 確認」タスク／frontend-reviewer が
     docs/screenshots/ に保存した <issue>-<画面>-<ブレークポイント>-<before|after>.png を使う。
     frontendDir 配下（"." の場合は UI ファイル）に変更が無ければこの節は削除してよい -->
| Before | After |
|--------|-------|
|        |       |
- [ ] 主要ブレークポイント（モバイル/デスクトップ）で表示確認済み

## 関連 Issues
Closes #
```

### 4. push する

```bash
git push -u origin <current-branch>
```

### 5. PR / MR を作成する

**GitHub の場合:**
```bash
PR_URL=$(gh pr create \
  --title "<タイトル>" \
  --body "<説明文>" \
  --head <current-branch> \
  --base "$TARGET")
echo "$PR_URL"
# 出力は URL 形式（例: https://github.com/OWNER/REPO/pull/42）
# 番号を取り出す場合:
PR_NUMBER=$(echo "$PR_URL" | grep -oE '[0-9]+$')
echo "PR 番号: $PR_NUMBER"
```

**GitLab の場合:**
```bash
MR_URL=$(glab mr create \
  --title "<タイトル>" \
  --description "<説明文>" \
  --source-branch <current-branch> \
  --target-branch "$TARGET" \
  --squash-before-merge)
echo "$MR_URL"
# 出力は URL 形式（例: https://gitlab.com/GROUP/PROJECT/-/merge_requests/42）
# 番号を取り出す場合:
MR_NUMBER=$(echo "$MR_URL" | grep -oE '[0-9]+$')
echo "MR 番号: $MR_NUMBER"
```

### 6. squash merge する

**GitHub の場合:**

> **前提**: リポジトリの設定で「Allow squash merging」が有効になっていること（Settings → General → Pull Requests）。

```bash
gh pr merge $PR_NUMBER --squash --delete-branch
```

**GitLab の場合:**
```bash
glab mr merge $MR_NUMBER --squash --remove-source-branch --yes
```

GitLab で `--squash` が 405 エラーになる場合は `--squash` を外して `--remove-source-branch --yes` のみで試す。

> **`release/*`・`hotfix/*` → `main` は Merge Commit**（`.claude/rules/git-workflow.md`「マージ戦略」）: この経路では squash しない — GitHub は `gh pr merge $PR_NUMBER --merge`、GitLab は `--squash-before-merge`（手順 5）と `--squash` を付けずにマージする。また手順 7 の戻しマージをソースブランチから行うため、`--delete-branch`／`--remove-source-branch` も付けず、戻しマージ完了後に削除する。

### 7. develop への戻しマージ確認（`release/*`・`hotfix/*` のみ）

`release/*`・`hotfix/*` のマージ先は `main` + `develop` の**二重**（`.claude/rules/git-workflow.md`「マージ戦略」）。戻しマージを忘れると次リリースから修正が落ちるため、`main` へのマージ完了後、**`AskUserQuestion` で戻しマージの扱いを確認してから完了報告に進む**：

| 選択肢 | 動作 |
|--------|------|
| 今ここで実行する | 当該ブランチ → `develop` の PR/MR を作成し **Merge Commit** でマージする（squash しない。手順 5〜6 と同じ要領でターゲットだけ `develop` に変える。ソースブランチが削除済みなら `main` → `develop` で戻す） |
| `/release-tasks` に委ねる | リリース工程のタスク敷設に含まれる「develop への戻しマージ」タスクで実行する（本スキルでは実行しない。二重実行しないこと） |
| 実行済み／不要 | 理由を確認して完了報告に進む |

`feature/*` 等の単一ターゲット経路ではこの手順をスキップする。

### 8. 完了報告

マージ済み PR/MR の URL を表示する（`release/*`・`hotfix/*` では戻しマージの扱い〔実行済み／`/release-tasks` に委任〕も併記する）。
