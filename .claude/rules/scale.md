# スケール（大規模・モノレポ・マルチサービス）時の注意

テンプレートのコア（Issue 駆動 + Git Flow + Worktree + TDD + Spec 駆動）は規模非依存だが、出荷の既定は **単一アプリ・単一フロント・単一ビルド/テストコマンド・単一デフォルトブランチ・1 Issue = 1 worktree** を暗黙の前提に置く。大規模システム（モノレポ・マルチサービス・多数 Issue）へ適用するときの注意と運用指針をここに集約する。具体のツール・構成は固定資産として同梱せず、`/project-setup`（`claude-automation-recommender`）がプロジェクトに合わせて提案・蓄積する。

## 出荷既定の単一前提（どこがスケールしないか）

| 観点 | 出荷既定 | 大規模での注意 |
|------|---------|--------------|
| ビルド/テスト等のコマンド | `commands.*` は各 1 文字列（`.claude/project-profile.json`） | サービス別に分ける場合は 1 コマンドへ集約する（ルートの run-all スクリプト・タスクランナー）か、CI 側でサービス別ジョブに展開する。同梱 CI 雛形（`app-test`）は単一コマンドを 1 回実行する（matrix・per-service ジョブは持たない）。サービス構成は profile の `services[]`（任意）で機械可読に宣言できる（下記「profile によるマルチサービス宣言」） |
| フロントエンド | `frontendDir` は単一ディレクトリ（複数は `frontendDirs[]`・任意） | フロントが複数あるなら `frontendDirs[]`（任意配列）に各フロントのルートを列挙し、各々のビジュアル検証を個別に回す（`frontend-reviewer` は対象ディレクトリ単位。SessionStart フックは `frontendDir` を主に見る＝下記「profile によるマルチサービス宣言」） |
| デフォルトブランチ | 単一（`defaultBranch`） | サブシステムごとに別ブランチ運用にはしない（Git Flow を全体に適用する） |
| Issue ↔ worktree | 1 Issue = 1 worktree・派生元は単一 default | 巨大変更は worktree を分けず、Issue 自体を割る（下記「L2 の分割」） |
| 自動生成物保護・チェック | `checks`/`protectedGlobs` はフラット配列で全編集に一律適用 | モノレポでサブツリーごとに変えたい場合は、サブツリーに `.claude/project-profile.json` を置く（フックは `load_profile` の上方向探索でサブツリーのプロファイルを解決する。ただしネスト profile を自動生成する仕組みは無い＝手動配置） |

## profile によるマルチサービス宣言（任意）

モノレポ/マルチサービスを**機械可読に宣言**するための任意フィールド（`.claude/project-profile.json`・スキーマは `project-profile.schema.json`）。**出荷の単一キー動線（`commands.*`／`frontendDir`）は既定のまま**で、これらは構造の宣言と運用指針のための上乗せ（空配列＝単一アプリ扱い＝既定）。

- **`services[]`**: 各サービス（独立にビルド/デプロイされる単位）を `{name, dir, kind?, frontendDir?}` で列挙する。例:
  ```json
  "services": [
    { "name": "web", "dir": "services/web", "kind": "web", "frontendDir": "services/web/src" },
    { "name": "api", "dir": "services/api", "kind": "api" }
  ]
  ```
- **`frontendDirs[]`**: UI を複数ディレクトリに持つ場合に各フロントのルートを列挙する（`frontendDir` は単一フロントの主・SessionStart フックはこちらを見る）。

**宣言の使いどころ（＝宣言まで・パイプラインの自動 services 化はしない）**:
- **CI**: 同梱 `app-test` は単一コマンドを 1 回実行する。サービス別に回したいなら `services[].dir` を手がかりに app-test を **matrix 化**するか **サービス別ジョブ**に展開する（雛形の自動 services 化は持たない＝利用者が CI を構成する）。
- **ビジュアル検証**: `frontendDirs[]`／`services[].frontendDir` の各々について `frontend-reviewer` を個別に回す。
- **保護・チェックのサブツリー分割**: サブツリーごとに挙動を変えたいなら、そのサブツリーに `.claude/project-profile.json` を置く（フックは `load_profile` の上方向探索で解決。`services[]` の宣言と併用してよい）。

> `services[]` は**現状どのフック・CI も機械消費しない**（宣言と指針のための上乗せ）。パイプライン全体を services 対応にする（CI の自動反復・per-service コマンド契約等）のは、実際の大規模ニーズが固まってから別途判断する。

## L2 が大きすぎるときは分割する

1 つの L2 Issue が大きすぎると、AC が膨らみ・worktree が長命化し・レビュー単位が肥大化する。**L3 を増やすのではなく L2 を複数の L2 に割る**のが原則。目安は「1 PR/MR で無理なくレビューできる粒度」を超えたら分割。これは「L1 で機能の Given/When/Then を書きたくなったら L2 へ下ろす」冗長回避ガイド（`.claude/rules/spec-driven.md`）と同じ方向＝**上の階層に詰め込まず、同じ階層を増やす**。

## コンポーネント間・Issue 間の依存

- Issue 間の依存（A がマージされてから B に着手）は、planning（`/github-planning`・`/gitlab-planning`）の起票前ゲートで人間が確認し、L2 Issue 本文冒頭の依存メタ `> 依存: #<Issue番号>` で記録する（depends-on 方向。様式の正は `.claude/rules/spec-driven.md`「追跡性」）。これに親子構造（GitHub Sub-issue / GitLab WorkItem 階層）と直列タスクの rebase 規律（`.claude/rules/git-workflow.md`）が併さる。`/worktree-status` が依存メタと現在の Issue 状態から「並列着手可能 / 待ち」を**動的に**提示するため、複数ウィンドウで Issue を並列実装するとき「次にどの Issue へ着手できるか」を把握できる。
- **依存の真実源は本文メタ（`> 依存:`）に置き、既定ではホストネイティブの依存リンクを使わない**（全ティア・GitHub/GitLab 対称・Premium 非依存を保つため）。ホストネイティブ依存は任意の上乗せ可視化として扱う: GitHub の Issue Dependencies（blocked by / blocking）は 2025-08-21 に GA・全ティア（Free 含む）対応、GitLab の linked issues は relates_to のみ全ティアで blocks / is_blocked_by は Premium / Ultimate 専用（Epic を避けて Milestone を採ったのと同じティア制約）。Premium 環境で UI のブロック表示が欲しい場合のみ任意でネイティブにも張ってよい（既定オフ）。
- `/dev-tasks` が張る `blockedBy` は **1 Issue 内の工程直列化**であって Issue 間の依存ではない（用語が紛らわしいため Issue 間は「依存／実装順序」と呼び、`blockedBy` 語は使わない）。Issue 間の順序は上記の依存メタ＋planning＋rebase で守る（`blockedBy` に大規模な依存グラフを期待しない）。

## 大規模コードベースの探索

- ファイル全読みより `serena` の意味的ナビゲーション（`get_symbols_overview`／`find_symbol`／`find_referencing_symbols`）を優先する（CLAUDE.md「有効なプラグイン」節）。影響範囲は `find_referencing_symbols` で全参照を洗ってから一括変更する。
- 既存コードへ機能追加する Issue の Spec を書く前は `feature-dev:code-explorer` で実行経路・主要ファイルを辿る（CLAUDE.md「条件発火スキル/エージェント」節）。

## 関連

- 3 階層 Spec（L1/L2/L3）の粒度と冗長回避は `.claude/rules/spec-driven.md`。
- ブランチ戦略・直列タスクの rebase・worktree 運用は `.claude/rules/git-workflow.md`。
- テストレベル体系（結合・総合）は `.claude/rules/testing-strategy.md`。
- スタック固有の自動化（サービス別 CI・性能/負荷テスト等）の提案・蓄積は `/project-setup`・`/project-resync`。
