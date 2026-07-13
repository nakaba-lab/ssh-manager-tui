# Spec 駆動開発（Issue 型）規則

## 基本概念

**Issue = 仕様書**。実装者（人間・AI を問わず）が Issue を読めば何を作るべきか明確にわかる状態を維持する。

実装計画は **L1 Milestone → L2 Issue → L3 Task** の 3 階層で起票する（`/github-planning`・`/gitlab-planning`）。3 階層は「大 → 中 → 小」のブレイクダウンであり、全 Level は**同一スケルトンを共有し、粒度だけを変える**。

---

## 3 階層 Spec 構造

全 Level が同じ 4 節スケルトンを共有する:

```
## 背景・目的
## 概要
## 受け入れ条件（AC）
## 完了条件（DoD）
```

### 粒度（Level ごとの書き分け）

| Level | 役割 | AC の粒度 | 型 |
|-------|------|----------|----|
| **L1 Milestone** | 成果（Epic 相当） | 成果レベルの粗い受け入れ観点を数件（機能 GWT は書かない） | 型なし（単一構造） |
| **L2 Issue** | 機能・バグ | 機能レベルの Given/When/Then | feature / bug の 2 種 |
| **L3 Task** | 作業手順 | 手順レベル「Given 〜状態, When 〜実装, Then 〜のテストが緑」 | 型なし（親 Issue の型を継承） |

> **冗長回避ガイド**: L1 で機能の Given/When/Then を書きたくなったら、それは L2 Issue に下ろす。L1 は成果観点だけに留める。

### 追跡性（親参照 1 行）

各 Level 本文の**冒頭**に親参照行を 1 行置く。構造的な親子関係は GitHub Sub-issue / GitLab WorkItem 階層が保証するため、AC 採番のトレースはしない。

| Level | 親参照行 |
|-------|---------|
| L1 Milestone | 親参照はなし（最上位）。`docs/要件定義書.md` がある場合は任意で要件参照行 `> 要件: <要件定義書の主要機能名>` を置く（主要機能とのトレーサビリティ） |
| L2 Issue | `> 親: #<Milestone番号> ／ 寄与する親AC: <n>番目`（Milestone は任意）。同 Milestone 内で着手前に完了が必要な L2 Issue があれば、続けて任意の依存行 `> 依存: #<Issue番号>, #<Issue番号>` を置く |
| L3 Task | `> 親: #<Issue番号> ／ 寄与する親AC: <n>番目`（親 Issue は**必須**、寄与親AC は任意） |

> **Issue 間の依存（`> 依存:` 任意・L2 のみ）**: 同 Milestone 内で「この Issue の着手前に完了している必要がある L2 Issue」を本文冒頭メタ `> 依存: #<Issue番号>, #<Issue番号>` で表す（depends-on 方向）。これは `/dev-tasks` の `blockedBy`（**1 Issue 内の工程直列化**）とは別レイヤーで、**Issue 間の順序**を表す（`.claude/rules/scale.md`）。エッジは planning（`/github-planning`・`/gitlab-planning`）の起票前ゲートで**人間が承認したものだけ**を書く（AI 推論で勝手に張らない＝`.claude/rules/alignment.md`）。AC 採番のトレースをしない方針は維持する（依存は Issue 単位）。`/worktree-status` がこのメタと現在の Issue 状態（open/closed）から「並列着手可能 / 待ち」を動的に再計算して提示する（陳腐化を避けるため導出した順序は本文に焼き込まず、その都度算出する）。

> **見積り・進捗（軽量）**: 重い見積り管理は持たない。L1 Milestone には due date を既定で設定し（進捗の基準線）、L2 Issue には規模感（S/M/L）をラベルか本文に任意で記載する（`/github-planning`・`/gitlab-planning` が起票時に促す）。現在地は `/worktree-status`（作業中 Issue の一覧）で、進捗は Milestone の due date と open/closed 件数で把握する。

---

## L1 Milestone Spec（成果レベル・型なし）

```markdown
> 要件: <要件定義書の主要機能名>   <!-- 任意。docs/要件定義書.md がある場合、対応する主要機能を示す -->

## 背景・目的
<このマイルストーンで達成する成果。解決する課題・到達したいゴール>

## 概要
<含まれる機能群・スコープを 2〜3 文で。詳細な機能仕様は配下の L2 Issue に委ねる>

## 受け入れ条件（AC）
<!-- 成果レベルの粗い観点を数件。機能の Given/When/Then は書かず L2 へ下ろす -->
- [ ] <達成すべき成果・観点 1>
- [ ] <達成すべき成果・観点 2>

## 完了条件（DoD）
- [ ] 配下のすべての Issue がクローズしている
- [ ] すべての AC（成果観点）が満たされている
- [ ] （受託・業務システムの場合）利用者/発注者による受入確認（UAT）が完了している
- [ ] （受託・業務システムの場合）テスト仕様書（`docs/test/`）が作成されている（`.claude/rules/testing-strategy.md`）
```

---

## L2 Issue Spec（機能レベル・feature / bug の 2 種）

`feature/*` および `fix/*` ブランチに対応する Issue。本文冒頭に親参照行（Milestone は任意）: `> 親: #<Milestone番号> ／ 寄与する親AC: <n>番目`

> **未決事項への依存**: `docs/要件定義書.md` の「未決事項」節の論点に依存する Issue は、**起票前にその論点を確定する**か、確定できない場合は本文に未決依存を明記する（`/github-planning`・`/gitlab-planning` の起票前ゲート〔AC ウォークスルー〕が確認する。作法は `.claude/rules/alignment.md`）。

### feature Issue

```markdown
## 背景・目的
## 仕様（Spec）
### 概要
### 受け入れ条件（AC）
- [ ] Given ..., When ..., Then ...
### 非機能要件
<!-- パフォーマンス・セキュリティ・制約など。Web/UI を含む Issue は「ビジュアル/UX 受け入れ条件」も必須: -->
<!-- レスポンシブ（主要ブレークポイントで崩れない）／アクセシビリティ（WCAG AA コントラスト・キーボード操作・代替テキスト）／空・ローディング・エラー状態の表示／デザイントークン・既存コンポーネントの流用 -->
<!-- 加えて docs/要件定義書.md の「UI/UX 方針」節（あれば）に基づくデザイン意図の AC も含める（方向性・参照デザイン・主要画面との整合。上記フロア項目＝崩れない/状態がある だけで終わらせない） -->
<!-- Web/UI を含む Issue は、主要画面のワイヤーフレームまたは画面遷移図（Mermaid flowchart か ASCII）を Issue 本文に含めるか、docs/design/ui.md（status: draft）に置いて参照する（/dev-tasks の「実装前設計」タスクで承認を得る） -->
## 完了条件（DoD）
```

### bug Issue

```markdown
## バグの概要
## 発生環境
<!-- OS・ブラウザ・アプリのバージョン/コミット -->
## 重大度・優先度
<!-- 重大度: SEV1（全面停止）/ SEV2（主要機能影響）/ SEV3（軽微） ／ 優先度: high / medium / low（下記「トリアージ基準」で決める） -->
## 再現手順
## 期待動作 / 実際の動作
## 受け入れ条件（修正完了の定義）
- [ ] 再現手順で問題が発生しない
- [ ] 回帰テストが追加されている
- [ ] （UI 不具合の場合）修正画面のスクリーンショットで崩れ・コントラスト・状態表示の回帰がないことを確認した
## 回帰テストケース
```

> **トリアージ基準**: bug の対応順は**重大度×頻度**で決める。重大度（SEV1: 全面停止／SEV2: 主要機能影響／SEV3: 軽微）が高く発生頻度の高いものから優先度（high/medium/low）を付けて着手する。SEV1 は他作業より優先して即応し、SEV3×低頻度は通常の開発キュー（Milestone 配下）に乗せる。重大度の定義は incident（本番障害）と共通（`.claude/rules/operations.md`）。
>
> **本番障害（インシデント）は incident テンプレートで起票する**: 稼働中の本番に影響する障害は bug ではなく `incident` テンプレート（`.github/ISSUE_TEMPLATE/incident.md`／`.gitlab/issue_templates/incident.md`。ラベル/タイトル接頭辞 `incident`）で起票し、暫定対応を記録したうえで、恒久対応を hotfix（fix Issue＝本 bug 型）として起票して紐付ける。対応フロー（severity・トリアージ・ポストモーテム）の詳細は `.claude/rules/operations.md`。

---

## L3 Task Spec（手順レベル・型なし／親 Issue の型を継承）

> **L3 Task は実装の反復単位**: `/dev-tasks` の実装フローでは L3 を 1 反復として Red-Green-Refactor を回し、各 L3 完了時にその L3 の AC/DoD（AC 対応テスト緑・親 AC に前進）を満たす。L3 が無い L2 では L2 AC を直接回す。

```markdown
> 親: #<親 Issue 番号> ／ 寄与する親AC: <n>番目（任意）

## 背景・目的
<この手順タスクが親 Issue のどの部分を実装するか>

## 概要
<実装する手順を 1〜2 文で>

## 受け入れ条件（AC）
- [ ] Given <着手前の状態>, When <この手順で実装する内容>, Then <対応するテストが緑>

## 完了条件（DoD）
- [ ] AC に対応するテストが実装され、すべてパスする
- [ ] 親 Issue の対象 AC に前進している
```

---

## 受け入れ条件（AC）の書き方

粒度は Level による（L1=成果観点 / L2=機能 GWT / L3=手順 GWT）。L2 / L3 は Given/When/Then 形式のチェックリストで書く:

```
- [ ] Given <前提条件>, When <操作>, Then <期待結果>
```

**良い例:**
```
- [ ] Given 有効な API キーを持つユーザー, When 一覧 API を呼ぶ, Then 一覧（200）が返る
- [ ] Given 無効な API キーを持つユーザー, When 一覧 API を呼ぶ, Then 401 が返る
- [ ] Given 存在しない ID, When 詳細 API を呼ぶ, Then 404 が返る
```

**良い例（UI/視覚を含む場合 — ロジック AC に加えて書く）:**
```
- [ ] Given モバイル幅(375px), When 一覧画面を開く, Then 主要要素が重ならず横スクロールが出ない
- [ ] Given データが 0 件, When 一覧画面を開く, Then 空状態のプレースホルダが表示される
- [ ] Given フォーム送信が失敗, When エラーが返る, Then `role=alert` でエラーメッセージが表示される
```

**悪い例（条件が曖昧）:**
```
- [ ] API が動く
- [ ] エラーが出ない
```

---

## 完了条件（DoD）の標準セット

下記の標準セットは **L2 Issue の DoD**。L3 Task は手順タスク向けに軽量化する（上記「L3 Task Spec」参照: AC 対応テストが緑／親 Issue の対象 AC に前進）。L1 の DoD は「配下 Issue が全クローズ」等の粗い完了観点にする（上記「L1 Milestone Spec」参照）。なお L1「配下 Issue 全クローズ」「全 AC 充足」、L2「PR/MR に `Closes` 含む」は完了の**条件**であり、その実クローズ操作（Issue＋子＋該当時 Milestone）はマージ後に finalize スキル（`/github-finalize`・`/gitlab-finalize`）が実行する（`/dev-tasks` の「クローズ＆ファイナライズ」タスクが参照）。

> **チェックボックスは「同期」で埋め、「クローズ前ゲート」で確認する（重要）**: AC/DoD のチェックボックス（`[x]`）は L2 Issue だけでなく **L1 Milestone・L3 Task の本文でも**実態に同期する。state（OPEN/CLOSED）だけでクローズすると「AC/DoD が `[ ]` のまま閉じる」ため、同期は 2 段階で行う:
> 1. **実装時点の同期**（下記フロー「AC/DoD 同期」）: 実装で達成した L2 Issue の AC と、実装した L3 Task 本文の AC＋達成済み DoD を `[x]` にする。`PR/MR に Closes # を含む` のように **MR 作成後にしか達成しない項目はこの時点では `[ ]` のまま残す**。
> 2. **マージ後の最終同期**（下記フロー「DoD 最終同期」）: MR 作成後に達成された L2 Issue の DoD（`PR/MR に Closes # を含む` 等）を `[x]` にする。Milestone を閉じる場合は **L1 Milestone 本文の AC/DoD も `[x]`** にする。
>
> そのうえで finalize スキル（`/github-finalize`・`/gitlab-finalize`）は **クローズ前に本文チェックボックスが全 `[x]` かを確認するゲート**を通す（未充足なら埋めるか、`AskUserQuestion` で承認を得る）。これにより「AC/DoD が `[ ]` のまま state だけで閉じる」ことを防ぐ。

```
- [ ] Issue のすべての AC にチェックが入っている
- [ ] AC に対応するテストが実装されている
- [ ] テストがすべてパスする（テストコマンドは CLAUDE.md の「プロジェクト設定」節を参照）
- [ ] （結合テストを分離している場合）`commands.integrationTest` が緑（`.claude/rules/testing-strategy.md`）
- [ ] `code-reviewer` のレビューが通っている
- [ ] （UI 変更時）対象画面のスクリーンショットを PR/MR に添付し、レスポンシブ・コントラスト・キーボード操作・空/ローディング/エラー状態を目視確認した
- [ ] （UI 変更時）`frontend-reviewer` のレビューが通っている
- [ ] PR / MR 説明に `Closes #<issue番号>` が含まれている
```

> **（UI 変更時）の項目**は、CLAUDE.md「プロジェクト設定」の**フロントエンドディレクトリが設定済み**（`"."`＝ルート直下にフロント、を含む）で、かつ差分がその配下（画面・コンポーネント）を含むときのみ必須。`frontendDir` が `"none"`（UI を持たない明示）や UI を伴わない Issue では対象外（チェック不要）。**空（未確認）は「対象外」の根拠にならない**（kind=web なら設定不備＝SessionStart フックが警告する。frontendDir を確定してから判定する）。これにより「テスト緑＝完了」で見た目が未検証のままクローズされるのを防ぐ。

---

## 実装フロー（Spec → Code）

```
Issue 読込
  ↓
実装前設計（設計に影響する場合: 設計オプション比較〔alignment.md〕を経て docs/design/ の該当領域を status: draft で先行作成/更新し、承認を得る
          — /dev-tasks の「実装前設計」タスク。UI 変更を含む Issue はワイヤーフレーム・画面遷移図を ui 領域の draft に含める。
          設計に影響しない軽微変更も no-op の判断を照返して承認を得る〔alignment.md の大原則〕）
  ↓
AC をテストに変換（TDD: Red フェーズ）
  ↓
テストが通るまで実装（TDD: Green フェーズ）
  ↓
コードを整理（TDD: Refactor フェーズ）
  ↓
AC/DoD 同期（実装時点）: gh/glab で「L2 Issue 本文の達成 AC」と「実装した L3 Task 本文の AC＋達成済み DoD」を [x] に書き換える
                       （※「PR/MR に Closes # を含む」等 MR 作成後に達成する項目はこの時点では [ ] のまま）
  ↓
ドキュメント更新: docs/design/ を現状に更新（frontmatter スキーマ準拠。draft で先行作成した領域は status: active に確定）
              ＋ CHANGELOG [Unreleased] 追記
  ↓
コミット（feat/fix で docs/design/ 未更新なら commit-msg が拒否。設計変更なしは body に Design: none。
         設計書の frontmatter スキーマ不正は pre-commit の astro check が拒否）
  ↓
PR / MR 作成（Closes #<issue番号> を含める）
  ↓
マージ
  ↓
DoD 最終同期: MR 作成後に達成された L2 Issue DoD（「PR/MR に Closes # を含む」等）を [x] にする。
            Milestone を閉じる場合は L1 Milestone 本文の AC/DoD も [x] にする
  ↓
クローズ（ファイナライズ）: クローズ前に Issue＋子 Task＋（該当時）Milestone の本文チェックボックスが
                        全 [x] かを確認するゲートを通し（未充足は埋めるか承認）、Issue＋子＋（条件付き）Milestone を明示クローズ（finalize スキル＝/github-finalize・/gitlab-finalize）
```

**実装は L3 Task を反復単位とする**＝L3 があれば L3 ごとに（その L3 の AC を 1 件ずつ Red-Green-Refactor で回し〔`.claude/rules/tdd.md` 参照〕）緑化し、各 L3 完了時に L3 DoD（AC 対応テスト緑・親 AC に前進）を満たす。L3 が無い小 L2 は L2 AC を直接回す。すべての L3／AC が通ったら、AC/DoD 同期（L2 Issue 本文の AC＋実装した L3 Task 本文の AC/DoD を `[x]` 反映）・ドキュメント更新（`docs/design/` ＝ `.claude/rules/design-doc.md`、`CHANGELOG.md`）を経て PR/MR を作る。

マージ後は `Closes #<issue番号>` の自動クローズ（**デフォルトブランチへのマージ時のみ**発火・L2 Issue 1 件のみ）に頼らず、finalize スキル（`/github-finalize`・`/gitlab-finalize`）で Issue＋子＋（該当時）Milestone を明示クローズする。

---

## VCS との連携

| 操作 | 方法 |
|------|------|
| Issue 起票 | `/github-planning`（GitHub）または `/gitlab-planning`（GitLab）スキル |
| テスト起票 | `test-writer` エージェントに AC を渡す |
| PR/MR 作成 | `/ship` スキル（`Closes #<番号>` を説明文に含める） |
| Issue クローズ | マージ後に finalize スキル（`/github-finalize`・`/gitlab-finalize`）で明示クローズ（`Closes #<番号>` は補完＝**デフォルトブランチへのマージ時のみ**自動発火し L2 Issue 1 件のみ） |
| 子（Sub-issue / Task）クローズ | finalize スキルで親 L2 の OPEN な子をカスケードクローズ（自動クローズしないため明示が必要） |
| Milestone クローズ | finalize スキルで open issue 数が 0 になったとき AskUserQuestion で確認のうえクローズ（両プラットフォームとも自動クローズしない） |

> **クローズ前ゲート**: 上記いずれのクローズも、finalize スキルが **クローズ対象の本文チェックボックス（AC/DoD）が全 `[x]` か**を先に確認する。未充足が残る場合は、その時点で達成済みなのに未チェックの項目を `[x]` に同期し、なお埋まらない項目があれば `AskUserQuestion` で「未充足のままクローズするか」を確認してから閉じる（state だけで無条件には閉じない）。

利用するホストは CLAUDE.md の「プロジェクト設定」節（`vcsHost`）に従う。

---

## Issue テンプレート

VCS ホストに応じて以下を用意している:

| ホスト | ディレクトリ | ファイル |
|--------|-------------|---------|
| GitHub | `.github/ISSUE_TEMPLATE/` | `feature.md` / `bug.md` / `incident.md` / `config.yml` |
| GitLab | `.gitlab/issue_templates/` | `feature.md` / `bug.md` / `incident.md` |

UI で Issue 作成時にテンプレートを選択するか、planning スキルが構造を自動的に埋める。

> **正は本ファイル**: 構造定義の真実源はこの `spec-driven.md`。UI テンプレファイルのうち **feature / bug は L2（L2 Issue Spec）の写し**である。乖離した場合は本ファイルを正とする。**incident（本番障害）は L2 Spec 型ではなく**運用の起票テンプレート（対応フローの正は `.claude/rules/operations.md`。恒久対応は fix Issue〔hotfix〕として L2 で起票する）。
> **L1 / L3 は UI テンプレを持たない**（Milestone・Sub-issue/Task には UI テンプレ機構が無い）。planning スキルが本ファイルの「L1 Milestone Spec」「L3 Task Spec」構造で説明文を埋める。
