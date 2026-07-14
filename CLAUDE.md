# CLAUDE.md

このファイルは Claude Code（claude.ai/code）がこのプロジェクトで作業する際のガイダンスを提供する。**プロジェクト共通の指示はここに集約**し、コードベース固有の情報は `/project-setup` や `/init` で追記する。

---

## プロジェクト設定（このプロジェクトのプロファイル）

> `/project-setup` がこの表と `.claude/project-profile.json` を同時に埋める。手動で変える場合は両方を更新すること。
> スキル・エージェントはこの値（テストコマンド・デフォルトブランチ等）を、hook は `protectedGlobs` / `protectedBranches` / `checks` を参照して動作する。

| 項目 | 値 |
|------|----|
| プロジェクト名 | sshm-tui |
| 種別 | cli（Rust 製ターミナル UI＝TUI。ブラウザフロントは持たない） |
| 言語 | rust（edition 2024・MSRV 1.94） |
| パッケージマネージャ | cargo |
| ビルド | `cargo build --release` |
| 開発サーバ | `` （TUI のため開発サーバなし。手動起動は `cargo run`） |
| テスト | `cargo test --all` |
| Lint | `cargo clippy --all-targets -- -D warnings` |
| フォーマット | `cargo fmt --all` |
| マイグレーション | `` （なし） |
| 結合テスト | `` （単体に統合。inline `#[cfg(test)]` で外部 I/O に依存しない純粋テスト） |
| カバレッジ | `` （計測しない） |
| デプロイ | `` （サーバデプロイなし。リリースは `/release` ランブック→`v*` タグ→`release.yml`→GitHub Release／crates.io・Scoop・winget） |
| VCS ホスト | github |
| デフォルトブランチ | `develop` |
| 保護ブランチ | `develop`, `main` |
| フロントエンドディレクトリ | none（TUI＝ブラウザフロント無し。`frontend-reviewer`・DoD の UI 項目は対象外。TUI の UI 配慮は `ui/theme.rs`・`responsive_split`・キーボード操作で担保） |

> 機械可読版（hook が読む）: `.claude/project-profile.json`
>
> （大規模・任意）複数フロントは `frontendDirs[]`、モノレポ/マルチサービスは `services[]` を `.claude/project-profile.json` で機械可読に宣言できる（単一キー動線＝`frontendDir`／`commands.*` は既定のまま。使い方は `.claude/rules/scale.md`）。

---

## 標準開発ワークフロー（Issue 駆動 + Worktree + TDD + Spec 駆動）

全作業は **Issue（= 仕様書）を起点**とし、Issue ごとに独立した git worktree を作成して TDD で実装する。

| 要素 | 規則ファイル |
|------|-------------|
| Git Flow / Worktree / Conventional Commits | `.claude/rules/git-workflow.md` |
| TDD（Red-Green-Refactor） | `.claude/rules/tdd.md` |
| テストレベル体系（結合・総合・受入・性能） | `.claude/rules/testing-strategy.md` |
| Spec 駆動開発（L1/L2/L3 Issue 型） | `.claude/rules/spec-driven.md` |
| すり合わせ規律（曖昧さ規律・論点ディスカッション・理解の照返し・設計オプション比較・AC ウォークスルー・未決事項管理） | `.claude/rules/alignment.md` |
| 生きた設計書（`docs/design/`）・ドキュメント体系 | `.claude/rules/design-doc.md` |
| コードレビュー観点・接頭辞 | `.claude/rules/code-review.md` |
| 運用（監視・障害対応・バックアップ/DR・データ移行・マニュアル・OSS ライセンス） | `.claude/rules/operations.md` |
| 大規模・スケール時の注意（モノレポ/マルチサービス・多数 Issue） | `.claude/rules/scale.md` |
| 命名規則（言語別） | `.claude/rules/naming-{java,js,py}.md` |

### ドキュメント体系（実装の実態を残す）

| ドキュメント | 役割 | 更新タイミング |
|------------|------|--------------|
| `docs/要件定義書.md` | WHAT/WHY（初期合意）。「テスト方針」節＝テスト戦略の真実源（`.claude/rules/testing-strategy.md`）・「未決事項」節＝未決論点・デフォルト適用の可視化（`.claude/rules/alignment.md`） | `/project-setup`・要件が変わったら該当節を改訂し「変更履歴」節に追記（未決事項の解消手順は `.claude/rules/alignment.md`。乖離・必須節の欠落は `/project-resync` が点検） |
| Issue 本文の AC（`[x]`） | 実装の達成状況 | 実装完了時に同期（gh/glab） |
| `docs/design/` | HOW（最新の設計・構造。frontmatter 必須） | マージ毎（`commit-msg`／`pre-commit` の astro check／CI の三段で強制）。設計に影響する Issue は実装前に `status: draft` で先行更新（実装前設計） |
| `CHANGELOG.md` | 利用者向けの変更履歴 | マージ毎に `[Unreleased]`、リリースで確定（`/release-notes`） |
| `docs/用語集.md` | 業務用語（日本語）→ 英語識別子の対訳辞書（訳語のブレ防止・ローマ字禁止規律の受け皿） | `/project-setup` が初期生成・新しい業務概念を命名したら追記 |
| `docs/test/` | 総合テスト（システムテスト）・リリース後スモークの実行記録（例: `docs/test/release-vX.Y.Z-system-test.md`） | リリース毎（`/release-tasks` がタスク敷設） |
| `docs/操作マニュアル.md`・`docs/運用ガイド.md` | エンドユーザー向け操作手順／運用者向け手順（提供形態に応じて任意生成。`.claude/rules/operations.md`「マニュアル体系」） | 操作・運用に影響するリリースで更新（`/release-notes`・`/release-tasks` のチェックリストが確認） |

詳細は `.claude/rules/design-doc.md`。設計書 frontmatter のスキーマ（真実源）と公開サイトは `docs-site/`（Astro+Starlight）。`docs/` 全体は Pages へ自動デプロイされ、**公開範囲はリポジトリの可視性と独立**（private リポジトリ ≠ private サイト。注意の正は `docs-site/README.md`、公開可否の確認は `/project-setup` の「docs サイト有効化」ゲートと `.claude/rules/operations.md`）。

### 1 ステップずつの標準フロー

| ステップ | 主スキル・コマンド／エージェント | 補助（superpowers など） |
|---------|----------------------|--------------------------|
| 0. プロジェクト適合 | `/project-setup` | — |
| 1. Issue 起票（Spec 作成） | `/github-planning` または `/gitlab-planning`（起票前ゲートで**全 L2 Issue の AC ウォークスルー**＋未決事項の依存チェック＋**Issue 間依存の確認と `> 依存:` 記録**＝`.claude/rules/alignment.md`） | `superpowers:writing-plans` |
| 2. Worktree 作成 | `/worktree-new` | `superpowers:using-git-worktrees` |
| 2.5 実装前設計（設計に影響する場合・人間承認） | 設計オプション比較を経て `docs/design/<領域>.md` を `status: draft` で先行作成/更新し `AskUserQuestion` で設計承認（比較・UI 2 案以上・no-op 照返しの作法は `.claude/rules/alignment.md`、draft 先行は `.claude/rules/design-doc.md`） | `superpowers:brainstorming`（設計探索） |
| 3. テスト先行（Red） | `test-writer` | `superpowers:test-driven-development` |
| 4. 実装（Green） | `feature-dev:feature-dev` | （UI 実装時・書く前に）`frontend-design:frontend-design`・`superpowers:subagent-driven-development`・（テスト失敗時）`superpowers:systematic-debugging` |
| 4.5 ビジュアル/UX 検証（Web/UI 変更時） | `frontend-reviewer`（スクショ取得→確認→`must:` 0 まで修正を反復） | `frontend-design:frontend-design`（改善提案） |
| 5. AC/DoD 同期 | 達成 AC（L2 Issue）と実装した L3 Task 本文の AC/DoD を gh/glab で `[x]` に反映 | — |
| 6. リファクタ（Refactor） | `code-simplifier:code-simplifier` | — |
| 7. ドキュメント更新 | `docs/design/`（設計書）＋ `CHANGELOG.md` の `[Unreleased]` を更新 | `.claude/rules/design-doc.md` |
| 8. コミット | `/git-commit` | `superpowers:verification-before-completion` |
| 9. レビュー | `code-reviewer` | `pr-review-toolkit:review-pr`・`superpowers:requesting-code-review` |
| 10. マージ | `/ship` | `superpowers:finishing-a-development-branch` |
| 11. クローズ（ファイナライズ） | `/github-finalize`・`/gitlab-finalize`（DoD 最終同期→クローズ前ゲート→明示クローズ） | — |
| 12. クリーンアップ | `/worktree-cleanup` | `/clean_gone` |

> **起動方法の凡例**: `/name`・`plugin:name` のスキル/コマンドは **Skill ツール**で、エージェントは **Task ツール**（`subagent_type`）で起動する。ローカルの 4 エージェント（`test-writer`・`code-reviewer`・`security-reviewer`・`frontend-reviewer`＝`.claude/agents/` に実体）は bare 名で、**プラグイン提供のエージェント**（`code-simplifier:code-simplifier`・`feature-dev:code-explorer`）は名前空間付きで渡す（bare 名では `Agent type ... not found`・スラッシュでは起動しない）。`feature-dev:feature-dev`・`pr-review-toolkit:review-pr` は**コマンド**であり同名のエージェントは無い（Task で呼ばない）。bare `code-reviewer` はローカルエージェントを指す（プラグインの同名とは別物）。 なお（**Workflow ツールが使える場合のみ**）、ゲートの無い読み取り中心のタスクは Workflow で fan-out できる＝下記「Workflow ツールによる fan-out（可用時・任意）」。

`/project-setup`（適合フロー）と `/dev-tasks`（開発フロー ステップ 2.5〜12 ＝ **12 タスク**。worktree 未作成時のみ先頭にステップ 2 の worktree 作成 `/worktree-new` を加えて **13 タスク**）が各ステップを Task として自動生成し、`blockedBy` で直列化する（途中で飛ばさないため）。`/worktree-new` 自体は worktree/ブランチ作成のみを担う。

> **リリース工程（Milestone 完了後）**: リリース判定 → 総合テスト → 受入チェックリスト（UAT・該当時）→ リリースノート確定（`/release-notes` の実行を案内）→ release ブランチ・`main` 反映＋タグ → `develop` への戻しマージ → リリース後スモーク → マニュアル・ドキュメント最終確認、の **8 タスク**は `/release-tasks` が同様に直列敷設する（`/dev-tasks` の「クローズ＆ファイナライズ」が Milestone 完了時にリリースを提案する）。

### Worktree の命名規則

ブランチ `feature/42-add-login-page` → ディレクトリ `../feature-42-add-login-page`（スラッシュ→ハイフン・リポジトリの兄弟ディレクトリ）。派生元ブランチは「プロジェクト設定」の `defaultBranch`（既定 `develop`）。詳細は `.claude/rules/git-workflow.md`「git worktree 運用」節。

### Workflow ツールによる fan-out（可用時・任意）

ultracode 等で **Workflow ツール（fan-out オーケストレーション）が使える場合**、ゲートの無い読み取り中心のタスクを「1 タスク = 1 Workflow」で並列化して速度・精度を上げられる（**使えない場合は従来の Skill/Task 経路へ自動フォールバック＝必須にしない**。可用性はモデル/ハーネスのバージョンに依存する）。直列チェーン＋人間ゲートは全モデル共通の普遍ベースラインで、本機能はその上の任意加速器。

**設計時に押さえる能力モデル:**

- **専門エージェントを WF 内で動かせる（2 経路）**: スクリプトの各 `agent()` で、**(a) 専門特化したプロンプト＋出力スキーマ（JSON Schema）を書けば、その場で専門エージェントを定義**して走らせられる（汎用ワーカーがプロンプトで専門化し、schema で検証済みの構造化データを返す）／**(b) `agentType` に既存の登録エージェント**（ローカルの `code-reviewer`・`test-writer`・`security-reviewer`・`frontend-reviewer`、プラグインの `feature-dev:code-explorer` 等）を指定して撒く。
- **サブエージェント内からは** ネイティブの `Skill` ツールで**スキル**（`superpowers:*`・`frontend-design:frontend-design` 等）を、`ToolSearch` で**セッション接続済み MCP**（`serena` 等）を呼べる。
- **Workflow スクリプトの `agent()` ワーカーはネスト不可**（ワーカーは `Agent`/`Task` を持たない＝**多段オーケストレーションは全部スクリプト側に書く**。`workflow()` のネストも 1 段のみ）。※これは Workflow ツールの設計上の前提であり公式ドキュメントの明示記述ではない（可用性が変われば前提も揺らぐ）。
- **（通常の Task/Agent 経路は別系統・別の話）** Claude Code **v2.1.172+** では、`tools` を絞っていないサブエージェントは `Agent` ツールを継承し**最大 5 階層まで子サブエージェントを起動しうる**（Workflow ワーカーのネスト不可とは無関係）。本テンプレのローカル 4 エージェント（`code-reviewer`・`test-writer`・`security-reviewer`・`frontend-reviewer`）は frontmatter の `disallowedTools: Agent` で**単段に固定**している（子を撒かせない＝レビュー/審査は親ターンで束ねる）。ここでの「サブエージェント／`Agent` ツール」は subagent 起動を指し、`TaskCreate`/`blockedBy`（タスク敷設・直列化）とは別物。**親 `settings.json` の `deny`・hooks がサブエージェント（ネスト含む）内のツール呼び出しに効くかは版間で揺れるため**、機微操作（秘密読み・破壊操作）や書き換えを子に持たせず、人間ゲートと証拠提示は親ターンに残す（下記「守る作法」）。

**守る作法（安全骨格を崩さない）:**

- **人間ゲート（`AskUserQuestion`・`【自律対象外】`）と証拠提示は常に親ターンに残す**＝ Workflow に制御フローを所有させない（中の重労働だけ担わせる）。
- **fan-out は「読む/判断する」所だけ。書き換えは親で直列に適用**（並列 write の衝突回避）。
- **WF 経路とフォールバック経路で、そのステップの入出力契約を同一に保つ**（後段が経路に依存しない）。

**向く場面:** **コードレビュー（多観点＋敵対的検証）**が本命（下記「条件発火スキル/エージェント」表）。設計探索・コードベース調査（いずれも読み取り中心）にも効く。

### 条件発火スキル/エージェント

| 状況 | スキル/エージェント |
|------|--------------------|
| バグ・テスト失敗時 | `superpowers:systematic-debugging` |
| メモリリーク・高メモリ使用の調査時（Web/ブラウザで再現するもの） | `chrome-devtools-mcp` の `memory-leak-debugging`（ブラウザのページを操作して `take_heapsnapshot` で取得→`memlab` で解析。生 `.heapsnapshot` は直接読まない。ブラウザを介さない純 Node サーバのヒープは別途取得が要る） |
| 新機能の設計前 | `superpowers:brainstorming` |
| 既存コードへ機能追加する Issue の Spec を書く前（AC・スコープを実装の現状に接地させる） | `feature-dev:code-explorer`（実行経路・抽象を辿り読むべき主要ファイルを返す読み取り専用探索。起動は上記凡例＝Task の名前空間付き） |
| 不慣れ／更新の速い外部ライブラリの API を使う・最新ドキュメントが要るとき | `context7`（`resolve-library-id`→`query-docs` で当該バージョンのドキュメント・コード例を取得） |
| 独立タスクが 2 つ以上 | `superpowers:dispatching-parallel-agents` |
| ゲートの無いタスクを並列化して速度/精度を上げたい（**Workflow ツール可用時のみ**） | **Workflow** で per-task fan-out（本命＝コードレビュー段。上記「Workflow ツールによる fan-out（可用時・任意）」。非対応時は従来経路へフォールバック） |
| 複数ウィンドウで Issue を並列実装したい（次に着手できる Issue を知りたい） | `/worktree-status`（Issue 間依存〔`> 依存:`〕と現在の Issue 状態から「並列着手可能 / 待ち」を動的提示） |
| セキュリティ確認（API・認証変更時） | `security-reviewer` |
| UI/フロント実装・変更時 | **実装前（必須）**: `frontend-design:frontend-design`（Skill ツール）でデザイン指針（タイポグラフィ・配色・モーション・レイアウト）を読み込んでから書く ／ **実装後**: `frontend-reviewer`（ビジュアル・a11y・レスポンシブ確認。`playwright`/`chrome-devtools-mcp`。改善提案には `frontend-design:frontend-design` も使う） |
| Lint 新規導入 | `/setup-js`・`/setup-python` |
| リリース準備時（Milestone 完了・リリース作業の開始） | `/release-tasks`（リリース工程の 8 タスクを敷設＝内訳は上記「リリース工程」注記。CHANGELOG 確定・リリースノート生成だけなら `/release-notes` を直接実行） |
| セッションで判明した知見（隠れたコマンド・落とし穴・効いた手順）を CLAUDE.md に残したい時 | `/revise-claude-md`（このセッションの学びを CLAUDE.md に追記。improver の定期保守とは別＝その場の発見を取りこぼさない） |
| 完了基準が明確で自動検証（テスト/Lint）がある反復・無人タスク | `ralph-loop:ralph-loop`（**必ず `--max-iterations` を主たる安全網にする**＝無限ループ防止。`--completion-promise` は完了フレーズの exact 一致による補助的な早期終了で、単独では無限ループを止められない。停止は `/cancel-ralph`。`/dev-tasks` 内の自律はビルトインの `/goal` を使い、人間ゲート（マージ/片付け）を挟む工程では Stop フックが exit を阻むため ralph を被せない） |
| 同じ是正・指摘を繰り返している（同種のミスを再発防止したい） | `/hookify`（会話を分析し warn/block する恒久フックを生成。`remember` の受動的「記憶」に対し tool-use 時に強制する） |
| Claude Code 設定の最適化（初回） | `/project-setup`（recommender・improver を内包＝「カスタムエージェント一覧」下の注記） |
| スタックが大きく変わった後の再同期 | `/project-resync`（recommender・improver・プロファイルを現行スタックへ再同期） |
| コンテキストを clear／終了する前（次セッションへ引き継ぐ） | `/remember`（次に何をするかのハンドオフを `.remember/remember.md` に記録。継続メモリの自動保存とは別の、明示的な引き継ぎ） |

---

## カスタムスキル一覧（`.claude/skills/`）

| スキル | 発火ワード例 |
|--------|------------|
| `/project-setup` | 「セットアップして」「プロジェクトを初期化して」「このテンプレを適用して」 |
| `/project-resync` | 「再同期して」「設定を見直して」「自動化を再評価して」「スタックが変わったので設定を更新して」 |
| `/worktree-new` | 「worktree を作って」「新しいブランチを作って」 |
| `/dev-tasks` | 「Issue #42 の作業を始めて」「開発タスクを敷いて」「作業フローを立てて」「TDD の作業計画を立てて」 |
| `/worktree-status` | 「worktree の状態を確認して」「並列着手できる Issue を教えて」 |
| `/worktree-cleanup` | 「worktree を掃除して」「後片付けして」 |
| `/git-commit` | 「コミットして」「コミットメッセージを作って」 |
| `/ship` | 「main に反映して」「マージして」 |
| `/pr-description` | 「PR を作って」「PR 説明文を作って」 |
| `/review-diff` | 「差分をレビューして」「変更をチェックして」「この diff を見て」 |
| `/release-notes` | 「リリースノートを作って」「CHANGELOG を生成して」 |
| `/release-tasks` | 「リリースして」「リリース準備して」「リリース作業を始めて」「リリースタスクを敷いて」 |
| `/github-planning` | 「GitHub に起票して」「Issue を起票して」 |
| `/github-finalize` | 「GitHub の Issue をクローズして」「ファイナライズして」 |
| `/gitlab-planning` | 「GitLab に起票して」「Milestone/Issue を作成して」 |
| `/gitlab-finalize` | 「GitLab の Issue をクローズして」「ファイナライズして」 |
| `/setup-js` | 「ESLint を入れて」「Lint を設定して」 |
| `/setup-python` | 「Ruff を設定して」「Lint を入れて」 |
| `/claude-code-docs` | 「Claude Code の使い方を調べて」 |

## カスタムエージェント一覧（`.claude/agents/`）

| エージェント | 用途 |
|------------|------|
| `code-reviewer` | feature/fix の変更を汎用観点 + プロジェクト固有チェック（任意追記）でレビュー |
| `test-writer` | プロジェクトのテスト規約に従ってユニットテストを生成 |
| `security-reviewer` | API・認証まわりのセキュリティ審査（OWASP 観点） |
| `frontend-reviewer` | UI 変更のデザイン/UX/アクセシビリティ/レスポンシブを審査（スクショ確認。`frontendDir` 設定時） |

> スタック固有のエージェント・スキルは固定の実例を同梱しない。`/project-setup`（`claude-code-setup:claude-automation-recommender`・`claude-md-management:claude-md-improver`）でプロジェクトに合わせて生成・追記する。

---

## Hooks（自動実行）

`.claude/settings.json` がフックスクリプト（`.claude/hooks/*.py`）を呼ぶ（python3 が必要）。**ローマ字識別子の検知と completed 時リマインダーは profile 非依存で常時有効**。`protectedGlobs`／`checks` は設定するまで no-op。`protectedBranches` は出荷時 `develop`/`main` が入っており、それらのブランチ上での直接編集（`.claude/` 配下・`CLAUDE.md` を除く）を警告する。

| フック | タイミング | 内容 |
|--------|-----------|------|
| `pre_edit.py` | PreToolUse（Edit/MultiEdit/Write） | `protectedGlobs` 一致をブロック（自動生成物保護）／`protectedBranches` への直接編集を警告 |
| `post_edit.py` | PostToolUse（Edit/MultiEdit/Write） | ローマ字識別子の検知（常時）／`checks` 定義の言語別チェック（型チェック・コンパイル等）を実行 |
| `pre_task_update.py` | PreToolUse（TaskUpdate） | タスクを `completed` にする瞬間に「【完了条件】/DoD の証拠（コマンド出力・差分・URL 等）を会話で提示したか」のリマインダーを注入（非ブロック・常時有効。`.claude/rules/alignment.md` の大原則の機械的な歯止め） |
| `check_hooks_setup.py` | SessionStart | 設定不備の警告（非ブロック）: ① `core.hooksPath` が `.githooks` 未設定（＝git ネイティブフックが無効） ② `kind=web` なのに `frontendDir` が空（＝ビジュアル/UX 検証が全段沈黙。`"none"`＝UI なしの明示では出ない。hooksPath 設定済みでも独立に出る）。いずれも `name` 設定済みプロジェクトのみ（git 管理外・name 空では沈黙、①は `commitMessage.enabled:false` でも沈黙） |

言語別チェックを足すには `.claude/project-profile.json` の `checks` に追記する：

```json
{ "match": "**/*.ts", "command": "npx tsc --noEmit", "cwdFromRoot": true, "timeout": 60 }
```

### git ネイティブフック（`.githooks/commit-msg`・`pre-commit`）

上記は Claude Code フック（`.claude/settings.json` 経由）だが、コミット規律は **git ネイティブの 2 フック**で強制する。`git config core.hooksPath .githooks` を設定すると有効になる（`/project-setup` が初回コミット後に設定する）：

- **`commit-msg`**: Conventional Commits 形式（`<type>(<scope>): …`・scope 必須）と body 必須を検証して不適合コミットを**拒否**し、ブランチ名のチケット番号から `Refs #N` を自動補完する（`.claude/project-profile.json` の `commitMessage` で調整＝キー一覧と詳細は `.claude/rules/git-workflow.md` の「コミットメッセージ」節）。あわせて**生きた設計書チェック**も行い、`feat`/`fix` コミットで `docs/design/` 配下が未更新かつ body に `Design: none` が無ければ**拒否**する（`designDoc` で調整）。
- **`pre-commit`**: `docs/design/` を編集したコミットで `docs-site`（Astro+Starlight）の `astro check` を実行し、設計書 frontmatter のスキーマ不適合を**拒否**する（`docsSite` で調整。`commit-msg`＝「更新したか」に対する相補ガード＝「構造が正しいか」。`docs-site/node_modules` 未導入の worktree はフックが `npm ci` を案内する）。

設計書規律と設定キーの詳細は `.claude/rules/design-doc.md`。リモートでも `design-doc-check` CI と Pages ビルド（スキーマ＝ビルド成功条件）が二重ガードする（下記「同梱 CI」）。

> **`core.hooksPath` はクローンに継承されない**（git config のため）。チームで共有するプロジェクトでは各自が clone 後に `git config core.hooksPath .githooks` を再実行する。

フックと並ぶガードレールとして、`.claude/settings.json` の **permissions** も出荷時に設定済み：マージ等の不可逆操作（`gh pr merge`・`glab mr merge`・release/リポジトリ削除・secret/variable 操作・`git reset --hard`・`git clean -f`）は **`ask`（確認制）**で人間ゲート（push＝最終承認・マージ承認）を機械的に裏付ける。秘密ファイル（`.env`・`.env.*`・`**/secrets/**`・`*.pem`/`*.key`/`id_rsa*`/`id_ed25519*` の鍵類）は **`Read` ツールでの読み取りを deny** している（force push・`rm -rf` 系も deny）。なお ask は gh/glab の専用サブコマンドを捕捉する best-effort であり（`gh api` 直叩きは対象外）、Read の deny は Read ツールに効く（Bash 経由の読み取りは確認制と `security-guidance` プラグインが補完する）。`.env.example` も deny に含まれるため、設定例の確認は利用者が提示するか確認制の Bash で行う。

`.claude/settings.json`・hooks・permissions を Claude に変更させる場合は **auto mode をオフ**にすること（auto mode では `.claude/` 配下の書き込みがブロックされる）。

### 同梱 CI（GitHub Actions／GitLab CI）

ガードレールはローカルのフックだけでなくリモート（CI）でも守る。同梱 CI は次の 6 つ：

| CI | 役割 | 既定の状態 |
|----|------|-----------|
| `design-doc-check` | 設計書規律（feat/fix の PR/MR で `docs/design/` 更新を要求）。要件定義書の必須節チェック（`requirements-doc-check`・警告のみ）と `kind: ui` 設計書の「UI/画面設計」節チェック（`ui-section-check`）のジョブも同居 | 有効 |
| `secret-scan`（GitHub: `.github/workflows/secret-scan.yml`／GitLab: `.gitlab/ci/secret-scan.yml`） | 秘密情報（API キー・トークン等）のコミット混入を gitleaks で検出（スキャン範囲・誤検知の逃がし・多段ガードの正は `.claude/rules/operations.md`「秘密情報の管理」） | 有効（検出のみ fail。gitleaks 取得失敗等の環境問題は警告して成功） |
| `docs-deploy`（GitHub）／`pages`（GitLab） | `docs/` の Pages 公開＋設計書スキーマ検証（ビルド成功条件） | 有効 |
| `ci`（`.github/workflows/ci.yml`・**プロジェクト独自**） | fmt → clippy → test を ubuntu + windows マトリクスで実行 ＋ cargo audit（テンプレの `app-test.yml` は ci.yml と重複＋`test-required` が inline `#[cfg(test)]` テスト非対応のため削除済み） | 有効（`[main, develop]`） |
| `release`（`.github/workflows/release.yml`・**プロジェクト独自**） | `v*` タグ起点のリリースビルド → GitHub Release（fmt/clippy/test/audit/`cargo deny check` のゲート込み。テンプレの `release-deploy.yml` は重複のため削除済み） | 有効 |
| Dependabot（`.github/dependabot.yml`・GitHub のみ） | 依存更新の自動 PR（実効エントリは `docs-site`（npm）と同梱 CI（github-actions）。スタック分はコメント例から有効化。GitLab は Renovate／Dependency Scanning を案内） | `docs-site`・github-actions が有効 |

このプロジェクトはテンプレの `app-test.yml`・`release-deploy.yml` を削除し、既存の `ci.yml`（fmt/clippy/test/audit）・`release.yml`（`v*` タグ起点）を使う。discipline 系（`design-doc-check`・`secret-scan`・`docs-deploy`）は同梱のまま保持している。

---

## 有効なプラグイン（`.claude/settings.json`）

ワークフロー統合系（`superpowers`・`feature-dev`・`pr-review-toolkit`・`code-review`・`commit-commands`・`code-simplifier`）、コードインテリジェンス（`serena`・各種 LSP・`context7`）、ブラウザ/フロント（`playwright`・`chrome-devtools-mcp`・`frontend-design`＝**UI を書き始める前に必ず読み込む生成系スキル**。使い方の正は上記「条件発火スキル/エージェント」表）、運用補助（`ralph-loop`・`hookify`・`remember`・`skill-creator`・`claude-code-setup`・`claude-md-management`・`security-guidance`）が有効。

- **`playwright` / `chrome-devtools-mcp` の用途分担**: スクリーンショット取得・ブラウザ操作（E2E）は `playwright` を既定とし、`chrome-devtools-mcp` は性能トレース（LCP 分解の `debug-optimize-lcp`）・ヒープスナップショット等の DevTools 機能に使う（両方有効なため、どちらでも撮れるスクショで迷わないための既定）。
- **`serena`**: コーディング開始前に `initial_instructions` を呼ぶ（操作マニュアルのロード）。大規模コードベースの調査・影響範囲確認には、ファイル全読みより `get_symbols_overview`／`find_symbol`／`find_referencing_symbols` を優先する（言語サーバ駆動の意味的ナビゲーション）。
- **`context7`**: 記憶で API を書かず、不慣れ／更新の速い外部ライブラリは使う前に最新ドキュメントを取得して確認する（取得手順は上記「条件発火スキル/エージェント」表）。
- **`remember`**: フック（SessionStart/PostToolUse）で継続メモリが自動稼働する（結線不要）。明示的なハンドオフは `/remember`（使いどころは上記「条件発火スキル/エージェント」表）。既定はローカル（`.remember/` は gitignore 済み）で、チームでハンドオフを共有するなら `.gitignore` から外す。

### MCP サーバー（`.mcp.json`）

`.mcp.json` に `chrome-devtools`（ブラウザ操作・Web プロジェクト向け）を同梱している。`.claude/settings.json` の `enabledMcpjsonServers` に `chrome-devtools` を入れてあるため**既定で承認済み**（起動のたびに承認プロンプトは出ない）。MCP サーバーを追加した場合は、同じく `enabledMcpjsonServers` に名前を足すか、初回の承認プロンプトで許可する。Web を扱わないプロジェクトでは `.mcp.json` の当該エントリと `enabledMcpjsonServers` から削除してよい。

> **バージョン固定**: 同梱エントリは `chrome-devtools-mcp@1.2.0` に固定している（`@latest` に戻さない＝未審査の最新版がセッション起動のたびに無確認で実行される）。npx 直指定の MCP サーバーは Dependabot の走査対象外のため、更新は `/project-resync` の依存点検などの節目に意図的な作業として書き換える（手順の正は `.github/dependabot.yml` の注記）。

---

## コーディング規約（言語非依存の基本）

詳細は `.claude/rules/naming-{java,js,py}.md`。主要ルール（命名は hook がローマ字変数名を検知する）：

- **推測で埋めない（すり合わせ規律）**: 仕様の曖昧さを検出したら、推測で埋めずに実装前に `AskUserQuestion` で確認する（曖昧さの定義と作法は `.claude/rules/alignment.md`）。
- **ローマ字変数名禁止**: `syouhin`, `torihiki`, `kanri`, `shori` などのローマ字識別子は使わない（英語名を使う）。
- **業務用語の英語名は `docs/用語集.md` の対訳に従う**（無ければ追記してから使う）: 同一概念に複数の英語名（訳語のブレ）を生まない。
- **ハンガリアン記法禁止**: `strName`, `iCount` など。
- **Boolean のゲッター**: `isEnabled()`（`getIsEnabled()` ではない）。
- **コミットは Conventional Commits**: `<type>(<scope>): <description>`（scope 必須）＋ body 必須（WHY と影響範囲）。`commit-msg` フックが検証する（`.claude/rules/git-workflow.md`）。
- **`feat`/`fix` には対応するテストを含める**（テストなしコミット禁止）。
- **UI 実装時の最低基準**（**本プロジェクトは TUI（Web フロント無し・`frontendDir: none`）のため、以下の Web 向け最低基準〔WCAG コントラスト・Storybook・`docs/screenshots/`・`frontend-design`〕は対象外**。TUI の UI 配慮は `ui/theme.rs`〔全色の単一真実源〕・`responsive_split`〔90 桁で横並び/縦積み〕・キーボード操作・空/ローディング/エラー状態で担保する。以下はテンプレの汎用基準として保持）: レスポンシブ（主要ブレークポイントで崩れない）・キーボード操作・コントラスト（WCAG AA）・空/ローディング/エラー状態を最低限満たす。**コンポーネントは状態別カタログ（既定 Storybook。非対応スタックは同等手段 or 設計書に opt-out 理由）を持つ**（カタログ未導入のプロジェクトは `/project-resync` のデザイン土台点検で導入する）、**UI 変更時はスクリーンショットを残す**（保存先は `docs/screenshots/`）。**書き始める前の `frontend-design:frontend-design` 読み込み（上記「条件発火スキル/エージェント」表）も必須。この最低基準は下限であって目標ではない（目指す方向は `docs/要件定義書.md` の「UI/UX 方針」節を参照）。**「テスト緑＝完了」ではない（見た目は `frontend-reviewer` が確認する）。詳細な UI 規約は `/project-setup` が生成・蓄積する。

> プロジェクト固有の規約（禁止ライブラリ、フレームワーク慣習、自動生成ファイル等）は `/init` 実行後にこの下へ追記し、`.claude/rules/code-review.md` の「プロジェクト固有チェック」節と `.claude/project-profile.json` の `protectedGlobs`/`checks` に反映する。

<!-- ここから下に /init や手動で、コードベース固有の情報（アーキテクチャ・ビルド手順・ドメイン知識）を追記する -->

## プロジェクト固有情報（sshm-tui）

`sshm` は `~/.ssh/config` のホストを閲覧・編集・接続する Rust 製の TUI（[ratatui](https://ratatui.rs)）。クレート名 `sshm-tui`／バイナリ名 `sshm`（edition 2024, MSRV `rust-version = "1.94"`）。単一クレート（ワークスペースではない）。**Windows-first** だがクロスプラットフォーム。決定的な制約: **実際の OpenSSH config ファイルを読み書きする**ため、編集は無損失かつ外科的でなければならない。

### ビルド・テスト・Lint コマンド

| 目的 | コマンド |
|------|---------|
| ビルド | `cargo build`（debug）／`cargo build --release`（→ `target/release/sshm`〔Windows は `sshm.exe`〕） |
| TUI 起動 | `cargo run`（`cargo run -- --list` で非対話一覧、`cargo run -- --config <path>` で別 config＝手動テストに便利） |
| 全テスト | `cargo test --all`（260 個の inline `#[cfg(test)]` ユニットテスト。`config/`・`os/` 層。`tests/` ディレクトリは無い） |
| 単一テスト | `cargo test <部分一致名>`（例 `cargo test roundtrip_crlf`）／`cargo test config::`（config モジュール全体） |
| Lint | `cargo clippy --all-targets -- -D warnings`（**警告 = エラー**。CI が門番するのと同一） |
| フォーマット | `cargo fmt --all`（適用）／`cargo fmt --all -- --check`（検証） |
| 依存監査 | `cargo audit`（RUSTSEC）／`cargo deny check`（サプライチェーンゲート。設定 `deny.toml`） |

- **push 前に CI の 3 ゲートを合わせる**（CI は Linux **と** Windows で回す）: `cargo fmt --all -- --check`・`cargo clippy --all-targets -- -D warnings`・`cargo test --all`。CI 実体は `.github/workflows/ci.yml`（fmt → clippy → test → audit）。
- テストの**大半は純粋**（parse / arg-building のみ）だが、**一部は実 OpenSSH を spawn する**（`os::resolve` のテストが `ssh -G`・`ssh-keygen -H` を実行するため CI ランナーに OpenSSH が要る）。この ssh 起動テストは cold な Windows ランナーで稀にタイムアウトしてフレークする（再実行か hardening が要る＝下記「落とし穴」）。
- OpenSSH（`ssh`・`ssh-keygen`）が `PATH` にあること。新規タブ接続には追加で `wt.exe` が要る。
- **編集/保存の挙動を手で試すときは必ず `--config` を使い捨てファイルに向ける**（アプリは渡されたパスへそのまま書き込む）。`scratch-config` スキルがこの使い捨て config を作る。
- **リリースは利用者起動の `/release` ランブック**（`.claude/skills/release/SKILL.md`: version bump → quality gates → release build → git tag → GitHub release）。副作用があり **user-only**（無断で走らせない。配布セットアップは crates.io=`sshm-tui`／Scoop・winget は `packaging/`）。

### アーキテクチャ

#### レイヤリングと一方向依存ルール（最重要の不変条件）

3 層を厳格・強制的な依存方向で分離する。各モジュールの doc コメントが「see CLAUDE.md layering」と本節を参照している。**`config/` や `os/` から ratatui に手を伸ばさないこと**:

- **`config/`** — 無損失な `~/.ssh/config` パーサ ＋ 外科的 writer。加えて `config/diff.rs`（純粋な行レベル差分＝共通接頭辞/接尾辞トリム + 行 LCS。**保存前の差分プレビュー**を支える。レンダ済み 2 文字列だけを読み、モデルは見ない）。**ratatui 依存ゼロ**・完全ヘッドレステスト可能。最重要かつ最もテストされたモジュール。
- **`os/`** — 外界との統合すべて: `ssh`/`ssh-keygen`/`sftp` の spawn、TCP 到達性プローブ、SSH 鍵の発見 + フィンガープリントによるペアリング、`known_hosts` の解析/書き換え、クリップボード、バイナリ解決、**SFTP** 層（`os/sftp.rs`＝純粋な `ls -l` 一覧パーサ + ライブな `SftpSession` browse ワーカー〔ワーカースレッド上の短命 `sftp -b` op・armed オートフィル + circuit-breaker 付き〕。転送バッチ構築は `update.rs`）、暗号化パスワード vault（`os/vault.rs`）、および 2 つの小さな**非秘密**ファイル＝設定（`os/prefs.rs`＝`~/.ssh/sshm-prefs.json` の平 JSON・owner-private atomic write・パスワードオートフィルの opt-in を永続化・秘密は持たない）と接続履歴（`os/history.rs`＝`~/.ssh/sshm-history.json` の平 JSON・alias→最終接続時刻・上限付き fail-soft・"recent" ソートを支える）。**ratatui 依存ゼロ。**
- **`ui/`** — 純粋描画のみ。**ドメイン状態を絶対に mutate しない**（widget のスクロール/選択状態だけ）。

`config/`・`os/` は UI を一切知らない。UI と `update.rs` がそれらに依存する。

#### 単方向の状態フロー（Elm 風）

`event_loop.rs` がループを回す: **draw → 入力/tick をポーリング**（TICK 200ms）。

- ドメイン mutation はすべて **`update.rs`**（`handle_key`）と **`app.rs`** に住む。`update.rs` が全キー入力をアクティブ `Screen` でルーティングする。
- **`app.rs`** が単一の `App` 構造体（全状態）と、描画**と**入力ディスパッチの双方を駆動する `Screen` enum を持つ。
- **`ui/mod.rs`** の `draw()` が `Screen` でディスパッチし、ベース画面を描いてからモーダルオーバーレイを上に重ねる。
- 画面/モードを追加すると 3 箇所を触る: `Screen` enum（`app.rs`）・dispatch アーム（`update.rs`）・draw アーム（`ui/mod.rs`）。モーダルは `App::prev_screen` + `open_overlay`/`close_overlay` ヘルパーで実装。

#### UI 描画（テーマ + レスポンシブ）

- **`ui/theme.rs`** — 全色の単一の真実源（Tokyo Night パレット）と小さな `Style` ヘルパー（`selection()`・`border()`・`SELECT_SYMBOL`）。**色の選択は必ずここを通す**。画面モジュールで `Color` をハードコードしない。
- **`ui/widgets.rs`** — 共有部品: `panel`／`modal_block`（角丸ボーダ・フォーカス時アクセント）・`footer_hints`・`kv_line`・`section_header`・`input_line`・`responsive_split`。`responsive_split` は `WIDE_MIN_WIDTH`（90 桁）以上で 2 ペインを横並び、未満で縦積みにする。
- `draw()` は固定 3 行フレーム（breadcrumb タイトル · body · footer hints）を描き、アクティブモーダルを重ねる。フッターは 80 桁以内で描けること（`footers_fit_80_cols` テスト）。

#### 無損失 round-trip 不変条件（config 層）

**ユーザーが編集していない部分について `render(parse(s)) == s` がバイト単位で成立する。** これがコア契約で、`config/mod.rs` のテスト群が守る。

- ドキュメントは順序付き `Vec<Item>` で、各行が元のインデント・キーワードの大小・セパレータ（`" "`・`"="`・`" = "`）・引数テキストを保持する（`model.rs`）。編集はブロック全体を再レンダせず、`writer.rs` の外科的セッター（`set_single`・`set_multi`・`set_extras`）で**行粒度**でブロック本体を mutate する（未変更行はそのまま・変更値だけ書き換え・ヘッダはパターンが実際に変わったときだけ再レンダ）。
- `HostView`（`model.rs`）はフォームが使う `HostBlock` の平坦・編集可能な**射影**。`from_block` が構築し、`apply_view`（`config/mod.rs`）が外科的に書き戻す。専用フォームフィールドが無いオプションは `HostView::extras` を round-trip する（B1 回帰テストが取りこぼしを守る）。
- ssh_config のクオートルール（`tokens.rs`）: バックスラッシュは**エスケープ文字ではない**ので、素の Windows パス（`C:\Users\me\.ssh\id`）はそのまま round-trip する。リテラル `"` のエスケープは無いので、`"` を含む値は保存パスがファイル破壊を避けて拒否する。

#### config ファイルが接続の真実源

保存済みホストは素の `ssh <alias>`（`os/connect.rs`）で接続し、OpenSSH 自身が書いたばかりのファイルを読む（ProxyJump・forwards・IdentityFile 等が自動適用）。明示的な `ssh` フラグ（`-i`・`-J`・`-L`…）はアドホックな `ConnectOverrides` のときだけ発行し、保存値には決して使わない。

#### 到達性プローブ（liveness）

`os/liveness.rs` は固定ワーカースレッドプールが共有ジョブキューをドレインし `mpsc` で報告する。UI スレッドは決してブロックせず、tick ごとに `App::drain_liveness()` を 1 回呼ぶ。結果は **`App::hosts` 内のホスト index** でキーされ、ホスト追加/削除でずれる — `rebuild_hosts()` が liveness マップをクリアし呼び出し側が再プローブする。プロキシ経由のホスト（`ProxyJump`/`ProxyCommand`＝`HostView::is_proxied`）は `Skipped`（直接 TCP プローブは無意味）。

#### 鍵の発見とペアリング

`os/keys.rs` は `~/.ssh` を再帰探索（`MAX_DEPTH = 8`）し、**最初のヘッダ行だけ**を sniff して分類する（秘密鍵本体は決して読み込まない）。公開/秘密の対は各半分を独立にフィンガープリント（`ssh-keygen -l -f`）して SHA256 **公開**フィンガープリントを比較してペアリングし、`PairStatus`（`Matched`/`Mismatched`/`Unverified`/`NotApplicable`）で表す。パスフレーズ無しでは公開半を surface できない暗号化 PEM は `Unverified`（エラーではない）。

### ドメイン設計の要点（秘密・オートフィル・耐久書き込み）

- **vault（暗号化パスワードストア）**: 固定パス `~/.ssh/sshm-vault.json`（config とは完全に別ファイル・秘密は SSH config に触れさせない）。マスターパスワードを **Argon2id** で 32 byte 鍵に伸長し、エントリを **XChaCha20-Poly1305**（AEAD）で暗号化。salt/nonce/KDF パラメータは平文だが AEAD の associated data に束縛（改竄したヘッダはタグ検証で落ちる）。KDF パラメータは復号前に範囲チェック（DoS・弱体化を防ぐ）。マスターパスワードは永続化しない（誤りは AEAD タグ失敗＝"incorrect master password"）。秘密は `Secret`/`Zeroizing`（drop 時にスクラブ・`Debug` で redact）。アイドル 15 分で自動ロック（`VAULT_IDLE_LOCK`）。
- **接続時オートフィル（askpass）**: sshm 自身が `SSH_ASKPASS` ヘルパーになる。秘密を持つのは信頼された TUI 側 **listener**（接続実行中の本プロセス）で、OpenSSH が渡すプロンプトを分類し `ssh -G` 解決した identity に束縛して解放を判断する。**helper**（`SSHM_ASKPASS_CHANNEL` env で選択される別 `sshm` プロセス）はユーザースコープのチャネルで `[token][prompt]` を中継し、listener が返した 1 つの秘密を print するだけ。オートフィルの解放は System32 由来の信頼ゲート（`is_system32` を `GetSystemDirectoryW` で解決）で門番する。`src/os/askpass.rs`・`connect.rs` の変更は `security-reviewer`・`windows-first-reviewer` で確認する。
- **secure_fs**（`secure_fs.rs`）: config writer と vault の双方が使う、耐久・owner-private な書き込み基盤（不予測な O_EXCL 一時名・owner-only 権限〔unix `0o600`／Windows は継承除去した owner-only ACL〕・best-effort fsync）。どちらの層も互いに依存しないよう共通基盤に切り出してある。

### Windows-first 特記（退行させない）

- **バイナリ解決**（`os/binaries.rs`）: `PATH` の素の `ssh` より `System32\OpenSSH` を優先する（Git/MSYS の `ssh` は `~/.ssh/config`・`-J`・forwards の解釈が異なるため）。フォールバック使用時は `[PATH ssh]` 警告を出す。
- **キーイベント**: Windows コンソールは key-down と key-up の両方を出すため、`event_loop.rs` は `KeyEventKind::Press` だけに反応する（二重入力回避）。
- **原子保存**（`writer.rs`）: `secure_fs` の O_EXCL 一時ファイルへ書いてから宛先へ原子的に差し替える（delete-before-rename の窓なし・孤児残骸なし）。**Windows の上書き〔宛先が既存＝主経路〕は `ReplaceFileW`**（宛先の既存 ACL を保存するため。差し替え後に `restrict_acl` で owner-only を再付与＝#3）、**初回作成と unix は `fs::rename`**（unix でも rename は原子的）。owner-only 権限は Windows=`restrict_acl`／unix=`0o600`、親ディレクトリを fsync。保存前に一度だけ `.bak`（セッション初回のバックアップ）を作る。
- **`wt.exe` エスケープ**（`os/connect.rs` の `escape_wt_arg`）: 空白でクオート・`;` をエスケープ・埋め込み `"` を二重化・バックスラッシュはそのまま。
- **クロスプラットフォーム clippy**: CI は `cargo clippy --all-targets -- -D warnings` を Linux **と** Windows で回す。`#[cfg(windows)]` からしか参照されないシンボル（`find_wt`・`escape_wt_arg` 等）は Linux ビルドで `dead_code`/`unused_imports` を踏む（Windows のみのローカル clippy では見えない）。使用箇所に `#[cfg(windows)]`（テストも使うなら `#[cfg(any(windows, test))]`）でゲートする。

### 規約

- **エラー**: config の parse/write 層は typed な `ConfigError`（`error.rs`）、それ以外は `anyhow::Result`。`ConfigError` は `anyhow::Error` へ自動変換される。
- **ユーザー向け失敗は panic せず `Toast`**（`App::toast`）で出す。エラー Toast は sticky（次のキー入力で消える）・成功 Toast は自動失効。
- **編集フォームは `app.rs` の 3 定数を同期させる**: `FIELD_LABELS`・`form_idx` インデックス・`MULTI_FIELDS`。フィールド追加時は 3 つ（および `form_from_view`/`view_from_form`）をすべて更新する。
- **テストはインライン `#[cfg(test)]` モジュール**（`config/`・`os/` のファイル内）。回帰テストはコメントでバグ id（B1・B5 等）を付ける — リファクタ時に保存する。
- **検索実装は 2 種類**: ホスト一覧は**ファジーランク**（`nucleo-matcher`・`App::refilter`）、known_hosts 一覧は素の大小無視の**部分一致**（`App::kh_filtered`）。挙動を共有すると仮定しない。

### 落とし穴（gotchas）

- **`cargo build`/`--release` がスキップしてスタール**: 再コンパイルせず "Finished" と出て `target/{debug,release}/sshm.exe` が古いまま残ることがある。確実にビルドし直すには `cargo clean --release -p sshm-tui`。IDE の new-diagnostics も古いことがあるので cargo の出力を信頼する。
- **ratatui 0.30.x は制御文字・幅0（bidi/zero-width）グラフェムをセルグリッド前に落とす**ため、サーバ制御のファイル名による表示スプーフィングは TUI 内では非問題。
