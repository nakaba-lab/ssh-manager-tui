---
name: github-planning
description: 実装計画を GitHub の Milestone・Issue・Sub-issue として3階層で起票するとき使用する。「GitHubに起票して」「GitHub Issue を作って」「GitHub の Milestone を作成して」が発火ワード。GitLab への起票は /gitlab-planning。マージ後のクローズ（ファイナライズ）は /github-finalize（GitLab は /gitlab-finalize）。単なる計画立案のみの依頼では使わない。
---

実装計画を GitHub の **Milestone（Epic 相当） → Issue → Sub-issue（Task 相当）** の3階層で起票します。

$ARGUMENTS がある場合は起票対象の計画ファイルパスまたは計画内容として扱います。

> **起票前に必読**: 各 Level の Spec 構造（L1/L2/L3 の節・粒度・親参照行）は `.claude/rules/spec-driven.md` の「3 階層 Spec 構造」に従う。本スキルは**起票コマンドと親参照行の付け方のみ**を示し、本文構造は重複定義しない。各 Level の本文を作る前に同ファイルの該当節（**L1 Milestone Spec** / **L2 Issue Spec** / **L3 Task Spec**）を読むこと。

---

## 事前確認

```bash
# gh 認証確認
gh auth status

# リポジトリパスを変数として保持（gh auth 済みであれば確実に取得できる）
REPO=$(gh repo view --json nameWithOwner -q .nameWithOwner)
echo "リポジトリ: $REPO"
```

---

## 起票前ゲート（人間承認・重要）

起票はリモートへの**外部操作**（チームに見える・通知が飛ぶ・まとめて取り消しにくい）。作成系コマンドを 1 つでも実行する前に、計画ツリーを提示して承認を得る。

1. **計画ツリーを組み立てる（この時点では何も作成しない）**：`spec-driven.md` に従い、L1 Milestone / L2 Issue 群 / L3 Sub-issue 群のタイトルと階層を確定する。`docs/要件定義書.md` が存在する場合は、**主要機能 → L1/L2 の対応表（トレーサビリティ）**も組み立て、どの L1/L2 にもカバーされない主要機能を明示する（意図的に対象外とする場合は理由を記録する）。要件定義書に「未決事項」節がある場合はその内容も読み、**未決の論点に依存する Issue を特定**しておく。さらに、**L2 Issue 間の依存（どの Issue が他の Issue の完了を着手前提とするか）の候補**を根拠付きで洗い出し、有向グラフ（DAG）として整理する（依存メタの様式は `spec-driven.md`「追跡性」の `> 依存:`）。
2. **AC ウォークスルー（全 L2 Issue・`.claude/rules/alignment.md`）**：起票する **L2 Issue すべて**について、AC（Given/When/Then）を**平文の利用シナリオに展開して照返し**、解釈違い（境界値・状態の定義・暗黙の前提）が無いか確認を得る（展開の例・作法は `.claude/rules/alignment.md` の「AC ウォークスルー」）。確認で解釈が確定・修正された AC は本文に反映してから起票する。**要件定義書の「未決事項」に依存する Issue は起票前に停止し、その論点を `AskUserQuestion` で確定する**（その場で確定できない場合は Issue 本文に未決依存を明記して起票する）。シナリオ展開すべき解釈の余地が無いと判断した場合も、その判断を照返して確認を得る（省略の判断も人間が行う＝同ファイルの大原則）。
3. **提示する**：起票先リポジトリ（`$REPO`）＋ツリー＋件数（Milestone ◯ / Issue ◯ / Sub-issue ◯）。要件定義書がある場合は主要機能 → L1/L2 の対応表と未カバー機能も併せて提示する。L2 Issue 間に依存があれば、**依存グラフ（Mermaid flowchart）と、そこから導いた実装順序（重みなしトポロジカル順）・並列着手可能な Issue 群（同時に着手できるまとまり＝レベル集合）**も会話に提示する（Milestone 説明等には焼き込まない＝陳腐化回避。クリティカルパスの重み付き算出はしない）。
   ```
   起票先: <$REPO>
   Milestone: <タイトル>
     ├─ Issue: <タイトル>
     │    ├─ Sub-issue: <タイトル>
     │    └─ Sub-issue: <タイトル>
     └─ Issue: <タイトル>
          └─ Sub-issue: <タイトル>
   計: Milestone 1 / Issue N / Sub-issue M
   ```
4. **ゲート（`AskUserQuestion`）**：
   - header: `起票の確認`
   - question:「上記の計画を `$REPO` に起票します（Milestone ◯ / Issue ◯ / Sub-issue ◯ 件）。これはチームに見える外部操作で、まとめて取り消しにくい。起票してよいですか？」
   - options:「全て起票（推奨）／一部を除外／起票先を変更／中止」
   - **「一部を除外」時のみ** 2 問目を multiSelect で除外する **Issue** を選ばせる（**除外した Issue はその配下 Sub-issue も除外**）。件数が多い／Sub-issue 単位で絞るなら Issue バケット → Sub-issue の 2 段（各段 4 件以内、超過は「他◯件」ページ送り）。選択肢は組み立てた計画から**動的生成**する（固定リストを持たない）。
   - 「起票先を変更」→ 正しいリポジトリを確認して 3 から再提示。「中止」→ 何も作成せず終了。
   - **依存エッジも同じゲートで承認する**：提示した L2 Issue 間の依存（`> 依存:`）を確認し、AI が推測で張ったものは採らず人間が確定したエッジだけを採用する（`.claude/rules/alignment.md`）。承認後、起票本文に `> 依存:` を書く。
5. **【人間承認なしに起票しない】**：AC ウォークスルー（上記 2）と承認（または除外確定）を得る前に `gh`／`gh api` の作成系コマンド（milestone / issue / sub-issue 作成）を 1 つも実行しない。承認後、除外を除いた集合だけを下記ステップ 1〜3（Milestone / Issue / Sub-issue 作成）で起票する。

---

## ステップ1: Milestone を作成する（Epic 相当）

GitLab の Epic に相当するグルーピングとして Milestone を使用する。説明文は `spec-driven.md` の **L1 Milestone Spec**（4 節・成果レベル AC）で書く。L1 は最上位なので**親参照行はなし**。

```bash
# Milestone の説明文をファイル経由で渡す（\n がリテラルにならないよう heredoc を使う）
# 構造は spec-driven.md「L1 Milestone Spec」に従う
cat > /tmp/milestone_desc.md << 'EOF'
## 背景・目的
...

## 概要
...

## 受け入れ条件（AC）
- [ ] <成果レベルの観点>

## 完了条件（DoD）
- [ ] 配下のすべての Issue がクローズしている
EOF

MILESTONE_TITLE="Milestone タイトル"

# Milestone 作成（due_on は任意。YYYY-MM-DDT00:00:00Z 形式）
gh api repos/$REPO/milestones --method POST \
  -f "title=$MILESTONE_TITLE" \
  -f "description=$(cat /tmp/milestone_desc.md)" \
  -f state="open"
# → レスポンスの number（例: 1）を記録する
MILESTONE_NUMBER=1
```

> **注意**: `number` は後の Issue 紐づけに使用する。`id`（内部ID）とは異なる。

> **起票時の確認（要件整合・due date）**:
> - **要件整合**: `docs/要件定義書.md` がある場合、Milestone のスコープが要件定義書（スコープ・主要機能）と整合するか確認する。乖離（スコープ・主要機能の増減）があれば、要件定義書の「変更履歴」節への追記（改訂）を促してから起票する。
> - **due date**: 設定を既定で促す（`-f "due_on=YYYY-MM-DDT00:00:00Z"`。進捗の基準線になる＝`spec-driven.md` の「見積り・進捗（軽量）」）。不要なら省略してよい（任意のまま）。

---

## ステップ2: Issue を作成する（GitLab Issue 相当）

Issue の説明文は **Spec 駆動開発（Issue 型）規則**（`.claude/rules/spec-driven.md`）の **L2 Issue Spec**（feature / bug の 2 種）に従い、受け入れ条件（AC）を含む仕様書として作成する。本文**冒頭に親参照行**を置く（Milestone は任意）:

```
> 親: #<MILESTONE_NUMBER> ／ 寄与する親AC: <n>番目（任意）
```

> **依存行（任意）**: 起票前ゲートで承認された L2 Issue 間の依存がある場合、親参照行の直後に `> 依存: #<Issue番号>, #<Issue番号>` を置く（`spec-driven.md`「追跡性」）。依存先 Issue の番号は**作成後に確定する**ため、依存は **2 パス**で書く: ① L2 Issue を先に全作成して番号を採番 → ② `gh issue edit <番号> --body-file <更新本文>` で `> 依存:` 行を追記する（sub-issue の「作成→紐づけ」と同じ 2 段作法）。

> feature / bug の節構成は本スキルに重複定義しない。`spec-driven.md` の「L2 Issue Spec」を参照すること。

> **Web/UI を含む Issue では**、`spec-driven.md` の「ビジュアル/UX 受け入れ条件」（非機能要件配下の例）と「（UI 変更時）の DoD 項目」も本文に含めて起票する（CLAUDE.md「プロジェクト設定」の `frontendDir` 設定済み〔`"."`＝ルート直下も設定済み。`"none"`＝UI を持たない明示は対象外〕が目安）。視覚 AC は `docs/要件定義書.md` の「UI/UX 方針」節（あれば）を参照し、フロア項目（崩れ・状態・コントラスト）に加えて**デザイン意図（方向性・参照デザイン・主要画面との整合）**も具体化する。これにより実装・レビュー・クローズの各段で見た目が要求される。加えて**主要画面のワイヤーフレームまたは画面遷移図（Mermaid flowchart か ASCII）**を Issue 本文に含めるか、`docs/design/ui.md`（`status: draft`）に置いて参照する。

> **規模感（任意）**: L2 Issue には規模感（S/M/L）をラベルか本文 1 行で任意記載する（軽量な見積りの手がかり。必須にしない。`spec-driven.md` の「見積り・進捗（軽量）」）。

### 起票コマンド

```bash
# 説明文をファイルに書いてから変数に読み込む（特殊文字の shell 解釈を避けるため）
cat > /tmp/issue_desc.md << 'EOF'
> 親: #<MILESTONE_NUMBER> ／ 寄与する親AC: <n>番目（任意）

## 背景・目的
...
EOF
DESC=$(cat /tmp/issue_desc.md)

ISSUE_URL=$(gh issue create \
  --repo "$REPO" \
  --title "[タグ] Issue タイトル" \
  --body "$DESC" \
  --milestone "$MILESTONE_TITLE")
# → URL に含まれる番号が Issue number（例: /issues/21 → number=21）
ISSUE_NUMBER=$(echo "$ISSUE_URL" | grep -oE '[0-9]+$')

# Sub-issue 紐づけ用に issue の id（データベース ID）も取得しておく
ISSUE_ID=$(gh api repos/$REPO/issues/$ISSUE_NUMBER --jq .id)
echo "Issue #$ISSUE_NUMBER (id: $ISSUE_ID)"
```

---

## ステップ3: Sub-issue を作成する（GitLab WorkItem Task 相当）

GitLab の WorkItem Task に相当する子タスクを GitHub Sub-issues API で作成する。
Sub-issue は通常の Issue として作成してから親 Issue に紐づける。説明文は `spec-driven.md` の **L3 Task Spec**（手順レベル・親 Issue の型を継承）に従い、本文冒頭に親参照行（親 Issue は**必須**）を置く。

```bash
# ヘルパースクリプトを用意すると繰り返し作業が楽になる
cat > /tmp/create_sub_issue.sh << 'SCRIPT'
#!/bin/bash
REPO=$1
PARENT_NUMBER=$2
TITLE=$3
DESC_FILE=$4   # 説明文ファイルのパス

# 1. 子 Issue を通常の Issue として作成
CHILD_URL=$(gh issue create \
  --repo "$REPO" \
  --title "$TITLE" \
  --body "$(cat $DESC_FILE)")
CHILD_NUMBER=$(echo "$CHILD_URL" | grep -oE '[0-9]+$')

# 2. 子 Issue の id（データベース ID）を取得
CHILD_ID=$(gh api repos/$REPO/issues/$CHILD_NUMBER --jq .id)

# 3. 親 Issue の Sub-issue として紐づける
gh api repos/$REPO/issues/$PARENT_NUMBER/sub_issues \
  --method POST \
  -F sub_issue_id=$CHILD_ID \
  --jq '"Created Sub-issue #\(.number): \(.title)"'
SCRIPT
chmod +x /tmp/create_sub_issue.sh

# 使用例（説明文は spec-driven.md「L3 Task Spec」に従う。冒頭の親参照行は必須）
cat > /tmp/sub_desc.md << 'EOF'
> 親: #<ISSUE_NUMBER> ／ 寄与する親AC: <n>番目（任意）

## 背景・目的
...

## 概要
...

## 受け入れ条件（AC）
- [ ] Given <状態>, When <実装>, Then <テストが緑>

## 完了条件（DoD）
- [ ] AC に対応するテストが緑
EOF

/tmp/create_sub_issue.sh "$REPO" "$ISSUE_NUMBER" \
  "タスク1-1: DBマイグレーション" \
  /tmp/sub_desc.md
```

> **注意**: `sub_issue_id` には Issue の `number`（URL の番号）ではなく `id`（データベース ID）を渡す。`gh api repos/$REPO/issues/$NUMBER --jq .id` で取得できる。

---

## ステップ4: 完全性チェック（起票の証拠）

```bash
# Milestone の進捗確認
gh api repos/$REPO/milestones/$MILESTONE_NUMBER \
  --jq '{title: .title, open: .open_issues, closed: .closed_issues, url: .html_url}'

# Issue 一覧確認（Milestone に紐づいた親 Issue）
gh issue list --repo "$REPO" --milestone "$MILESTONE_TITLE" \
  --json number,title,state \
  --jq '.[] | "#\(.number) [\(.state)] \(.title)"'

# Sub-issue 一覧確認（親 Issue の子タスク）
gh api repos/$REPO/issues/$ISSUE_NUMBER/sub_issues \
  --jq '.[] | "  └── #\(.number) [\(.state)] \(.title)"'
```

確認コマンドの結果だけで「完了」にせず、計画と実体を突き合わせる:

- **件数一致**：承認した計画の件数（Milestone / Issue / Sub-issue）と、実際に起票された件数が一致するか。**不一致なら未起票を特定して補完**する。
- **orphan 検出**：Sub-issue は「子 Issue の作成は成功したが親への紐づけ（`sub_issues` への POST）が失敗」＝親 Issue の `sub_issues` 一覧に**現れず欠落しうる**。各子の紐づけ成功を確認し、orphan があれば `gh api repos/$REPO/issues/$PARENT/sub_issues --method POST -F sub_issue_id=<id>` で**再リンク**する。
- **依存エッジの整合**：起票前ゲートで承認した各依存が、対応する Issue 本文の `> 依存:` 行に書かれているか確認する（欠落は `gh issue edit` で補完）。
- **証拠の提示**：起票した Milestone / Issue / Sub-issue の番号・URL 一覧と「計画 N 件＝起票 N 件・orphan 0・依存エッジ整合済み」を提示してから完了とする。

---

## クローズ（ファイナライズ）は `/github-finalize` へ

本スキルは**起票まで（ステップ1〜4）**を担う。マージ完了後の **Issue＋子 Sub-issue＋（条件付き）Milestone のクローズ（ファイナライズ）は `/github-finalize` スキル**が担う（AC/DoD 充足を確認する**クローズ前ゲート**を含む）。`/dev-tasks` の「クローズ＆ファイナライズ」タスクは `vcsHost=github` のとき `/github-finalize` を呼ぶ。

---

## 要点まとめ

| 操作 | API / コマンド | 備考 |
|------|---------------|------|
| Milestone 作成 | `gh api repos/OWNER/REPO/milestones --method POST` | Epic 相当 |
| Issue 作成 | `gh issue create --milestone <number または title>` | GitLab Issue 相当 |
| Issue の id 取得 | `gh api repos/OWNER/REPO/issues/NUMBER --jq .id` | Sub-issue 紐づけに必要 |
| Sub-issue 作成 | `gh issue create` → `gh api .../sub_issues --method POST -F sub_issue_id=ID` | GitLab WorkItem Task 相当 |
| Sub-issue 一覧 | `gh api repos/OWNER/REPO/issues/NUMBER/sub_issues` | 親 Issue の子タスク一覧 |
| PR 作成 | `gh pr create` | `/ship` スキルで行う |

## GitLab との対応関係

| GitLab | GitHub | 作成方法 |
|--------|--------|---------|
| Epic（グループレベル） | Milestone | `gh api .../milestones --method POST` |
| Issue | Issue | `gh issue create` |
| WorkItem Task（GraphQL） | Sub-issue（REST API） | `gh api .../sub_issues --method POST` |

## ハマりどころ

- **`sub_issue_id` は `number` ではなく `id`**: Sub-issues API に渡すのは Issue の URL に含まれる `number`（例: `21`）ではなく、データベース上の `id`（例: `123456789`）。`gh api repos/$REPO/issues/$NUMBER --jq .id` で取得する
- **Sub-issues はリポジトリ設定に依存しない**: GitLab WorkItem と異なり、GitHub Sub-issues は REST API で完結しリポジトリの特別な設定は不要
- **`--milestone` には number または title を渡す**: `gh issue create` の `--milestone` は Milestone のタイトル文字列または番号どちらでも受け付ける
- **shell 特殊文字**: Issue 説明文に `*`/`\`` が含まれると glob 展開される。説明文はファイルに書いてから変数に読み込む
- **`gh api` の `-f` / `-F` フラグ**: 文字列値には `-f`、数値には `-F` を使う（`sub_issue_id` は数値なので `-F`）
- **本文構造は spec-driven.md が正**: 各 Level の節構成・粒度・親参照行は本スキルに重複定義せず `.claude/rules/spec-driven.md`（L1 Milestone Spec / L2 Issue Spec / L3 Task Spec）に従う。起票前に必ず読む
- **起票は冪等でない（再実行注意）**: 再実行・途中失敗で Milestone/Issue が**重複作成**される。作成前に同タイトルの既存をチェックし（Milestone: `gh api repos/$REPO/milestones --jq '.[].title'`／Issue: `gh issue list --repo "$REPO" --search "in:title <タイトル>"`）、あれば再利用するかユーザーに確認してから作成する。起票前ゲートで件数・起票先を確認することも重複の早期検知に効く
- **自動クローズはデフォルトブランチ限定**: PR 本文の `Closes #N` は**リポジトリのデフォルトブランチへのマージ時のみ**発火し、L2 Issue 1 件しか閉じない。default=main の Git Flow に備え、マージ後は必ず `/github-finalize` で Issue＋子＋（該当時）Milestone を明示クローズする
