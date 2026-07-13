# ドキュメント雛形生成（設計書・用語集・変更履歴）

> 対象フロー: 新規・既存とも ｜ 種別: 【人間ゲート】

> 初回コミット（「バージョン管理・リモート同期」タスク）に含めるため、その前にここで生成する。`docs/`・ルート（`.claude/` 外）のため auto mode 制約は受けない。

「生きた設計書」（`docs/design/`）・「用語集」（`docs/用語集.md`）・「変更履歴」（`CHANGELOG.md`）の入れ物を、承認のうえ生成する。設計書の構造は `.claude/rules/design-doc.md` に従う。

1. **`docs/design/README.md`（索引）を生成**：領域一覧・各ファイルへのリンク・最終更新の枠と、各領域ファイルの **frontmatter（必須 `title`/`area`/`status`/`relatedIssues`/`updated`）＋本文節テンプレート（責務／構成要素／データフロー／外部依存／主要な設計判断）の案内を置く（構造は `.claude/rules/design-doc.md`、スキーマは `docs-site/src/content.config.ts`）。索引 `README.md` 自体は design スキーマ検証の対象外だが、docs サイトの公開ページにはなるため先頭に `title: 設計書` の frontmatter を付ける（欠けてもファイル名 `README` で補完される）。
2. **`docs/design/architecture.md`（システム全体構成）を必須生成**：新規＝要件定義書の主要機能・技術選定から、既存＝`/init` のアーキ要約から初期生成する。コンポーネント関係・デプロイ構成を **Mermaid flowchart の構成図**で含め、必須 frontmatter（`area: architecture`）を付ける（節構成・必須領域の定義は `.claude/rules/design-doc.md`）。全体像は領域横断の照合元になるため、他の領域候補が無くてもこのファイルだけは必ず作る。
3. **初期領域の提案**：新規＝要件定義書の主要機能から、既存＝`/init` のアーキ要約から、最初の領域候補（例: 主要モジュール単位）を提案する。**横断的関心事（`docs/design/cross-cutting.md`＝エラーハンドリング方針・ログ設計・トランザクション境界・i18n/タイムゾーン）も提案項目に含める**（採用は任意）。承認された領域があれば `docs/design/<領域>.md` を**必須 frontmatter 付き**の雛形で作る（`status: draft` 等。無理に作らず README の索引＋ architecture.md だけでも可）。
4. **`docs/用語集.md` を初期生成**：業務用語（日本語）→ 英語識別子の対訳辞書（表形式: 用語／英語名／使用箇所・備考）。新規＝要件定義書の語彙から、既存＝コードベースの主要概念から、数件でよいので初期エントリを起こす。実装中に新しい業務概念を命名したらこの表に追記する運用（同一概念に複数の英語名が生まれる「訳語のブレ」を防ぐ。ローマ字識別子禁止〔`post_edit.py` が検知〕の受け皿として英語名の正をここに置く）。docs サイト有効時は公開ページになるため先頭に `title: 用語集` の frontmatter を付ける。
5. **`CHANGELOG.md` を生成**：Keep a Changelog 形式で `## [Unreleased]` セクションのみの雛形を置く。
6. **マニュアル雛形の要否確認（`AskUserQuestion`・任意生成）**：提供形態に応じて `docs/操作マニュアル.md`（エンドユーザー向け）・`docs/運用ガイド.md`（起動/停止・バックアップ・設定変更・障害時連絡）の雛形生成の要否を確認する（エンドユーザー・運用者が居る提供形態〔web／desktop／cli〕で提案。library は通常不要。`docs/` 直下のため docs サイト有効時は Pages に自動公開される＝社外閲覧可となるため、連絡先・内部手順の公開可否もここで確認する。運用の規律は `.claude/rules/operations.md`、リリース時の更新確認は `/release-notes`・`/release-tasks` のチェックリストが担う）。
7. **人間ゲート（`AskUserQuestion`）**：生成内容を提示し「この雛形で確定してよいか／初期領域構成を修正するか」を確認する。
   - **GitLab を使う場合**：CI による設計書チェック（`design-doc-check`）は出荷のルート `.gitlab-ci.yml` が既に `include`（`- local: .gitlab/ci/design-doc-check.yml`）済みのため、追加設定なしで効く旨を案内する（無効化したい場合のみルート `.gitlab-ci.yml` の該当 include 行を外す）。GitHub も `.github/workflows/design-doc-check.yml` が追加配置で自動的に効くため案内不要。

`【完了条件】docs/design/README.md・docs/design/architecture.md・docs/用語集.md・CHANGELOG.md のパス（およびマニュアル雛形の要否確認の結果＝docs/操作マニュアル.md／docs/運用ガイド.md の生成有無）を提示し、利用者が雛形（および初期領域）を承認した。【証拠なしに completed 禁止】ファイルを実際に作成しパスを提示する前に completed にしない。`
