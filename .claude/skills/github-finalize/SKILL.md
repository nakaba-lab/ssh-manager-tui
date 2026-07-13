---
name: github-finalize
description: マージ完了後に GitHub の Issue＋子 Sub-issue＋（条件付き）Milestone を、AC/DoD 充足を確認したうえで明示クローズ（ファイナライズ）するとき使用する。「GitHub の Issue をクローズして」「GitHub をファイナライズして」「マージしたので Issue を閉じて」が発火ワード。起票（Milestone/Issue/Sub-issue 作成）は /github-planning。GitLab のクローズは /gitlab-finalize。
---

GitHub の **L2 Issue＋その子 Sub-issue＋（条件付き）Milestone** を、AC/DoD の充足を確認したうえで明示的にクローズ（ファイナライズ）します。`/dev-tasks` の「クローズ＆ファイナライズ」タスクから（`vcsHost=github` のとき）本スキルを呼びます。起票（Milestone/Issue/Sub-issue 作成）は `/github-planning` が担います（本スキルは起票しない）。

> **なぜ明示クローズが要るか**: PR 本文の `Closes #N` 自動クローズは **リポジトリのデフォルトブランチへのマージ時のみ**発火し、閉じるのは **L2 Issue 1 件だけ**。Git Flow（feature→develop）で default=main の場合は L2 Issue すら閉じず、Milestone・子 Sub-issue は常に閉じない。本スキルは default ブランチに関わらず階層をクローズする。`Closes #N` は linked-PR 表示と default=develop 時の自動クローズのために**残す**。

> **冪等（必須）**: 各クローズは **OPEN のものだけ**を対象にする。`Closes #N` が既に L2 Issue を閉じていても本スキルは安全な no-op になる。

> **Milestone は L2 Issue のみカウント**: 本テンプレの planning は Milestone を L2 Issue だけに割り当てる（子 Sub-issue は Milestone 未割り当て）。よって `open_issues` は L2 の開閉だけを反映する。

---

## 事前確認

```bash
# gh 認証確認
gh auth status

# リポジトリパスを変数として保持（gh auth 済みであれば確実に取得できる）
REPO=$(gh repo view --json nameWithOwner -q .nameWithOwner)
echo "リポジトリ: $REPO"
```

対象 Issue 番号を `ISSUE_NUMBER` に確定する（マージした L2 Issue。ブランチ名 `feature/42-...` の番号や `/dev-tasks` の対象 Issue から特定する）。

---

## ステップ1: DoD 最終同期＋クローズ前ゲート（クローズ本体の前に必ず通す）

state（open/closed）だけでクローズすると AC/DoD のチェックボックスが `[ ]` のまま閉じてしまう。**下のクローズ本体（ステップ2）を走らせる前に**、本文の AC/DoD を実態へ同期し、未充足が残らないか確認する。

1. **DoD 最終同期**: マージ済みでこの時点で達成された L2 Issue の DoD（特に「PR / MR 説明に `Closes #<番号>` が含まれている」）を `[x]` にする。本文をファイル経由で書き換えて更新する:

   ```bash
   gh api repos/$REPO/issues/$ISSUE_NUMBER --jq .body > /tmp/issue_body.md
   # /tmp/issue_body.md の該当行（- [ ] … Closes #… 等）を [x] に直す
   gh issue edit "$ISSUE_NUMBER" --repo "$REPO" --body-file /tmp/issue_body.md
   ```

   Milestone を閉じる見込みなら、L1 Milestone 本文（`gh api repos/$REPO/milestones/<番号> --jq .description`）の AC/DoD も同様に `[x]` へ同期し、`gh api repos/$REPO/milestones/<番号> --method PATCH -f "description=$(cat /tmp/ms_body.md)"` で更新する。

2. **充足チェック（ゲート）**: クローズ対象（L2 Issue・OPEN な L3 子・該当 Milestone）の本文に未チェック `- [ ]` が残っていないか洗い出す:

   ```bash
   gh api repos/$REPO/issues/$ISSUE_NUMBER --jq .body | grep -n '^- \[ \]' || echo "L2 未チェックなし"
   for n in $(gh api repos/$REPO/issues/$ISSUE_NUMBER/sub_issues --jq '.[]|select(.state=="open")|.number'); do
     gh api repos/$REPO/issues/$n --jq .body | grep -n '^- \[ \]' && echo "↑ Sub-issue #$n に未チェックあり" || echo "Sub-issue #$n 未チェックなし"
   done
   # Milestone を閉じる見込みなら Milestone 本文（description）の未チェックも確認
   MS=$(gh api repos/$REPO/issues/$ISSUE_NUMBER --jq '.milestone.number // empty')
   [ -n "$MS" ] && { gh api repos/$REPO/milestones/$MS --jq .description | grep -n '^- \[ \]' && echo "↑ Milestone #$MS に未チェックあり" || echo "Milestone #$MS 未チェックなし"; }
   ```

   - 達成済みなのに未チェックの項目は手順1の要領で `[x]` に同期する。
   - **条件付き項目（「（UI 変更時）」「（UI 不具合の場合）」等）が当該 Issue に当てはまらない**場合（UI を伴わない変更・`frontendDir: "none"` の明示）は、その項目を `[x]`（末尾に ` N/A` を付してよい）にして充足扱いにする。当てはまる（UI 変更を含む）場合は実際にスクリーンショット確認（`docs/screenshots/` の保存分）・`frontend-reviewer` 通過を経てから `[x]` にする。`frontendDir` が空（未確認）のまま「N/A」にしない（kind=web なら設定不備）。
   - **本当に未達成の項目が残る**なら `AskUserQuestion`（header: `未充足のままクローズ`、options:「同期して充足させる／未充足のままクローズ／中止」）で確認してからクローズ本体に進む。**勝手に state だけで閉じない**。

ゲートを通したら、以下のクローズ本体を実行する。

---

## ステップ2: クローズ本体（OPEN のものだけ・冪等）

```bash
REPO=$(gh repo view --json nameWithOwner -q .nameWithOwner)
ISSUE_NUMBER=<マージした L2 Issue 番号>

# 1) L3 子（Sub-issue）のうち OPEN のものだけ閉じる（冪等）
for n in $(gh api repos/$REPO/issues/$ISSUE_NUMBER/sub_issues \
            --jq '.[]|select(.state=="open")|.number'); do
  gh issue close "$n" --repo "$REPO" --reason completed
  echo "closed sub-issue #$n"
done

# 2) L2 Issue 本体が OPEN なら閉じる（冪等。Closes #N で既に closed なら no-op）
STATE=$(gh api repos/$REPO/issues/$ISSUE_NUMBER --jq .state)
if [ "$STATE" = "open" ]; then
  gh issue close "$ISSUE_NUMBER" --repo "$REPO" --reason completed
  echo "closed issue #$ISSUE_NUMBER"
else
  echo "issue #$ISSUE_NUMBER は既に closed（no-op）"
fi

# 3) この Issue が属する Milestone 番号（無ければ空）
MILESTONE_NUMBER=$(gh api repos/$REPO/issues/$ISSUE_NUMBER --jq '.milestone.number // empty')

# 4) Milestone の OPEN issue 数を確認
if [ -n "$MILESTONE_NUMBER" ]; then
  OPEN=$(gh api repos/$REPO/milestones/$MILESTONE_NUMBER --jq .open_issues)
  echo "Milestone #$MILESTONE_NUMBER の open issue 数: $OPEN"
fi
```

---

## ステップ3: Milestone クローズの判断（`AskUserQuestion` を 1 度）

`$OPEN` が **0** のときのみ提示する。

**クローズ前ゲート（L1 本文・無条件）**: `$OPEN==0` で Milestone を閉じうるここでは、提示の前に**必ず** Milestone 本文（description）の AC/DoD を同期・確認する（ステップ1の Milestone 同期を「閉じる見込み」の自己判断に委ねず、`open_issues==0` のここで無条件化する＝L1 AC/DoD が `[ ]` のまま state だけで閉じる穴を塞ぐ）:

```bash
gh api repos/$REPO/milestones/$MILESTONE_NUMBER --jq .description > /tmp/ms_body.md
# /tmp/ms_body.md の達成済みで未チェックの AC/DoD（- [ ] …）を [x] に直して書き戻す
gh api repos/$REPO/milestones/$MILESTONE_NUMBER --method PATCH -f "description=$(cat /tmp/ms_body.md)" >/dev/null
# 残った未チェックを洗い出す
gh api repos/$REPO/milestones/$MILESTONE_NUMBER --jq .description | grep -n '^- \[ \]' && echo "↑ Milestone 本文に未充足あり" || echo "Milestone 本文 充足"
```

未充足が残るなら `AskUserQuestion`（header: `未充足のままクローズ`、options:「同期して充足させる／未充足のままクローズ／中止」）で確認してから下の Milestone クローズ判断に進む（state だけで無条件には閉じない）。

- header: `Milestone クローズ`
- question:「Milestone #<番号>「<タイトル>」の open issue が 0 件になりました。Milestone をクローズしますか？（可逆。後で `state=open` で再開できます）」
- options:「クローズする（推奨）／開いたままにする」

「クローズする」を選んだときのみ実行:

```bash
gh api repos/$REPO/milestones/$MILESTONE_NUMBER --method PATCH -f state=closed \
  --jq '"closed milestone #\(.number): \(.title)"'
```

**Milestone をクローズした場合の再同期案内（必ず提示）**: Milestone 完了はスタックの節目になりやすい。Milestone を実際にクローズしたときは、次の案内を 1 度提示する（このスキルからは自動起動しない＝利用者が `/project-resync` を実行する）:

> ✅ Milestone をクローズしました。この区切りでスタック（言語・フレームワーク・ビルド系・構成）が変わっていれば、`/project-resync` を実行すると自動化（hooks/サブエージェント/スキル/プラグイン/MCP）・CLAUDE.md・プロジェクトプロファイル（`commands`/`checks`/`languages` 等）を現行スタックへ再同期できます。

`$OPEN` が 1 以上、または Milestone 紐づけが無い場合は閉じない（Issue＋子のクローズのみで完了）。

---

## 完了の証拠（提示してから完了とする）

```bash
gh api repos/$REPO/issues/$ISSUE_NUMBER --jq '"#\(.number) [\(.state)] \(.title)"'
gh api repos/$REPO/issues/$ISSUE_NUMBER/sub_issues \
  --jq '.[] | "  └── #\(.number) [\(.state)] \(.title)"'
[ -n "$MILESTONE_NUMBER" ] && gh api repos/$REPO/milestones/$MILESTONE_NUMBER \
  --jq '{number:.number, state:.state, open:.open_issues, closed:.closed_issues}'
```

L2 Issue＝`closed`、全 L3 子＝`closed`、（該当時）Milestone＝`closed` を示す出力を提示してから「クローズ＆ファイナライズ」タスクを完了にする。

---

## 関連

- 起票（Milestone/Issue/Sub-issue 作成）: `/github-planning`
- GitLab のクローズ: `/gitlab-finalize`
- 本文構造（L1/L2/L3 の AC/DoD）の真実源: `.claude/rules/spec-driven.md`
- 開発フロー全体での位置（ステップ11 クローズ）: `.claude/rules/spec-driven.md` の実装フロー／`/dev-tasks`
