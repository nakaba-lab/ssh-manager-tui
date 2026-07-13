---
name: gitlab-planning
description: 実装計画を GitLab の Milestone・Issue・子タスク（Task）として3階層で起票するとき使用する。「GitLabに起票して」「GitLab の Issue を作って」「GitLab の Milestone を作成して」が発火ワード。GitHub への起票は /github-planning。マージ後のクローズ（ファイナライズ）は /gitlab-finalize（GitHub は /github-finalize）。単なる計画立案のみの依頼では使わない。
---

実装計画を GitLab の **Milestone → Issue → Task** の 3 層構造で起票します。

> **なぜ Milestone か**: Epic はグループレベル機能で **GitLab Premium / Ultimate 専用**（Free / CE では作成できない）。本スキルはエディションに依存しない **Milestone**（プロジェクトレベル・全エディションで利用可）でグルーピングする。Epic が使える環境で Epic にまとめたい場合は、起票後に GitLab UI または `glab api groups/GROUP/epics` で Issue を Epic に紐づけてよい（任意）。github-planning（Milestone 起点）と対称な構成。

$ARGUMENTS がある場合は起票対象の計画ファイルパスまたは計画内容として扱います。

> **起票前に必読**: 各 Level の Spec 構造（L1/L2/L3 の節・粒度・親参照行）は `.claude/rules/spec-driven.md` の「3 階層 Spec 構造」に従う。本スキルは**起票コマンドと親参照行の付け方のみ**を示し、本文構造は重複定義しない。各 Level の本文を作る前に同ファイルの該当節（**L1 Milestone Spec** / **L2 Issue Spec** / **L3 Task Spec**）を読むこと。

---

## 事前確認

```bash
# glab 認証確認
glab auth status

# リモート URL からプロジェクトパスを確認
git remote -v
# 例: origin https://gitlab.com/GROUP/PROJECT.git
# → GROUP/PROJECT が PROJECT_PATH（以後 GROUP/PROJECT と表記）
```

---

## 起票前ゲート（人間承認・重要）

起票はリモートへの**外部操作**（チームに見える・通知が飛ぶ・まとめて取り消しにくい）。作成系コマンドを 1 つでも実行する前に、計画ツリーを提示して承認を得る。

1. **計画ツリーを組み立てる（この時点では何も作成しない）**：`spec-driven.md` に従い、L1 Milestone / L2 Issue 群 / L3 Task 群のタイトルと階層を確定する。`docs/要件定義書.md` が存在する場合は、**主要機能 → L1/L2 の対応表（トレーサビリティ）**も組み立て、どの L1/L2 にもカバーされない主要機能を明示する（意図的に対象外とする場合は理由を記録する）。要件定義書に「未決事項」節がある場合はその内容も読み、**未決の論点に依存する Issue を特定**しておく。さらに、**L2 Issue 間の依存（どの Issue が他の Issue の完了を着手前提とするか）の候補**を根拠付きで洗い出し、有向グラフ（DAG）として整理する（依存メタの様式は `spec-driven.md`「追跡性」の `> 依存:`）。
2. **AC ウォークスルー（全 L2 Issue・`.claude/rules/alignment.md`）**：起票する **L2 Issue すべて**について、AC（Given/When/Then）を**平文の利用シナリオに展開して照返し**、解釈違い（境界値・状態の定義・暗黙の前提）が無いか確認を得る（展開の例・作法は `.claude/rules/alignment.md` の「AC ウォークスルー」）。確認で解釈が確定・修正された AC は本文に反映してから起票する。**要件定義書の「未決事項」に依存する Issue は起票前に停止し、その論点を `AskUserQuestion` で確定する**（その場で確定できない場合は Issue 本文に未決依存を明記して起票する）。シナリオ展開すべき解釈の余地が無いと判断した場合も、その判断を照返して確認を得る（省略の判断も人間が行う＝同ファイルの大原則）。
3. **提示する**：起票先プロジェクト（`GROUP/PROJECT`）＋ツリー＋件数（Milestone ◯ / Issue ◯ / Task ◯）。要件定義書がある場合は主要機能 → L1/L2 の対応表と未カバー機能も併せて提示する。L2 Issue 間に依存があれば、**依存グラフ（Mermaid flowchart）と、そこから導いた実装順序（重みなしトポロジカル順）・並列着手可能な Issue 群（同時に着手できるまとまり＝レベル集合）**も会話に提示する（Milestone 説明等には焼き込まない＝陳腐化回避。クリティカルパスの重み付き算出はしない）。
   ```
   起票先: GROUP/PROJECT
   Milestone: <タイトル>
     ├─ Issue: <タイトル>
     │    ├─ Task: <タイトル>
     │    └─ Task: <タイトル>
     └─ Issue: <タイトル>
          └─ Task: <タイトル>
   計: Milestone 1 / Issue N / Task M
   ```
4. **ゲート（`AskUserQuestion`）**：
   - header: `起票の確認`
   - question:「上記の計画を `GROUP/PROJECT` に起票します（Milestone ◯ / Issue ◯ / Task ◯ 件）。これはチームに見える外部操作で、まとめて取り消しにくい。起票してよいですか？」
   - options:「全て起票（推奨）／一部を除外／起票先を変更／中止」
   - **「一部を除外」時のみ** 2 問目を multiSelect で除外する **Issue** を選ばせる（**除外した Issue はその配下 Task も除外**）。件数が多い／Task 単位で絞るなら Issue バケット → Task の 2 段（各段 4 件以内、超過は「他◯件」ページ送り）。選択肢は組み立てた計画から**動的生成**する（固定リストを持たない）。
   - 「起票先を変更」→ 正しいプロジェクトを確認して 3 から再提示。「中止」→ 何も作成せず終了。
   - **依存エッジも同じゲートで承認する**：提示した L2 Issue 間の依存（`> 依存:`）を確認し、AI が推測で張ったものは採らず人間が確定したエッジだけを採用する（`.claude/rules/alignment.md`）。承認後、起票本文に `> 依存:` を書く。
5. **【人間承認なしに起票しない】**：AC ウォークスルー（上記 2）と承認（または除外確定）を得る前に `glab`／`glab api` の作成系コマンド（milestone / issue / `workItemCreate`）を 1 つも実行しない。承認後、除外を除いた集合だけを下記ステップ 1〜3（Milestone / Issue / Task 作成）で起票する。

---

## ステップ1: Milestone を作成する（グルーピング）

GitLab の Milestone はプロジェクトレベルのグルーピング機能で、全エディションで使える。説明文は `spec-driven.md` の **L1 Milestone Spec**（4 節・成果レベル AC）で書く。L1 は最上位なので**親参照行はなし**。

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

# Milestone 作成（due_date は任意。YYYY-MM-DD 形式）
glab api projects/GROUP%2FPROJECT/milestones --method POST \
  -f "title=$MILESTONE_TITLE" \
  -f "description=$(cat /tmp/milestone_desc.md)"
# → レスポンスの id（数値）と iid を記録する。Issue 紐づけには title か id を使う。
MILESTONE_ID=1   # レスポンスの id を控える
```

> **注意**: `projects/GROUP%2FPROJECT` のように、プロジェクトパスの `/` は URL エンコード（`%2F`）する。

> **起票時の確認（要件整合・due date）**:
> - **要件整合**: `docs/要件定義書.md` がある場合、Milestone のスコープが要件定義書（スコープ・主要機能）と整合するか確認する。乖離（スコープ・主要機能の増減）があれば、要件定義書の「変更履歴」節への追記（改訂）を促してから起票する。
> - **due date**: 設定を既定で促す（`-f "due_date=YYYY-MM-DD"`。進捗の基準線になる＝`spec-driven.md` の「見積り・進捗（軽量）」）。不要なら省略してよい（任意のまま）。

---

## ステップ2: Issue を作成する

Issue の説明文は **Spec 駆動開発（Issue 型）規則**（`.claude/rules/spec-driven.md`）の **L2 Issue Spec**（feature / bug の 2 種）に従い、受け入れ条件（AC）を含む仕様書として作成する。本文**冒頭に親参照行**を置く（Milestone は任意）:

```
> 親: Milestone「<MILESTONE_TITLE>」 ／ 寄与する親AC: <n>番目（任意）
```

> **依存行（任意）**: 起票前ゲートで承認された L2 Issue 間の依存がある場合、親参照行の直後に `> 依存: #<Issue番号>, #<Issue番号>` を置く（`spec-driven.md`「追跡性」）。依存先 Issue の iid は**作成後に確定する**ため、依存は **2 パス**で書く: ① L2 Issue を先に全作成して iid を採番 → ② `glab issue update <iid> --description "<更新本文>"` で `> 依存:` 行を追記する（Task の「作成→紐づけ」と同じ 2 段作法）。

> **（任意）ネイティブ依存**: 依存の真実源は本文メタ（`> 依存:`）。Premium/Ultimate 環境で UI のブロック表示が欲しい場合のみ、任意で `glab api graphql` の `workItemAddLinkedItems(input: { id, workItemsIds, linkType: BLOCKED_BY })` でネイティブな blocked-by も張れる（**既定オフ**。Free/CE では blocks/is_blocked_by が使えない＝Epic を Milestone で代替したのと同じティア配慮）。

> feature / bug の節構成は本スキルに重複定義しない。`spec-driven.md` の「L2 Issue Spec」を参照すること。

> **Web/UI を含む Issue では**、`spec-driven.md` の「ビジュアル/UX 受け入れ条件」（非機能要件配下の例）と「（UI 変更時）の DoD 項目」も本文に含めて起票する（CLAUDE.md「プロジェクト設定」の `frontendDir` 設定済み〔`"."`＝ルート直下も設定済み。`"none"`＝UI を持たない明示は対象外〕が目安）。視覚 AC は `docs/要件定義書.md` の「UI/UX 方針」節（あれば）を参照し、フロア項目（崩れ・状態・コントラスト）に加えて**デザイン意図（方向性・参照デザイン・主要画面との整合）**も具体化する。これにより実装・レビュー・クローズの各段で見た目が要求される。加えて**主要画面のワイヤーフレームまたは画面遷移図（Mermaid flowchart か ASCII）**を Issue 本文に含めるか、`docs/design/ui.md`（`status: draft`）に置いて参照する。

> **規模感（任意）**: L2 Issue には規模感（S/M/L）をラベルか本文 1 行で任意記載する（軽量な見積りの手がかり。必須にしない。`spec-driven.md` の「見積り・進捗（軽量）」）。

### 起票コマンド（作成時に Milestone へ割り当てる）

```bash
# 説明文をファイルに書いてから変数に読み込む（バッククォートや特殊文字の shell 解釈を避けるため）
DESC=$(cat /tmp/issue_desc.md)
glab issue create --repo GROUP/PROJECT \
  --title "[タグ] Issue タイトル" \
  --description "$DESC" \
  --milestone "$MILESTONE_TITLE" \
  --no-editor
# → URL に含まれる番号が iid（例: issues/21 → iid=21）
```

> `glab issue create` の `--milestone` は Milestone の **title** を受け付ける。作成時に割り当てれば後からの紐づけは不要。

---

## ステップ3: 子タスク（Task）を作成する

GraphQL の `workItemCreate` mutation で Issue の下に Task を作成する。説明文（`$DESC`）は `spec-driven.md` の **L3 Task Spec**（手順レベル・親 Issue の型を継承）に従い、**冒頭に親参照行（親 Issue は必須）**を含める。まず Task の Work Item Type ID と、親 Issue の Work Item GID を確認する。

```bash
# Task の Work Item Type ID を確認（通常: gid://gitlab/WorkItems::Type/5）
glab api graphql -f query='
{
  project(fullPath: "GROUP/PROJECT") {
    workItemTypes { nodes { id name } }
  }
}'

# 親 Issue の GID を取得（gid://gitlab/WorkItem/XXXXXXXXX 形式）
glab api graphql -f query='
{
  project(fullPath: "GROUP/PROJECT") {
    workItems(iids: ["21","22","23"]) {
      nodes { id iid title }
    }
  }
}'
```

```bash
# ヘルパースクリプトを用意すると繰り返し作業が楽になる
cat > /tmp/create_task.sh << 'SCRIPT'
#!/bin/bash
PARENT_GID=$1   # gid://gitlab/WorkItem/XXXXXXXXX
TITLE=$2
DESC=$3
TASK_TYPE="gid://gitlab/WorkItems::Type/5"

TITLE_ESC=$(echo "$TITLE" | python3 -c "import sys,json; print(json.dumps(sys.stdin.read().strip()))")
DESC_ESC=$(echo "$DESC"  | python3 -c "import sys,json; print(json.dumps(sys.stdin.read().strip()))")

glab api graphql -f query="mutation {
  workItemCreate(input: {
    projectPath: \"GROUP/PROJECT\",
    title: $TITLE_ESC,
    description: $DESC_ESC,
    workItemTypeId: \"$TASK_TYPE\",
    hierarchyWidget: { parentId: \"$PARENT_GID\" }
  }) {
    workItem { id iid title }
    errors
  }
}" | python3 -c "
import sys,json; d=json.load(sys.stdin)
wi=d['data']['workItemCreate']['workItem']
errs=d['data']['workItemCreate']['errors']
print('ERROR:', errs) if errs else print(f'Created #{wi[\"iid\"]}: {wi[\"title\"]}')
"
SCRIPT
chmod +x /tmp/create_task.sh

# 使用例（説明文は spec-driven.md「L3 Task Spec」に従う。冒頭の親参照行は必須）
# 本文はファイルに実改行で書き $(cat) で渡す。ヘルパーの json.dumps が実改行・`>`・バッククォートを安全にエスケープする。
# 二重引用符内の "\n" は実改行にならず blockquote/見出しが描画されないため使わない。
cat > /tmp/task_desc.md << 'EOF'
> 親: #21 ／ 寄与する親AC: 1番目

## 背景・目的
...

## 概要
...

## 受け入れ条件（AC）
- [ ] Given <状態>, When <実装>, Then <テストが緑>

## 完了条件（DoD）
- [ ] AC に対応するテストが緑
EOF

/tmp/create_task.sh "gid://gitlab/WorkItem/189387663" \
  "タスク1-1: DB マイグレーション" \
  "$(cat /tmp/task_desc.md)"
```

> Work Item Task は GitLab の全エディションで利用できる（Epic と異なり Premium 不要）。

---

## ステップ4: 完全性チェック（起票の証拠）

```bash
# Milestone に紐づく Issue 一覧
glab api "projects/GROUP%2FPROJECT/milestones/$MILESTONE_ID/issues" 2>&1 | \
  python3 -c "
import sys,json
for i in json.load(sys.stdin):
    print(f'#{i[\"iid\"]} [{i[\"state\"]}] {i[\"title\"][:60]}')
"

# 親子（Issue → Task）の階層を確認
glab api graphql -f query='
{
  project(fullPath: "GROUP/PROJECT") {
    workItems(iids: ["21","22","23"]) {
      nodes {
        iid title
        widgets {
          ... on WorkItemWidgetHierarchy {
            children { nodes { iid title } }
          }
        }
      }
    }
  }
}' | python3 -c "
import sys,json
d=json.load(sys.stdin)
for issue in sorted(d['data']['project']['workItems']['nodes'], key=lambda x: int(x['iid'])):
    print(f'Issue #{issue[\"iid\"]}: {issue[\"title\"]}')
    for w in issue.get('widgets',[]):
        for c in sorted(w.get('children',{}).get('nodes',[]), key=lambda x: int(x['iid'])):
            print(f'  └── Task #{c[\"iid\"]}: {c[\"title\"]}')
"
```

確認コマンドの結果だけで「完了」にせず、計画と実体を突き合わせる:

- **件数一致**：承認した計画の件数（Milestone / Issue / Task）と、実際に起票された件数が一致するか。**不一致なら未起票を特定して補完**する。
- **orphan 検出**：Task は「`workItemCreate` 自体は成功したが `hierarchyWidget.parentId` での親紐づけが効かず」＝親 Issue の `WorkItemWidgetHierarchy.children` に**現れず欠落しうる**（mutation の `errors` も必ず確認）。各 Task の親子関係を確認し、orphan があれば `workItemUpdate`（`hierarchyWidget: { parentId: "<親 GID>" }`）で**再リンク**する。
- **依存エッジの整合**：起票前ゲートで承認した各依存が、対応する Issue 本文の `> 依存:` 行に書かれているか確認する（欠落は `glab issue update` で補完）。
- **証拠の提示**：起票した Milestone / Issue / Task の iid・URL 一覧と「計画 N 件＝起票 N 件・orphan 0・依存エッジ整合済み」を提示してから完了とする。

---

## クローズ（ファイナライズ）は `/gitlab-finalize` へ

本スキルは**起票まで（ステップ1〜4）**を担う。マージ完了後の **Issue＋子 Task（Work Item）＋（条件付き）Milestone のクローズ（ファイナライズ）は `/gitlab-finalize` スキル**が担う（AC/DoD 充足を確認する**クローズ前ゲート**を含む）。`/dev-tasks` の「クローズ＆ファイナライズ」タスクは `vcsHost=gitlab` のとき `/gitlab-finalize` を呼ぶ。

---

## 要点まとめ

| 操作 | API / コマンド | 備考 |
|------|---------------|------|
| Milestone 作成 | `glab api projects/GROUP%2FPROJECT/milestones --method POST` | プロジェクトレベル・全エディション |
| Issue 作成 | `glab issue create --repo GROUP/PROJECT --milestone <title>` | 作成時に Milestone 割り当て |
| Work Item Type 確認 | GraphQL `workItemTypes` | Task = `WorkItems::Type/5` |
| Issue の GID 取得 | GraphQL `workItems(iids:[...])` | `gid://gitlab/WorkItem/...` 形式 |
| 子 Task 作成 | GraphQL `workItemCreate` + `hierarchyWidget` | 本 SKILL のヘルパースクリプトを流用 |
| 構造確認 | Milestone issues API ＋ GraphQL `WorkItemWidgetHierarchy` | — |

## GitHub との対応関係

| 概念 | GitLab（本スキル） | GitHub（github-planning） |
|------|-------------------|--------------------------|
| グルーピング | Milestone | Milestone |
| 仕様 | Issue | Issue |
| 子タスク | Work Item Task（GraphQL） | Sub-issue（REST API） |

## ハマりどころ

- **Epic は使わない**: Epic はグループレベル＋Premium/Ultimate 専用。Free/CE で 403/404 になるため、本スキルは Milestone を使う。Epic 環境で Epic にまとめたい場合のみ、起票後に任意で紐づける。
- **プロジェクトパスの URL エンコード**: `glab api projects/...` ではパスの `/` を `%2F` にする（`GROUP%2FPROJECT`）。
- **Milestone 割り当ては作成時に**: `glab issue create --milestone <title>` で作成時に割り当てれば、後からの API 紐づけは不要。
- **shell 特殊文字**: Issue 説明文に `` ` `` や `*` が含まれると glob 展開される。説明文はファイルに書いてから変数に読み込む。
- **本文構造は spec-driven.md が正**: 各 Level の節構成・粒度・親参照行は本スキルに重複定義せず `.claude/rules/spec-driven.md`（L1 Milestone Spec / L2 Issue Spec / L3 Task Spec）に従う。起票前に必ず読む
- **起票は冪等でない（再実行注意）**: 再実行・途中失敗で Milestone/Issue が**重複作成**される。作成前に同タイトルの既存をチェックし（Milestone: `glab api projects/GROUP%2FPROJECT/milestones --jq '.[].title'`／Issue: `glab issue list --repo GROUP/PROJECT --search "<タイトル>"`）、あれば再利用するかユーザーに確認してから作成する。起票前ゲートで件数・起票先を確認することも重複の早期検知に効く
- **自動クローズはデフォルトブランチ限定**: MR 本文の `Closes #N` は**デフォルトブランチへのマージ時のみ**発火し L2 Issue 1 件しか閉じない。default=main の Git Flow に備え、マージ後は必ず `/gitlab-finalize` で Issue＋子 Task＋（該当時）Milestone を明示クローズする
- **子 Task のクローズは GID が要る**: `workItemUpdate(stateEvent: CLOSE)` は iid ではなく `gid://gitlab/WorkItem/<id>` を取る。階層クエリで各子の `id`（GID）と `state` を取得し OPEN のものだけ閉じる
