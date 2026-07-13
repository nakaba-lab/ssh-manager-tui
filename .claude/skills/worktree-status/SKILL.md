---
name: worktree-status
description: アクティブな worktree の一覧・各 Issue の状態に加え、Issue 間の依存から「並列着手できる Issue／待ち」を動的に確認するとき使用する。「worktreeの状態を確認して」「今何のIssueを作業中か教えて」「作業中のブランチを一覧して」「並列着手できる Issue を教えて」「次に着手できる Issue は」が発火ワード。状態の確認のみ（読み取り専用）で、作業の着手・フロー敷設は /dev-tasks、worktree の作成は /worktree-new、削除は /worktree-cleanup を使う。
---

現在の git worktree 一覧と、対応する Issue の状態を表示します。
リモート URL からプラットフォーム（GitHub / GitLab）を自動判定します。

## 手順

### 1. worktree 一覧取得

```bash
git worktree list
```

### 2. プラットフォーム判定

```bash
REMOTE_URL=$(git remote get-url origin 2>/dev/null)
if echo "$REMOTE_URL" | grep -q "github.com"; then
  PLATFORM="github"
else
  PLATFORM="gitlab"
fi
```

### 3. Issue番号の抽出と状態確認

各 worktree のブランチ名からIssue番号を抽出する。

```bash
# ブランチ名のパターン: feature/42-xxx, fix/99-xxx など
git worktree list --porcelain | grep 'branch' | sed 's|branch refs/heads/||'
```

Issue番号が含まれるブランチごとに状態を確認：

**GitHub の場合:**
```bash
gh issue view <ISSUE_NUMBER> --json number,title,state \
  --jq '"#\(.number) [\(.state)] \(.title)"' 2>/dev/null
```

**GitLab の場合:**
```bash
glab issue view <ISSUE_NUMBER> --output json 2>/dev/null \
  | python3 -c "import sys,json; d=json.load(sys.stdin); print(f'#{d[\"iid\"]} [{d[\"state\"]}] {d[\"title\"]}')"
```

### 4. まとめて表示

以下の形式で一覧表示する：

```
worktree 状態サマリー
─────────────────────────────────────────────────────
ディレクトリ             ブランチ                   Issue 状態
../feature-42-login      feature/42-add-login-page  #42 [open] ログイン画面の追加
../fix-99-null-ptr       fix/99-fix-null-pointer    #99 [open] NullPointerException修正
~/work/develop           develop                    (デフォルトブランチ)
~/work/main              main                       (デフォルトブランチ)
─────────────────────────────────────────────────────
合計: 2件の作業中 Issue
```

Issue番号が取得できない（`develop`/`main` など）はスキップしてよい。

### 4.5 並列着手可能 / 待ち（Issue 間依存から動的に算出）

複数ウィンドウで Issue を並列実装するために、「いま別ウィンドウで `/dev-tasks` を開ける Issue（並列着手可能）」と「他の Issue の完了待ちの Issue」を**現在の Issue 状態から動的に算出**する。順序は本文に焼き込まず毎回ここで再計算するので陳腐化しない。

> **読むのは本文メタだけ**: 依存は L2 Issue 本文冒頭の `> 依存: #<番号>, #<番号>` メタ（`.claude/rules/spec-driven.md`「追跡性」）を真実源にする。ホストネイティブの依存フィールドは読まない（`gh` のバージョンや GitLab のティアに依存しないため）。

1. **対象 Issue を集める**: 現在の worktree の Issue が属する Milestone（`gh issue view <N> --json milestone` 等で判定。引数で Milestone を指定してもよい）の **open Issue を本文込みで取得**する。worktree がまだ無い ready な Issue も「次に開けるウィンドウ候補」として含める。
   - GitHub: `gh issue list --milestone "<title>" --state open --json number,title,state,body`
   - GitLab: `glab issue list --milestone "<title>" -F json`（`description` を含む）
2. **依存をパース**: 各 Issue 本文の `> 依存: #a, #b` 行から参照番号（`#\d+`）を抽出する。
3. **依存先の状態を解決**: 参照先が上の一覧に無ければ個別取得（GitHub `gh issue view <N> --json state` / GitLab `glab issue view <N>`）。closed/merged を「完了」とみなす。
4. **分類して表示**:
   - **並列着手可能**: open かつ全依存先が完了（または依存なし）。worktree の有無を併記する（未作成＝「いま別ウィンドウで `/dev-tasks <type> <N> ...` を開ける」）。
   - **待ち**: open かつ ≥1 依存先が未完了。待ち先を併記する（例: `#60 ← #58 待ち`）。
5. **循環の警告**: open Issue が残るのに並列着手可能が 0 件なら、依存に**循環がある可能性**を警告し planning で依存を見直すよう促す（トポロジカル順が定義できない＝唯一の必須ガード）。

```
並列着手可能（別ウィンドウで /dev-tasks を開けます）
  #58 検索フォーム        worktree 未作成 → /dev-tasks feature 58 ...
  #42 ログイン画面の追加   worktree あり（作業中）
待ち
  #60 設定画面            #58 の完了待ち
```

依存メタが 1 件も無ければこの節はスキップしてよい（単一前提＝従来どおり）。

### 5. 推奨アクション

作業中 worktree が3件以上ある場合は以下を提示する：

```
作業中 worktree が多い場合は /worktree-cleanup でマージ済みのものを整理できます。
```

並列着手可能な Issue（上記 4.5）があれば、「次に別ウィンドウで開ける Issue は #A, #B です（`/dev-tasks` で着手）」と提示し、複数ウィンドウでの並列実装を促す。
