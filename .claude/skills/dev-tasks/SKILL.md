---
name: dev-tasks
description: Issue の開発フロー（worktree 作成→実装前設計→テスト先行→実装→リファクタ→コミット→レビュー→マージ→片付け）を直列タスクとして敷設し着手するとき使用する。「Issueの作業を始めて」「この Issue に着手して」「開発タスクを敷いて」「作業フローを立てて」「TDDの作業計画を立てて」が発火ワード。worktree/ブランチの作成のみなら /worktree-new。
disable-model-invocation: true
---

Issue の標準開発フローを、飛ばし防止の**直列タスク**（`TaskCreate`＋`blockedBy`）として敷設し着手します。worktree が未作成なら**チェーンの先頭に worktree 作成（`/worktree-new`）**を置き、worktree のライフサイクルを「作成 → … → 片付け」で対称に括ります。

> 役割分担: **このスキルはタスク敷設のオーケストレータ**。worktree/ブランチの**作成そのもの**は `/worktree-new`（純粋プリミティブ）が担う。本スキルは worktree-new を**タスクとして参照**するだけで直接は呼ばない（単方向・循環なし）。

$ARGUMENTS がある場合は Issue 番号として扱います（例: `/dev-tasks 42`）。

## 進行管理（重要）

`/dev-tasks` は開発フローを **1 タスク = 1 役割**の直列タスク（`TaskCreate`＋`blockedBy`）に分解して飛ばしを防ぐ。`blockedBy` は着手できる**順序**だけを固定するので、無作業 completed・隣接タスクの 1 ターンマージを防ぐため各タスクに以下のロックを埋める（機械的強制ではなく誘導文。`TaskUpdate` にフックは無い）。

- 各タスクは 1 役割。description 末尾に **`【完了条件】<観測可能な単一の証拠：コマンド出力／テスト結果／コミットハッシュ／PR・MR URL／差分>`** を必ず置き、その証拠を提示するまで `completed` にしない（提示できなければ `in_progress` のまま不足を報告）。自己申告（「完了しました」）は証拠ではない。
- **自律区間（テスト先行〜コミット）は AskUserQuestion による無条件の構造的な停止が無い**（`/goal` が無人で周回しうる。「実装（Green）」の曖昧さ規律〔`.claude/rules/alignment.md`〕による `AskUserQuestion` は曖昧さを**検出した場合のみ**の停止で、検出自体が自己判断のため歯止めとしては当てにしない）。証拠行がここでの**唯一の構造的な歯止め**なので必ず守る。
- **`【自律対象外】`＝人間ゲート**（マージ・クリーンアップ）。利用者の手動実行・承認を得るまで `completed` にしない。それ以外は auto。なお「実装前設計」は auto に含まれるが、内部の `AskUserQuestion`（AC ウォークスルー確認・設計オプション比較・設計承認・no-op の照返し＝`.claude/rules/alignment.md`）で人間の承認を取ってから先へ進む（auto 内の人間ゲート＝【自律対象外】ではない）。**さらに「実装前設計」は `/goal` の自律周回の前段に置き、設計承認を得てから `/goal`（テスト先行〜レビュー）を促す**（設計はやり直しコストが最大のフェーズなので、自律モードの片手間の確認に委ねず、方向が固まるまで同期で詰める＝手順 4。auto／【自律対象外】の別〔上記〕とは独立した「`/goal` 周回に含めるか」の区別）。

### 線形チェーン不変条件

- 各タスクは直前タスク **1 本だけ**に `blockedBy`（分岐・合流なし＝単一線形鎖）。
- `TaskList` を読み、`[先頭]→…→クリーンアップ` の単一鎖になっており**役割名と件数（worktree 未作成 13／既存 12）**が想定どおりかを名前で 1 つずつ照合してから先頭を `in_progress` にする（手順 4）。

## 手順

### 1. 文脈判定（Issue と worktree の有無）

- 対象 **Issue 番号**を確定する。**`$ARGUMENTS` が与えられていればそれを最優先**で Issue 番号とする。無ければ現在ブランチ名 `feature/42-...` から推定し、それも不明ならユーザーに確認する。
- 対象 Issue の **worktree/ブランチが既に存在するか**を判定する。**判定基準は「対象 Issue 番号に対応する worktree/ブランチが `git worktree list` に存在するか」**（いま自分がどのブランチに居るかではない）:
  ```bash
  git worktree list            # 各行のブランチ名を対象 Issue 番号でフィルタして判定する
  git branch --show-current    # 参考: 既に対象 Issue の feature/42-... 上か
  ```
  - 対象 Issue の worktree が list に**無ければ「未作成」扱い**（→ 13 タスク）。現在ブランチが別 Issue（例 `feature/99-...`）でも、対象 Issue の worktree が無ければ未作成。
  - 対象 Issue の worktree が list に**有れば「既存」扱い**（→ 12 タスク）。
- 判定結果でチェーンを分岐する（手順 2）。

### 2. 開発タスクを敷設する（`TaskCreate`）

`TaskCreate` ツールで以下を順番に作成する（description には呼ぶべきスキル・エージェント名を含める）。**`TaskCreate` の subject はタスク表の subject を一字一句使い、description は表の description セルを逐語転記する（要約・言い換え・省略をしない）**。表が長い場合も切り詰めない — 完了条件・人間ゲート表記が落ちると進行の歯止めが消える。

**worktree が未作成の場合（先頭に worktree 作成を足して計 13 タスク）:**

| subject | description |
|---------|-------------|
| Worktree 作成 | `/worktree-new` スキルで feature/fix の worktree とブランチを作成する（Issue 番号・説明から命名）。**【完了条件】**`git worktree list` に当該ブランチ行が現れた出力を提示。 |
| 実装前設計（設計ドラフト承認） | 着手したらまず **AC ウォークスルー**を行う＝対象 Issue の AC（Given/When/Then）を平文の利用シナリオに展開して照返し、解釈違い（境界値・状態の定義・暗黙の前提）が無いか確認する（planning の起票前ゲートでウォークスルー済みなら**「済みであること」を利用者に確認するだけでよい**＝二重の全展開はしない。作法は `.claude/rules/alignment.md`）。対象 Issue が**設計に影響する場合**、影響する `docs/design/<領域>.md` を **`status: draft`** で先行作成/更新する（新領域なら必須 frontmatter `title/area/status/relatedIssues/updated` 付きで新規作成。構造は `.claude/rules/design-doc.md`）。**設計判断を伴う場合は、draft を書く前に 2〜3 案＋トレードオフ表（軸: 複雑さ・拡張性・性能・工数・既存との整合）を `AskUserQuestion` で示して選択を得る**（1 案しか妥当でないと判断した場合も「なぜ代替が無いか」を添えて確認を得る。選択の理由は設計書の「主要な設計判断」節に記録する＝`.claude/rules/alignment.md` の設計オプション比較）。**UI 変更を含む Issue は、ワイヤーフレーム/レイアウトを 2 案以上比較し、採択案のワイヤーフレーム（Mermaid flowchart か ASCII）と画面遷移図を ui 領域の draft に含める**。draft を提示して **`AskUserQuestion` で設計承認**を得てから次へ進む（auto 内の人間ゲート。【自律対象外】ではない）。**設計に影響しないと判断した場合も自己判断で no-op にしない**＝その判断と根拠（変更予定範囲＝`git diff` で触れる予定のファイル・領域と、設計に影響しない理由）を `AskUserQuestion` で利用者に照返し、確認を得てから no-op とする（省略の判断も人間が行う＝`.claude/rules/alignment.md` の大原則）。draft は実装完了後の「ドキュメント更新」タスクで `status: active` に確定する。**【完了条件】**AC ウォークスルー（または planning 済みの確認）を実施した旨＋draft 設計書の差分＋承認を得た旨（設計判断を伴う場合は採択案と理由、no-op の場合は利用者の確認を得た判断根拠と変更予定範囲の証拠）を提示。 |
| テスト先行実装（Red） | **L3 Task があれば L3 を反復単位とし、L3 ごとにその L3 の AC から失敗テストを用意する**（L3 が無い小 L2 は L2 AC 全体から書く）。`superpowers:test-driven-development` スキルを必ず呼んでから `test-writer` エージェントを使う。**【完了条件】**追加したテストが**実装前に失敗する（赤）**実行出力を提示（コンパイルエラーも Red 可。L3 が複数あれば各 L3 のテストが赤であること）。 |
| 実装（Green） | テストが通る最小の実装を書く。**L3 Task があれば L3 を反復単位とし、L3 ごとに緑化して各 L3 の DoD（AC 対応テスト緑・親 AC に前進）を満たす（L3 が無い小 L2 は L2 AC を直接緑化）。全 L3 完了で L2 AC が全 `[x]` になる。** `feature-dev:feature-dev` コマンド（Skill ツール／スラッシュで起動。同名のエージェントは無いため Task の subagent_type では呼べない）を使う。**UI（画面・コンポーネント）を含む実装は、書き始める前に Skill ツールで `frontend-design:frontend-design` スキルを呼び、デザイン指針（タイポグラフィ・配色・モーション・レイアウト）を読み込んでから実装する**（「テストが通る最小」をそのまま見た目に適用して素朴な UI に落ちるのを防ぐ。目指す方向は `docs/要件定義書.md` の「UI/UX 方針」節を参照）。独立タスクが複数ある場合は `superpowers:subagent-driven-development` スキルを使う。外部ライブラリ/フレームワークの API を使う場合は記憶で書かず、`context7`（MCP: `resolve-library-id`→`query-docs`）で当該バージョンのドキュメント・コード例を確認してから実装する（廃止 API・古いシグネチャの混入を防ぐ）。テストが赤のまま通らない／後段の修正で一度緑だったテストが割れた場合は、推測で実装を変える前に `superpowers:systematic-debugging` スキルで根本原因を先に特定する（症状潰しの空回りを避ける）。**実装中に仕様の曖昧さ（複数解釈可能な語・未定義の境界値・暗黙の前提・矛盾）を検出したら、推測で埋めずに `.claude/rules/alignment.md` の曖昧さ規律に従い `AskUserQuestion` で確認してから実装する**（確認結果は Issue 本文または設計書「主要な設計判断」節に記録する）。**【完了条件（カバレッジDoD）】**Issue の AC チェックリストが**全 [x]**、各 AC に対応するテストが緑、**各テストが実装前に赤だった証拠**（実行出力）を提示、**未カバー AC 0 件**（L3 を反復単位にした場合は各 L3 を順に緑化した証跡も含む）。 |
| ビジュアル/UX 確認（Green 後） | **UI 変更を含む場合のみ実体作業**。手順: ① CLAUDE.md「プロジェクト設定」の開発サーバ（`commands.dev`）を **Bash の `run_in_background` で起動**する（**ブラウザ操作 MCP はサーバを起動できない**。`commands.dev` 未設定なら推測起動せず設定不備として扱う）→ ② `curl http://localhost:<port>` で応答が返るまで readiness を確認 → ③ `playwright`（スクショの既定）／`chrome-devtools-mcp` で対象画面のスクリーンショットを**モバイル幅とデスクトップ幅**で取得し、**`docs/screenshots/<issue>-<画面>-<ブレークポイント>-<before\|after>.png` に保存** → ④ 終了時にバックグラウンドのサーバプロセスを kill。`frontend-reviewer` エージェント（Task の `subagent_type: frontend-reviewer`）で AC のビジュアル/UX 条件・デザイン意図整合（要件定義書「UI/UX 方針」）・崩れ・状態表示（空/ローディング/エラー）・コントラスト・キーボード操作・レスポンシブを確認する。**`must:` 指摘が出たら 確認→修正→再確認（再スクショ）を本タスク内で `must:` 0 件まで反復する**（別タスクにしない＝「コードレビュー」段と同じ流儀。線形鎖にループノードを作らない）。**修正でコードを変えた場合は実装（Green）のテストが緑のままであることを保つ**（割れた場合は後続「コミット」段の build/test ゲートが「実装（Green）」へ差し戻す＝サイレントに通さない）。**対象外（no-op）にできるのは次の 2 つだけで、どちらも証拠が要る**: (1) 差分が UI を含まない＝`git diff --name-only` の出力で `frontendDir` 配下（`"."` の場合は UI ファイル）の変更が 0 件であることを提示、(2) `frontendDir` が `"none"`（UI を持たない明示）＝profile の値を提示。**`kind=web` なのに `frontendDir` が空（未確認）の場合は no-op 禁止**＝`AskUserQuestion` で「frontendDir を設定する（UI のあるディレクトリ。ルート直下なら "."）／UI を持たないので "none" にする／kind を見直す」を確認し、回答に従い `.claude/project-profile.json` と CLAUDE.md「プロジェクト設定」表を **2 層同時更新**（`.claude/` 配下の書き込み＝auto mode オフ）してから本タスクを再開する。**【完了条件】**保存したスクリーンショットのファイルパス（修正前後）＋`must:` 0 件＋視覚 AC を満たした旨（no-op の場合は上記 (1) または (2) の証拠）を提示。 |
| AC/DoD 同期 | 実装（Green）で満たした内容を本文に反映する。**(1) L2 Issue 本文**: 達成済み AC のチェックボックスを `[ ]`→`[x]` にする（`gh issue edit <n> --body`／`glab issue update <n> --description`。VCS ホストは CLAUDE.md「プロジェクト設定」の `vcsHost`）。この時点で達成済みの DoD（AC 全 `[x]`／テスト実装・緑 等）も合わせて反映してよいが、**「PR/MR に Closes # を含む」など MR 作成後にしか達成しない項目は `[ ]` のまま残す**（後続「クローズ＆ファイナライズ」の DoD 最終同期で埋める）。**(2) L3 Task 本文**: この実装が対応する子 Task（Sub-issue／WorkItem）本文の AC と達成済み DoD（「AC に対応するテストが実装され緑」「親 Issue の対象 AC に前進」）を `[x]` にする。対象の子は親 Issue の子一覧（GitHub: `gh api repos/$REPO/issues/<親番号>/sub_issues`／GitLab: GraphQL `WorkItemWidgetHierarchy.children`）から、各子本文冒頭の親参照行『`> 親: #… ／ 寄与する親AC: n番目`』を読んで実装した範囲に対応するものを選ぶ。本文はファイル経由で書き換えて更新する（GitHub Sub-issue は通常 Issue として `gh issue edit <子番号> --body-file`／GitLab WorkItem は `workItemUpdate` の description）。**L3 を反復単位にした場合、各 L3 完了時にその L3 本文を随時同期してもよい（本タスクで一括でも可）。**close ではなく本文の**実態反映**であり、強制ではないが飛ばさない。**【完了条件】**更新後の L2 Issue 本文と当該 L3 Task 本文（または gh/glab の取得出力）で、達成済み AC/DoD が `[x]` になっていることを提示。 |
| リファクタリング（Refactor） | 重複排除・命名改善。`code-simplifier:code-simplifier` エージェント（**プラグイン提供**。Task ツールの subagent_type に名前空間付きの `code-simplifier:code-simplifier` を渡す。bare `code-simplifier` はローカルに実体が無く `Agent type ... not found` になる）を使う。リネーム・抽出・シグネチャ変更を伴う場合、`serena` が有効なら先に `find_referencing_symbols` で全参照を洗い出してから一括変更し、取りこぼし（テストに無い呼び出し側の破壊）を防ぐ（serena 未導入なら `grep` で代替）。**【完了条件】**リファクタ後も全テストが緑のままの実行出力＋簡略化差分（テストは変更しない）。 |
| ドキュメント更新（設計書・変更履歴） | (1) 触れた領域の `docs/design/<領域>.md` を現状に合わせて更新する（新領域なら新ファイル＋索引 `README.md`。**必須 frontmatter `title/area/status/relatedIssues/updated` を付与・更新する**。構造は `.claude/rules/design-doc.md`）。**「実装前設計」で draft を先行作成した場合は実装の現状に合わせて確定し `status: draft`→`active` に更新する**。**マイグレーションを含む差分では data-model 領域（`erDiagram`）の更新漏れがないかを確認する**。設計変更が無ければ次の「コミット」で body に `Design: none` を明記する。(2) 利用者に見える変更があれば `CHANGELOG.md` の `[Unreleased]` に 1 行追記する。(3) **新しい業務概念を命名した場合は `docs/用語集.md` に対訳（用語／英語名／使用箇所・備考）を追記する**（同一概念に複数の英語名を生まない）。**【完了条件】**`git diff docs/design/ CHANGELOG.md` を提示し（用語集に追記した場合はその差分も）、**`cd docs-site && npm run check`（astro check）の成功**を示す（設計変更なしならその理由を 1 文、利用者向け変更なしなら CHANGELOG 追記不要の旨を明示）。**【note】**新規 worktree で設計書を初めて編集する場合は先に `cd docs-site && npm ci`（`pre-commit` が案内する）。 |
| コミット | `superpowers:verification-before-completion` スキルで `commands.build`/`commands.test`（CLAUDE.md「プロジェクト設定」）の通過を確認し、**設定されていれば `commands.lint`/`commands.integrationTest` の通過も確認**してから `/git-commit` スキルを呼ぶ（結合テストの位置付けは `.claude/rules/testing-strategy.md`）。**【完了条件】**build/test（＋設定時は lint/integrationTest）通過ログ → `git log -1 --format=%B`（subject・空行・body が揃った3部構成）と `git log -1 --oneline` のハッシュを提示。body が欠落していたら（`core.hooksPath` 未設定でフックが弾けない環境で起きやすい）`git commit --amend` で補ってから提示する。**【証拠なしに completed 禁止】**通過ログとハッシュを示す前に completed にしない。**【差し戻し】**build/test が RED なら commit せず「実装（Green）」へ `in_progress` 差し戻し・不足報告。**`feat`/`fix` は `docs/design/` の差分が含まれること（または設計変更が無いなら body に `Design: none`）も確認してから `/git-commit` を呼ぶ（`commit-msg` フックが未更新を拒否する）。設計書を触れた場合は `astro check` が通ること（`pre-commit` フックが構造不正を拒否する）も確認する。** rebase でコンフリクトした CHANGELOG・設計書索引・用語集は両方残しで解消する（`.claude/rules/git-workflow.md`「並行開発のコンフリクト規律」）。 |
| コードレビュー | まず `superpowers:requesting-code-review` スキル（Skill ツール）を呼び、続けて主レビューの `pr-review-toolkit:review-pr` コマンド（Skill ツール／スラッシュで起動。`review-pr` という名のエージェントは無いため Task の subagent_type では呼べない）を実行し、最後にローカルの `code-reviewer` エージェント（Task ツールの subagent_type で起動。`feature-dev`/`pr-review-toolkit` の同名ではなくローカル。プロジェクト固有チェックは任意追記）を使う。レビューでは**設計書整合**（変更が `docs/design/` の該当領域に正しく反映されているか。領域の取り違え・実質を伴わない更新でないか）も確認し、**`commands.coverage`（CLAUDE.md「プロジェクト設定」）が設定されていればカバレッジを計測して著しい低下を報告する**（`.claude/rules/testing-strategy.md`）。`must:`/`should:` 指摘が出たら `superpowers:receiving-code-review` スキルで受領→修正→再レビューを**本タスク内で** `must:` 0 件まで反復（別タスクにしない）。**（Workflow ツールが利用可能な場合の代替経路）**本レビューを per-task Workflow として fan-out してよい: スクリプト層で `code-reviewer`／`security-reviewer`（API・認証変更時）／`frontend-reviewer`（UI 変更時）を `agentType` で並列ステージとして撒く（または同等の専門プロンプト＋出力スキーマで定義する）。各 `must:`/`should:` 指摘を**敵対的検証**（独立サブエージェントで反証）にかけ、**同一の接頭辞付き指摘一覧（`must:/should:/nit:/question:`）＋AC↔テスト対応＋設計書整合**に合成して返す（**出力契約は順次経路と同一**＝後段 `/ship` のレビュー記録転記を壊さない）。**修正（書き換え）は親で直列に適用**し `must:` 0 件まで本タスク内で反復する（fan-out は「読む/判断する」所だけ）。**Workflow ツールが使えない場合は**上記の順次レビューにフォールバックする（必須ではない。作法は CLAUDE.md「Workflow ツールによる fan-out」）。**【完了条件】**`must:/should:/nit:/question:` 接頭辞付き指摘一覧（または「`must:` なし」）＋ AC↔テスト対応＋設計書整合の確認結果（`commands.coverage` 設定時はカバレッジ計測結果も）を提示し、`must:` 0 件（`should:` は対応 or 見送り理由を明示）。**指摘一覧（接頭辞付き）と対応結果を記録し、後続「マージ」の `/ship` で PR/MR 説明文の「レビュー記録」節に転記する**（記録が無いと転記できない＝レビュー証跡を残す）。 |
| マージ（/ship・**手動ゲート**） | **【自律対象外】不可逆なので `/goal` では実行しない**。レビュー承認後、利用者が手動で `superpowers:finishing-a-development-branch` → `/ship`（MR 説明に `Closes #<issue番号>`）を実行し、**完了後に本タスクを `completed` にする**。**【完了条件】**マージ済み PR/MR の URL を提示してから completed。 |
| クローズ＆ファイナライズ | マージ完了後、Issue/子/Milestone を明示クローズする。`vcsHost`（CLAUDE.md「プロジェクト設定」）に応じて **`/github-finalize` または `/gitlab-finalize` スキル**を実行する。これらは **DoD 最終同期（`Closes #` 等を `[x]`）→ クローズ前ゲート（本文 AC/DoD が全 `[x]` か確認、未充足なら同期 or `AskUserQuestion`）→ OPEN のものだけ冪等クローズ → Milestone は open issue 0 で `AskUserQuestion` 確認** を行う。close は可逆なので auto（【自律対象外】ではない）。**【完了条件】**L2 Issue＝closed・各 L3 子＝closed を示す gh/glab 出力（該当時は Milestone＝closed の出力）に加え、**クローズ対象の本文 AC/DoD が全 `[x]`（または未充足項目について承認を得た旨）**を提示してから completed。**Milestone を閉じた（未完了 Issue が 0）直後は、`AskUserQuestion` で「このマイルストーンが完了しました。リリースしますか？」とリリースを提案する（実行はしない＝利用者が判断して `/release-tasks` を実行する〔リリース工程一式＝総合テスト→受入チェック→`/release-notes`→release ブランチ・タグ のタスク敷設〕。Milestone を使っていなければ提案不要）。** |
| Worktree クリーンアップ（**手動**） | **【自律対象外】** マージ完了後、利用者が手動で `/worktree-cleanup` → `/clean_gone` を実行し、**完了後に本タスクを `completed` にする**。**【完了条件】**`git worktree list` から当該 worktree が消えた出力を提示してから completed。 |

**worktree が既にある場合（先頭の「Worktree 作成」を省き、計 12 タスク）:** 上表から「Worktree 作成」行を除いた 12 タスク（実装前設計 〜 Worktree クリーンアップ）を作成する。**末尾の「Worktree クリーンアップ」は 12 タスクでも残す**（worktree を誰が作ったかに関わらず、マージ後の片付けは必要なため）。

### 3. blockedBy チェーンを張る（`TaskUpdate`）

タスク作成後、`TaskUpdate` で各タスクを直前タスクに `blockedBy` で繋ぎ直列化する。

**13 タスク（worktree 作成あり｜head = Worktree 作成）:**
```
実装前設計       ← blockedBy Worktree 作成
テスト先行実装   ← blockedBy 実装前設計
実装            ← blockedBy テスト先行実装
ビジュアル/UX 確認 ← blockedBy 実装
AC/DoD 同期      ← blockedBy ビジュアル/UX 確認
リファクタリング  ← blockedBy AC/DoD 同期
ドキュメント更新  ← blockedBy リファクタリング
コミット         ← blockedBy ドキュメント更新
コードレビュー   ← blockedBy コミット
マージ          ← blockedBy コードレビュー
クローズ＆ファイナライズ ← blockedBy マージ
クリーンアップ   ← blockedBy クローズ＆ファイナライズ
```

**12 タスク（worktree 既存｜head = 実装前設計。Worktree 作成タスクが無いだけ）:**
```
テスト先行実装   ← blockedBy 実装前設計
実装            ← blockedBy テスト先行実装
ビジュアル/UX 確認 ← blockedBy 実装
AC/DoD 同期      ← blockedBy ビジュアル/UX 確認
リファクタリング  ← blockedBy AC/DoD 同期
ドキュメント更新  ← blockedBy リファクタリング
コミット         ← blockedBy ドキュメント更新
コードレビュー   ← blockedBy コミット
マージ          ← blockedBy コードレビュー
クローズ＆ファイナライズ ← blockedBy マージ
クリーンアップ   ← blockedBy クローズ＆ファイナライズ
```

> **前のタスクが `completed` になるまで次へ進まない**（`blockedBy` で直列化）。途中のステップを飛ばさないためのレール。

### 4. 着手と完了報告

- **線形チェーン照合 → 先頭タスクを `in_progress`**：まず `TaskList` を読み、`[先頭]→…→クリーンアップ` の単一線形鎖で**役割名と件数（worktree 未作成 13／既存 12）**が想定どおりかを名前で照合する。照合が通って初めて先頭タスク（worktree 未作成なら「Worktree 作成」、既存なら「実装前設計」）を `TaskUpdate` で `in_progress` にする。`/project-setup` と同じく、最初のタスクを着手状態にして初めて敷設完了とする。
- 敷設したタスク一覧（順序）と、いま着手すべき先頭タスクを提示する。
- **`/goal` を促す前に、設計フェーズ（worktree 作成〔未作成時〕と「実装前設計」）を人間と同期で進める**: これらは `/goal` の自律周回の**前段**に置き、`/goal` を促さずに進める。worktree 作成は `/worktree-new`、実装前設計は `AskUserQuestion`（AC ウォークスルー確認・設計オプション比較・**設計承認**・no-op の照返し）で人間の承認を取りながら完了させる。**設計はやり直しコストが最大のフェーズ**（実装の速さでは直らない「正しく動く間違ったもの」を生む＝`.claude/rules/alignment.md`）なので、自律モードに入った後の片手間の確認に委ねず、方向が固まるまで同期で詰める。
- **設計承認を得たら、利用者に `/goal` の実行を促す（テスト先行〜レビューまで自律）**: `blockedBy` はタスクの**順序**を固定するが、ターンの切れ目で作業が止まる（Claude が制御を返す）のは防げない。Claude Code 組み込みの **`/goal`** は条件を満たすまでターンをまたいで自律的に進み続けるので、テスト先行〜レビューまでの**可逆な作業**を止めずに進められる。**マージ（`/ship`）とクリーンアップは不可逆/手動ゲートなので自律駆動に含めない**（タスク表で【自律対象外】と明示。レビューが終わって `blockedBy` が外れても `/goal` ではこの 2 タスクを実行しない）。設計承認後に、利用者へ次の実行を促す:
  ```
  /goal テスト先行からレビューまで完了して
  ```
  各ターン後に達成判定し、**レビュー完了で自動クリア＝そこで自律実行を停止**する（**停止後は直ちに下記「レビューで停止したら」の人間ゲート案内に進む**＝サイレント終了させない）。途中で止めるなら `/goal clear`。
- **レビューで停止したら、黙って終わらず必ず人間ゲートを案内する**: レビュー結果のサマリを提示し、利用者にマージ可否の最終確認を求めたうえで、手動で **`/ship`**（マージ先は型から既定＋確認）→ 続けて **「クローズ＆ファイナライズ」**（`vcsHost` に応じて `/github-finalize` または `/gitlab-finalize` で Issue＋子＋該当時 Milestone を明示クローズ）→ **`/worktree-cleanup`** を実行するよう案内する（マージは不可逆なので人間の最終ゲート）。**手動 `/ship` 完了後に「マージ」タスクを `completed` にし、続けて「クローズ＆ファイナライズ」タスクを実行する（クローズは可逆なので auto で進めてよいが、Milestone を閉じる際はレシピ内の `AskUserQuestion` で 1 度確認する）。クローズ証拠（closed 出力）を提示して「クローズ＆ファイナライズ」を `completed` にし、最後に `/worktree-cleanup` 完了後に「クリーンアップ」タスクを `completed` にして追跡を閉じる**。

---

> worktree/ブランチの**作成だけ**が目的（タスク敷設は不要）なら `/worktree-new` を直接使う。
