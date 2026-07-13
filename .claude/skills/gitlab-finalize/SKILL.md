---
name: gitlab-finalize
description: マージ完了後に GitLab の Issue＋子 Task（Work Item）＋（条件付き）Milestone を、AC/DoD 充足を確認したうえで明示クローズ（ファイナライズ）するとき使用する。「GitLab の Issue をクローズして」「GitLab をファイナライズして」「マージしたので Issue を閉じて」が発火ワード。起票（Milestone/Issue/Task 作成）は /gitlab-planning。GitHub のクローズは /github-finalize。
---

GitLab の **L2 Issue＋その子 Task（Work Item）＋（条件付き）Milestone** を、AC/DoD の充足を確認したうえで明示的にクローズ（ファイナライズ）します。`/dev-tasks` の「クローズ＆ファイナライズ」タスクから（`vcsHost=gitlab` のとき）本スキルを呼びます。起票（Milestone/Issue/Task 作成）は `/gitlab-planning` が担います（本スキルは起票しない）。

> **なぜ明示クローズが要るか**: MR 本文の `Closes #N` 自動クローズは **プロジェクトのデフォルトブランチへのマージ時のみ**発火し、閉じるのは **L2 Issue 1 件だけ**。Git Flow（feature→develop）で default=main の場合は L2 Issue すら閉じず、Milestone・子 Task は常に閉じない。本スキルは default ブランチに関わらず階層をクローズする。`Closes #N` は linked-MR 表示と default=develop 時の自動クローズのために**残す**。

> **冪等（必須）**: 各クローズは **OPEN（GraphQL=OPEN / REST=opened）のものだけ**を対象にする。`Closes #N` が既に L2 Issue を閉じていても安全な no-op。

> **Milestone は L2 Issue のみカウント**: planning は Milestone を L2 Issue だけに割り当てる（子 Task は未割り当て）。`milestones/<ID>/issues?state=opened` は L2 の開閉だけを反映する。

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

対象 Issue の iid を `ISSUE_IID` に確定する（マージした L2 Issue。ブランチ名 `feature/42-...` の番号や `/dev-tasks` の対象 Issue から特定する）。

---

## ステップ1: DoD 最終同期＋クローズ前ゲート（クローズ本体の前に必ず通す）

state（opened/closed）だけでクローズすると AC/DoD のチェックボックスが `[ ]` のまま閉じてしまう。**下のクローズ本体（ステップ2）を走らせる前に**、本文の AC/DoD を実態へ同期し、未充足が残らないか確認する。

1. **DoD 最終同期**: マージ済みでこの時点で達成された L2 Issue の DoD（特に「MR 説明に `Closes #<番号>` が含まれている」）を `[x]` にする。本文を取得して書き換え、`glab issue update` で更新する:

   ```bash
   glab api "projects/GROUP%2FPROJECT/issues/$ISSUE_IID" \
     | python3 -c "import sys,json;print(json.load(sys.stdin)['description'])" > /tmp/issue_body.md
   # /tmp/issue_body.md の該当行（- [ ] … Closes #… 等）を [x] に直す
   glab issue update "$ISSUE_IID" --repo GROUP/PROJECT --description "$(cat /tmp/issue_body.md)"
   ```

   Milestone を閉じる見込みなら、Milestone description（`glab api projects/GROUP%2FPROJECT/milestones/<ID>`）の AC/DoD も同様に同期して `--method PUT -f "description=$(cat /tmp/ms_body.md)"` で更新する。

2. **充足チェック（ゲート）**: クローズ対象（L2 Issue・OPEN な子 Task・該当 Milestone）の本文に未チェック `- [ ]` が残っていないか洗い出す（L3 Task 本文は「AC/DoD 同期」タスクで既に `[x]` 済みのはず。残っていれば同期）:

   ```bash
   # L2 Issue 本文の未チェックを洗い出す
   glab api "projects/GROUP%2FPROJECT/issues/$ISSUE_IID" \
     | python3 -c "import sys,json;b=json.load(sys.stdin)['description'];u=[l for l in b.splitlines() if l.strip().startswith('- [ ]')];print(chr(10).join(u) if u else 'L2 未チェックなし')"

   # OPEN な子 Task の本文（description）の未チェックも洗い出す
   glab api graphql -f query="
   { project(fullPath: \"GROUP/PROJECT\") { workItems(iids: [\"$ISSUE_IID\"]) {
       nodes { widgets { ... on WorkItemWidgetHierarchy {
         children { nodes { iid state
           widgets { ... on WorkItemWidgetDescription { description } } } } } } } } }
   }" | python3 -c "
import sys,json
for n in json.load(sys.stdin)['data']['project']['workItems']['nodes']:
    for w in n.get('widgets',[]):
        for c in w.get('children',{}).get('nodes',[]):
            if c.get('state')!='OPEN': continue
            desc=next((cw.get('description') or '' for cw in c.get('widgets',[]) if 'description' in cw),'')
            un=[l for l in desc.splitlines() if l.strip().startswith('- [ ]')]
            print(f'Task #{c[\"iid\"]}: '+(chr(10).join(un) if un else '未チェックなし'))
"
   ```

   - 達成済みなのに未チェックの項目は手順1の要領で `[x]` に同期する。
   - **条件付き項目（「（UI 変更時）」「（UI 不具合の場合）」等）が当該 Issue に当てはまらない**場合（UI を伴わない変更・`frontendDir: "none"` の明示）は、その項目を `[x]`（末尾に ` N/A` を付してよい）にして充足扱いにする。当てはまる（UI 変更を含む）場合は実際にスクリーンショット確認（`docs/screenshots/` の保存分）・`frontend-reviewer` 通過を経てから `[x]` にする。`frontendDir` が空（未確認）のまま「N/A」にしない（kind=web なら設定不備）。
   - **本当に未達成の項目が残る**なら `AskUserQuestion`（header: `未充足のままクローズ`、options:「同期して充足させる／未充足のままクローズ／中止」）で確認してからクローズ本体に進む。**勝手に state だけで閉じない**。

ゲートを通したら、以下のクローズ本体を実行する。

---

## ステップ2: クローズ本体（OPEN のものだけ・冪等）

```bash
ISSUE_IID=<マージした L2 Issue の iid>

# 1) 親 Issue の GID/state と、子 Task の GID/state を取得（OPEN の子だけ後で閉じる）
glab api graphql -f query="
{
  project(fullPath: \"GROUP/PROJECT\") {
    workItems(iids: [\"$ISSUE_IID\"]) {
      nodes {
        id iid state
        widgets {
          ... on WorkItemWidgetHierarchy {
            children { nodes { id iid state title } }
          }
        }
      }
    }
  }
}" | python3 -c "
import sys,json
node=json.load(sys.stdin)['data']['project']['workItems']['nodes'][0]
print('PARENT_GID='+node['id'])
print('PARENT_STATE='+node['state'])
for w in node.get('widgets',[]):
    for c in w.get('children',{}).get('nodes',[]):
        if c['state']=='OPEN':
            print('OPEN_CHILD='+c['id']+' #'+c['iid'])
"
```

```bash
# 2) OPEN な子 Task を 1 件ずつクローズ（冪等。上で得た各 OPEN_CHILD の GID を渡す）
close_workitem() {
  local GID=$1
  glab api graphql -f query="mutation {
    workItemUpdate(input: { id: \"$GID\", stateEvent: CLOSE }) {
      workItem { id iid state }
      errors
    }
  }" | python3 -c "
import sys,json
d=json.load(sys.stdin)['data']['workItemUpdate']
print('ERROR:', d['errors']) if d['errors'] else print(f\"closed task #{d['workItem']['iid']} -> {d['workItem']['state']}\")
"
}
# 例: close_workitem \"gid://gitlab/WorkItem/123456789\"

# 3) L2 Issue 本体が opened なら閉じる（冪等。Closes #N で既に closed なら no-op）
STATE=$(glab api "projects/GROUP%2FPROJECT/issues/$ISSUE_IID" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['state'])")
if [ "$STATE" = "opened" ]; then
  glab issue close "$ISSUE_IID" --repo GROUP/PROJECT
  echo "closed issue #$ISSUE_IID"
else
  echo "issue #$ISSUE_IID は既に closed（no-op）"
fi
```

> Issue 本体も Work Item なので、上の `workItemUpdate(stateEvent: CLOSE)` に PARENT_GID を渡しても閉じられる（`glab issue close` と同義）。どちらか一方で良い。

---

## ステップ3: Milestone クローズの判断（`AskUserQuestion` を 1 度）

```bash
MILESTONE_ID=$(glab api "projects/GROUP%2FPROJECT/issues/$ISSUE_IID" \
  | python3 -c "import sys,json;m=json.load(sys.stdin).get('milestone');print(m['id'] if m else '')")
if [ -n "$MILESTONE_ID" ]; then
  OPEN=$(glab api "projects/GROUP%2FPROJECT/milestones/$MILESTONE_ID/issues?state=opened" \
    | python3 -c "import sys,json;print(len(json.load(sys.stdin)))")
  echo "Milestone $MILESTONE_ID の opened issue 数: $OPEN"
fi
```

**クローズ前ゲート（L1 本文・無条件）**: `$OPEN==0` で Milestone を閉じうるここでは、提示の前に**必ず** Milestone description の AC/DoD を同期・確認する（ステップ1の Milestone 同期を「閉じる見込み」の自己判断に委ねず、`opened==0` のここで無条件化する＝L1 AC/DoD が `[ ]` のまま state だけで閉じる穴を塞ぐ）:

```bash
glab api "projects/GROUP%2FPROJECT/milestones/$MILESTONE_ID" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['description'])" > /tmp/ms_body.md
# /tmp/ms_body.md の達成済みで未チェックの AC/DoD（- [ ] …）を [x] に直して書き戻す
glab api projects/GROUP%2FPROJECT/milestones/$MILESTONE_ID --method PUT -f "description=$(cat /tmp/ms_body.md)" >/dev/null
# 残った未チェックを洗い出す
glab api "projects/GROUP%2FPROJECT/milestones/$MILESTONE_ID" \
  | python3 -c "import sys,json;b=json.load(sys.stdin)['description'];u=[l for l in b.splitlines() if l.strip().startswith('- [ ]')];print(chr(10).join(u) if u else 'Milestone 本文 充足')"
```

未充足が残るなら `AskUserQuestion`（header: `未充足のままクローズ`、options:「同期して充足させる／未充足のままクローズ／中止」）で確認してから下の Milestone クローズ判断に進む（state だけで無条件には閉じない）。

`$OPEN` が **0** のときのみ `AskUserQuestion`:

- header: `Milestone クローズ`
- question:「Milestone「<タイトル>」の opened issue が 0 件になりました。Milestone をクローズしますか？（可逆。後で `state_event=activate` で再開できます）」
- options:「クローズする（推奨）／開いたままにする」

「クローズする」のときのみ実行:

```bash
glab api projects/GROUP%2FPROJECT/milestones/$MILESTONE_ID --method PUT -f state_event=close \
  | python3 -c "import sys,json;m=json.load(sys.stdin);print(f\"closed milestone {m['id']}: {m['title']} -> {m['state']}\")"
```

**Milestone をクローズした場合の再同期案内（必ず提示）**: Milestone 完了はスタックの節目になりやすい。Milestone を実際にクローズしたときは、次の案内を 1 度提示する（このスキルからは自動起動しない＝利用者が `/project-resync` を実行する）:

> ✅ Milestone をクローズしました。この区切りでスタック（言語・フレームワーク・ビルド系・構成）が変わっていれば、`/project-resync` を実行すると自動化（hooks/サブエージェント/スキル/プラグイン/MCP）・CLAUDE.md・プロジェクトプロファイル（`commands`/`checks`/`languages` 等）を現行スタックへ再同期できます。

---

## 完了の証拠（提示してから完了とする）

```bash
glab api graphql -f query="
{ project(fullPath: \"GROUP/PROJECT\") {
    workItems(iids: [\"$ISSUE_IID\"]) {
      nodes { iid state title
        widgets { ... on WorkItemWidgetHierarchy { children { nodes { iid state title } } } } } } }
}" | python3 -c "
import sys,json
n=json.load(sys.stdin)['data']['project']['workItems']['nodes'][0]
print(f\"Issue #{n['iid']} [{n['state']}] {n['title']}\")
for w in n.get('widgets',[]):
    for c in w.get('children',{}).get('nodes',[]):
        print(f\"  └── Task #{c['iid']} [{c['state']}] {c['title']}\")
"

# （該当時）Milestone の最終状態も提示する（$MILESTONE_ID はステップ3 で取得済み）
[ -n "$MILESTONE_ID" ] && glab api "projects/GROUP%2FPROJECT/milestones/$MILESTONE_ID" \
  | python3 -c "import sys,json;m=json.load(sys.stdin);print(f\"Milestone #{m['id']} [{m['state']}] {m['title']}\")"
```

L2 Issue＝`CLOSED`、全 L3 子＝`CLOSED`、（該当時）Milestone＝`closed` を示す出力を提示してから「クローズ＆ファイナライズ」タスクを完了にする。

---

## 関連

- 起票（Milestone/Issue/Task 作成）: `/gitlab-planning`
- GitHub のクローズ: `/github-finalize`
- 本文構造（L1/L2/L3 の AC/DoD）の真実源: `.claude/rules/spec-driven.md`
- 開発フロー全体での位置（ステップ11 クローズ）: `.claude/rules/spec-driven.md` の実装フロー／`/dev-tasks`
