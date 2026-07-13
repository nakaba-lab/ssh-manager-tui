---
name: release-tasks
description: リリース工程（リリース判定→総合テスト→受入チェック→リリースノート確定→release ブランチ・main 反映＋タグ→develop への戻しマージ→リリース後スモーク→マニュアル最終確認）を直列タスクとして敷設し進行するとき使用する。「リリースして」「リリース準備して」「リリース作業を始めて」「リリースタスクを敷いて」が発火ワード。CHANGELOG 確定・リリースノート生成のみなら /release-notes、個別 Issue の開発フローは /dev-tasks。
disable-model-invocation: true
---

リリース工程（リリース判定→総合テスト→受入チェック→リリースノート確定→release ブランチ・`main` 反映＋タグ→`develop` への戻しマージ→リリース後スモーク→マニュアル最終確認）を、飛ばし防止の**直列タスク**（`TaskCreate`＋`blockedBy`）として敷設し着手します。Milestone 完了時（`/dev-tasks` の「クローズ＆ファイナライズ」がリリースを提案する）や、リリースを始めたいタイミングで利用者が実行します。

> 役割分担: **このスキルはリリース工程のタスク敷設オーケストレータ**。CHANGELOG の版確定・SemVer 提案は `/release-notes`、PR/MR 作成とマージは `/ship` が担い、本スキルはそれらを**タスクとして参照**する。ブランチ・タグ・マージの規則は `.claude/rules/git-workflow.md`、テストレベル（総合・受入・性能）の定義は `.claude/rules/testing-strategy.md`、リリース後の運用（障害対応・マニュアル体系）は `.claude/rules/operations.md` が正で、本スキルは工程の**順序と完了条件**だけを定める（重複定義しない）。

$ARGUMENTS がある場合はリリース対象の指定として扱います（数値なら Milestone 番号、`X.Y.Z` 形式なら目標バージョン。例: `/release-tasks 12`・`/release-tasks 1.2.0`）。

## 進行管理（重要）

`/release-tasks` はリリース工程を **1 タスク = 1 役割**の直列タスク（`TaskCreate`＋`blockedBy`）に分解して飛ばしを防ぐ。`blockedBy` は着手できる**順序**だけを固定するので、無作業 completed・隣接タスクの 1 ターンマージを防ぐため各タスクに以下のロックを埋める（機械的強制ではなく誘導文。`TaskUpdate` にフックは無い）。

- 各タスクは 1 役割。description 末尾に **`【完了条件】<観測可能な単一の証拠：コマンド出力／テスト記録の差分／PR・MR URL／タグの出力>`** を必ず置き、その証拠を提示するまで `completed` にしない（提示できなければ `in_progress` のまま不足を報告）。自己申告（「完了しました」）は証拠ではない。
- **自律区間（リリース判定〜総合テスト、リリース後スモーク〜マニュアル最終確認）は AskUserQuestion による構造的な停止が無い**（`/goal` が無人で周回しうる）。証拠行がここでの**唯一の歯止め**なので必ず守る。
- **`【自律対象外】`＝人間ゲート**（受入チェックリスト〔該当プロジェクトのみ。非該当は no-op 証拠で通過〕・release ブランチ作成と main 反映・develop への戻しマージ）。利用者の手動実行・承認を得るまで `completed` にしない。それ以外は auto。なお「リリースノート確定」は auto に含まれるが、`/release-notes` は手動発火のみ（`disable-model-invocation: true`）のため**実行そのものは利用者に案内**し、その結果（CHANGELOG 確定・版番号・リリース判定チェックリスト）を検証してから先へ進む（auto 内の人間ゲート＝【自律対象外】ではない）。

### 線形チェーン不変条件

- 各タスクは直前タスク **1 本だけ**に `blockedBy`（分岐・合流なし＝単一線形鎖）。
- 条件に該当しない可能性のあるタスク（受入チェックリスト・リリース後スモーク）も**省略せず敷設する**（no-op にも証拠が要る）。チェーンは常に **8 タスク固定**（`/dev-tasks` と違い件数の分岐は無い）。
- `TaskList` を読み、`リリース判定→…→マニュアル最終確認` の単一鎖になっており**役割名と件数（8）**が想定どおりかを名前で 1 つずつ照合してから先頭を `in_progress` にする（手順 4）。

## 手順

### 1. 文脈判定（リリース対象と前提の確認）

- **リリース対象を確定する**。**`$ARGUMENTS` が与えられていればそれを最優先**とする（数値= Milestone 番号、`X.Y.Z`= 目標バージョン）。無ければ open な Milestone（`vcsHost`〔CLAUDE.md「プロジェクト設定」〕に応じて gh/glab）から推定し、それも不明ならユーザーに確認する。Milestone を運用していないプロジェクトは「直前タグ以降の `develop` 差分」をリリース対象とする。
  ```bash
  git fetch origin --tags
  git tag --list 'v*' --sort=-v:refname | head -1   # 直前リリースタグ（無ければ初リリース＝全履歴が対象）
  git branch --show-current                          # リリース工程は develop の最新から始める
  git status --short                                 # 作業ツリーがクリーンか（未収束の作業は先に /dev-tasks で片付ける）
  ```
- **暫定の次バージョンを見積もる**: 直前タグと `CHANGELOG.md` の `[Unreleased]`（Breaking→MAJOR／`feat`→MINOR／`fix`・`perf` のみ→PATCH。`.claude/rules/git-workflow.md` の SemVer 表）から見積もる。総合テスト記録のファイル名に使い、**確定は「リリースノート確定」タスク**（`/release-notes` の提案＋利用者判断）で行う。
- **後続タスクの no-op／実体判定の根拠を先に読む**: UAT 要否（`docs/要件定義書.md`「テスト方針」の合意）／デプロイの有無（CLAUDE.md「プロジェクト設定」の `commands.deploy`・デプロイ雛形 CI〔release-deploy〕の有効性）／Web か（種別 kind。総合テスト・スモークの `playwright` E2E に効く）／docs サイトの有効性（`docsSite.enabled`）。

### 2. リリースタスクを敷設する（`TaskCreate`）

`TaskCreate` ツールで以下の **8 タスク**を順番に作成する（description には呼ぶべきスキル・参照すべき規則ファイル名を含める）。**`TaskCreate` の subject はタスク表の subject を一字一句使い、description は表の description セルを逐語転記する（要約・言い換え・省略をしない）**。表が長い場合も切り詰めない — 完了条件・人間ゲート表記が落ちると進行の歯止めが消える。

| subject | description |
|---------|-------------|
| リリース判定（前提確認） | リリース対象（文脈判定で確定した Milestone または直前タグ以降の `develop` 差分）について 3 点を確認する: (1) **Milestone の全 Issue がクローズ済み**（GitHub: `gh api repos/$REPO/milestones/<番号>` の `open_issues`＝0／GitLab: Milestone の open issue 一覧が 0 件。Milestone 運用が無ければ対象 Issue 群のクローズを個別確認）、(2) **CI が緑**（`develop` の最新コミットで全チェック成功。GitHub: `gh run list --branch develop`／GitLab: パイプライン状態）、(3) **`CHANGELOG.md` の `[Unreleased]` にリリース対象の変更が蓄積**されている（空なら「ドキュメント更新」の取りこぼし＝`/release-notes` の補完で救済できるが、判定時点で把握しておく）。未充足があれば列挙して解消を促し、解消まで `completed` にしない（未クローズ Issue の解消は `/dev-tasks`）。**【完了条件】**3 点それぞれの証拠（open issue 0 件の出力・CI 成功の出力・`[Unreleased]` の内容）を提示。 |
| 総合テスト（システムテスト） | `.claude/rules/testing-strategy.md` の「総合（システム）」に従い、**実環境相当**（ステージング等、本番に近い構成）で通しの検証を行う。基準は `docs/要件定義書.md` の**主要機能・非機能要件（定量目標）**と対象 **L1 Milestone の AC**。実施内容: ① 全テストスイートの実行（`commands.test`＋設定時は `commands.integrationTest`）② Web プロジェクトは `playwright` で**主要ユースケースの E2E**（画面遷移・主要導線の通し操作）③ 定量目標（応答時間・同時利用者数・データ規模）があれば性能テストも合わせて実施し、計測条件と結果を記録に含める。記録は **`docs/test/release-vX.Y.Z-system-test.md`**（実行日時・対象バージョン/コミット・結果・証跡。版番号は文脈判定の暫定版を使い、「リリースノート確定」で版が変わればリネームする）に残す（`docs/` 配下なので Pages に自動公開される）。**【差し戻し】**不合格（AC 未達・定量目標未達）の修正は本チェーン内で直さず、fix Issue を起票して通常の開発フロー（`/dev-tasks`）に戻す（修正マージ後に本タスクを再実行する）。**【完了条件】**記録ファイルの差分（`git diff`／新規ファイル）と全項目の結果（不合格 0 件）を提示。 |
| 受入チェックリスト（UAT・条件付き） | **【自律対象外】（該当プロジェクトのみ・人間ゲート）**。**受託・業務システムなど利用者/発注者による受入確認が要るプロジェクトのみ実体作業**（要否は `docs/要件定義書.md`「テスト方針」の合意に従う。定義は `.claude/rules/testing-strategy.md`「受入（UAT）」）。L1 Milestone の AC を**受入チェックリスト**（確認項目・確認手順・合否欄）に変換して提示し、利用者/発注者の確認を促す。確認完了（全項目合格）の報告を得るまで `completed` にしない。不合格・指摘が出たら fix Issue として起票し（`/dev-tasks`）、解消後に再確認する。**該当しないプロジェクトは no-op 可**＝該当しない根拠（要件定義書「テスト方針」節の記載等）を提示して次へ進む。**【完了条件】**受入チェックリストと確認完了（全項目合格）の報告（no-op の場合は該当しない根拠）を提示。 |
| リリースノート確定 | `/release-notes` で CHANGELOG の版確定（`[Unreleased]`→版番号）・SemVer 提案・**リリース判定チェックリスト**（CI 緑・総合テスト記録・マイグレーション手順・マニュアル更新要否・スモーク項目の確認）を行う。**`/release-notes` は手動発火のみ（`disable-model-invocation: true`）のため本スキルからは自動起動できない**＝利用者に実行を案内し、完了（CHANGELOG 確定の承認・版番号の決定・チェックリスト充足）を確認してから次へ進む（auto 内の人間ゲート。【自律対象外】ではない）。確定した版番号が文脈判定の暫定版と異なれば、総合テスト記録（`docs/test/release-vX.Y.Z-system-test.md`）をリネームする。**【完了条件】**`CHANGELOG.md` の版確定差分・決定した版番号・リリース判定チェックリストの充足結果を提示。 |
| release ブランチ作成と main 反映（**手動ゲート**） | **【自律対象外】不可逆（`main` 反映・タグ push）なので `/goal` では実行しない**。利用者が `.claude/rules/git-workflow.md` に従い手動で実行する: ① `develop` から **`release/X.Y.Z`** ブランチを作成し、リリース準備コミットを積む（`/release-notes` が書き込んだ CHANGELOG の版確定・バージョン表記の更新等。type は `chore`）→ ② `/ship` で `main` へ **Merge Commit** でマージ（squash しない。`/ship` がブランチ型から判定する）→ ③ `git tag vX.Y.Z` → `git push origin vX.Y.Z`（タグ push でデプロイ雛形 CI〔release-deploy〕が起動する構成では、リリース判定チェックリストの充足を確認してから push する）。完了後に本タスクを `completed` にする。**【完了条件】**マージ済み PR/MR の URL と、タグが push 済みの出力（`git ls-remote --tags origin` 等）を提示してから completed。 |
| develop への戻しマージ（**手動ゲート**） | **【自律対象外】**。`release/X.Y.Z` → `develop` を **Merge Commit** で戻す（`.claude/rules/git-workflow.md`「マージ戦略」。`/ship` の「develop への戻しマージ確認」で「今ここで実行する」を選んでいれば実行済み＝その証拠で completed にする。二重実行しない）。**忘れると次リリースが壊れる**: CHANGELOG の版確定・リリース準備コミットが `develop` に乗らず、次の `[Unreleased]` が確定済みの版と衝突する／`main` だけが進んでコンフリクトの温床になる。**【完了条件】**`develop` への戻しマージの証拠（マージコミットを含む `git log develop --merges -1` の出力か PR/MR の URL）を提示してから completed。 |
| リリース後スモーク | **デプロイが実行された場合のみ実体作業**（タグ push でデプロイ雛形 CI〔release-deploy〕が動いた／`commands.deploy` を手動実行した）。デプロイ先で**主要導線の疎通確認**を行う: ヘルスチェック・主要 API の応答・Web は `playwright` で主要画面の表示と導線操作（スモーク項目は `/release-notes` のリリース判定チェックリストで確認した項目を使う）。結果を `docs/test/` に追記する（総合テスト記録への追記か `docs/test/release-vX.Y.Z-smoke.md`）。**疎通不能・主要導線の破壊が見つかったら本番障害**＝incident テンプレートで起票し、切り戻し判断（`.claude/rules/operations.md`・`.claude/rules/git-workflow.md`「ロールバック」）に進む。**デプロイ未設定なら no-op 可**＝根拠（`commands.deploy` が空・デプロイ CI 無効）を提示して次へ進む。**【完了条件】**疎通確認の結果と `docs/test/` の記録差分（no-op の場合は根拠）を提示。 |
| マニュアル・ドキュメント最終確認 | ① **操作マニュアル・運用ガイドの更新要否**を確認する（`docs/操作マニュアル.md`・`docs/運用ガイド.md`。操作・運用手順に影響するリリースで更新する＝`.claude/rules/operations.md`「マニュアル体系」）。未更新が見つかれば `docs/*` ブランチで更新して `develop` へマージする。② **docs サイトのビルド確認**: docs サイト有効時（`docsSite.enabled`）は `cd docs-site && npm run build` または Pages CI（docs-deploy）の成功で、公開が壊れていないことを確認する。**【完了条件】**更新要否の判断結果（更新した場合はその差分）と docs サイトのビルド成功出力（docs サイト無効ならその旨）を提示。 |

### 3. blockedBy チェーンを張る（`TaskUpdate`）

タスク作成後、`TaskUpdate` で各タスクを直前タスクに `blockedBy` で繋ぎ直列化する。

**8 タスク（head = リリース判定）:**
```
総合テスト              ← blockedBy リリース判定
受入チェックリスト       ← blockedBy 総合テスト
リリースノート確定       ← blockedBy 受入チェックリスト
release ブランチ・main 反映 ← blockedBy リリースノート確定
develop への戻しマージ   ← blockedBy release ブランチ・main 反映
リリース後スモーク       ← blockedBy develop への戻しマージ
マニュアル最終確認       ← blockedBy リリース後スモーク
```

> **前のタスクが `completed` になるまで次へ進まない**（`blockedBy` で直列化）。途中の工程（特に総合テスト・戻しマージ）を飛ばさないためのレール。

### 4. 着手と完了報告

- **線形チェーン照合 → 先頭タスクを `in_progress`**：まず `TaskList` を読み、`リリース判定→…→マニュアル最終確認` の単一線形鎖で**役割名と件数（8）**が想定どおりかを名前で照合する。照合が通って初めて先頭タスク「リリース判定」を `TaskUpdate` で `in_progress` にする。`/dev-tasks` と同じく、最初のタスクを着手状態にして初めて敷設完了とする。
- 敷設したタスク一覧（順序）と、いま着手すべき先頭タスクを提示して終了する。
- **利用者に `/goal` の実行を促す（総合テストまで自律）**: `blockedBy` はタスクの**順序**を固定するが、ターンの切れ目で作業が止まる（Claude が制御を返す）のは防げない。Claude Code 組み込みの **`/goal`** で、リリース判定〜総合テストの**可逆な検証作業**を止めずに進められる。**release ブランチ作成と main 反映・develop への戻しマージは不可逆/手動ゲートなので自律駆動に含めない**（タスク表で【自律対象外】と明示。`blockedBy` が外れても `/goal` ではこれらを実行しない）。完了報告の最後に、利用者へ次の実行を促す:
  ```
  /goal 総合テストまで完了して
  ```
  各ターン後に達成判定し、**総合テスト完了（`docs/test/` の記録作成）で自動クリア＝そこで自律実行を停止**する（**停止後は直ちに下記「総合テスト後の進め方」の案内に進む**＝サイレント終了させない）。途中で止めるなら `/goal clear`。UAT 非該当（no-op）のプロジェクトでは `/goal リリースノート確定まで完了して` としてもよい（UAT 該当プロジェクトでは受入チェックリストが【自律対象外】のため範囲に含めない）。
- **総合テストで停止したら、黙って終わらず必ず以降の進め方を案内する（総合テスト後の進め方）**: 総合テストの結果サマリを提示したうえで、残る工程を順に案内・進行する: ① **受入チェックリスト**（該当プロジェクトはチェックリストを提示して利用者/発注者の確認を待つ。非該当は根拠を提示して no-op で `completed`）→ ② 利用者に **`/release-notes`** の実行を案内（「リリースノート確定」。結果を検証して `completed`）→ ③ 利用者が **`release/X.Y.Z` 作成 → `/ship` で `main` へ Merge Commit → タグ push** を実行（完了後に「release ブランチ作成と main 反映」を `completed`）→ ④ **戻しマージ**（`/ship` 内で実行済みならその証拠で `completed`、未実行なら実行を案内）→ ⑤ **リリース後スモークとマニュアル・ドキュメント最終確認**は auto で進め、全タスク `completed` の完了報告でリリース工程を閉じる。

---

> CHANGELOG の確定・リリースノート生成だけが目的（タスク敷設は不要）なら `/release-notes` を直接使う。個別 Issue の開発フロー（worktree→TDD→レビュー→マージ）は `/dev-tasks`。
