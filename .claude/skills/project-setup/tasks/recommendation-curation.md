# 推奨キュレーション＋適用

> 対象フロー: 新規・既存とも ｜ 種別: 【人間ゲート】

> このタスクは `.claude/` 配下に書き込むため、**auto mode はオフ**にしておく（auto mode では `.claude/` 配下の書き込みがブロックされる）。

1. `Skill("claude-code-setup:claude-automation-recommender")` を実行する。**読み取り専用**スキルで、推奨を出すだけ・ファイルは変更しない。
   - **提供形態が Web（フロントを含む）の場合は、recommender に「a11y lint・ビジュアル回帰・機能 E2E（ブラウザ操作）スモーク/ランナー（`playwright` 等）・デザインシステム/コンポーネントカタログ・スクリーンショット確認フック」を提案観点に含めるよう促す**（言語・FW 非依存は維持しつつ、Web のときだけ機能 E2E と視覚品質の観点をシードする。既存プロジェクト導入では Web か否かを `/init` の判明事項〔フレームワーク・UI ファイル（コンポーネント/テンプレート/スタイル）の存在〕で判定する）。
   - **提供形態を問わず、次の観点も recommender の提案に含めるよう促す**：デプロイ自動化・環境定義／エラートラッキング・ヘルスチェック・ログ整備／性能・負荷テストランナー（要件定義書の非機能定量目標の計測手段）／依存の脆弱性監査・更新自動化（`npm audit`・`pip-audit`・Dependabot・Renovate 等）／OSS ライセンス監査／テストデータ管理（フィクスチャ・シード・個人情報のマスキング）／API を公開・消費する場合のスキーマ駆動（OpenAPI 等）と契約検証。具体ツールの選定は recommender がスタックに合わせて行う（固定ツールを前提にしない＝言語・FW 非依存の維持）。
   - **recommender が提案する hooks のうち「振る舞い/コマンド系ガード」（危険コマンド警告・禁止パターン検知・stop 前チェックリスト等）は、Python を `.claude/hooks/` に手書きする代わりに `/hookify`（markdown 1 ファイル・言語非依存・再起動不要）で軽量に起こせる**。型/コンパイル等の言語別チェックは `project-profile.json` の `checks`、生成物保護は `protectedGlobs` が担うため、それらと住み分ける（重複させない）。
2. 推奨を**種別別（hooks / subagents / skills / plugins / MCP）に件数つきで提示**する。
3. **キュレーションゲート（`AskUserQuestion`）**：
   - header: `推奨の適用`
   - question:「recommender が N 件の自動化を提案しました（hooks ◯件 / サブエージェント ◯件 / スキル ◯件 / プラグイン ◯件 / MCP ◯件）。これらは読み取り専用の提案で、まだ何も適用していません。`.claude/` への適用方針を選んでください。」
   - options:「全て承認（推奨）／一部を除外（適用しない項目を次の設問で選ぶ）／個別に確認（1 件ずつ採否）／追加要望あり（提案外に欲しい自動化を足して再提示）」
   - **「一部を除外」時のみ** 2 問目を multiSelect：まず**除外する種別**（hooks / サブエージェント / スキル＋プラグイン / MCP の 4 バケット）を選ばせ、1 種別が 5 項目以上なら**種別内項目**（先頭 4 ＋「他◯件も除外」のページ送り）をさらに multiSelect。**選択肢は recommender の実出力から動的生成**する（固定のフック/ツール名を列挙しない＝言語・FW 非依存）。
4. **承認サブセットのみ** Claude 自身が適用する：承認 hooks/permissions → `.claude/settings.json`、承認サブエージェント → `.claude/agents/<name>.md`、承認スキル → `.claude/skills/<name>/SKILL.md`、承認 MCP → `.mcp.json`（または `claude mcp add`）。除外項目は適用しない。承認スキルを `.claude/skills/<name>/SKILL.md` に作る際は、単純な定型でなければ `skill-creator:skill-creator`（Skill ツール）で起草→evals→発火精度（description）の最適化まで回す（recommender は提案のみで作成・最適化はしないため、トリガリングの取りこぼしを抑える）。
5. **profile 値（`commands`・`checks`・`vcsHost` 等）はここで書かない**（「プロファイル確定」タスクに集約。2 層をずらさないため）。適用後、何を入れ何を除外したかのサマリを提示する。

`【完了条件】推奨ごとの適用/除外判断（AskUserQuestion の回答）と、承認分のみを反映した適用/除外サマリを提示（**適用した自動化が壊れていない最小検証も示す**＝書き込んだ `settings.json`／`.mcp.json` が valid JSON でパースできる・追加スキルの frontmatter が読める等。「書いた」だけでなく「ロードされる」ことの証拠）。【証拠なしに completed 禁止】回答取得前に .claude/ へ適用しない・回答を得る前に completed にしない。`
