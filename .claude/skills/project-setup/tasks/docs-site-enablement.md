# docs サイト有効化

> 対象フロー: 新規・既存とも ｜ 種別: 【人間ゲート】

> ドキュメントサイト（`docs-site/`）の依存導入と Pages デプロイ設定を確認する。`docs-site/`・
> 各 Pages CI は出荷物に同梱済み。ここでは「動く状態にする」だけ。`docs/`・ルート（`.claude/` 外）の
> 操作のため auto mode 制約は受けない（profile の `docsSite.enabled` を変える場合は `.claude/` 配下＝auto mode オフ）。

1. **依存導入**：`cd docs-site && npm ci`（以降ローカルで `npm run check`／`npm run dev`／`npm run build` が可能。`pre-commit` の設計書スキーマ検証もこの依存を使う）。
   - **図の描画（既定同梱）**：設計書の Mermaid は同梱の docs-site が `astro-mermaid` を**既定同梱**しており、公開サイトで**図として描画される**（追加設定不要）。描画が不要なら無効化できる（手順の正は `docs-site/README.md`「Mermaid レンダリング」・方針は `.claude/rules/design-doc.md`）。
2. **Pages デプロイ設定**（`vcsHost` に応じて）：
   - **GitHub**：`.github/workflows/docs-deploy.yml` が追加配置で効く。公開元ブランチは build ジョブの `if`（`github.ref_name == github.event.repository.default_branch`）で **git ホストの default ブランチに自動追従**する。**注意: ホスト default は profile の `defaultBranch`（develop）とは別物**で、「バージョン管理・リモート同期」タスクの `git init -b main` によりホスト default は **main** になる＝既定では Pages は **main からのみ公開**され、develop のみにマージした生きた設計書は main 到達まで出ない（`on.push.branches: [develop, main]` は両ブランチで build を起動するが、公開は if ゲートでホスト default のみ）。develop の最新設計を継続公開したいなら、下記ゲートの「公開元ブランチを変更して有効化」で `gh repo edit --default-branch develop` を実行する。リポジトリ設定 → Pages → Source を「GitHub Actions」にする旨も案内する。
   - **GitLab**：出荷のルート `.gitlab-ci.yml` が `pages`（`- local: .gitlab/ci/pages.yml`）を既に `include` 済みのため追加設定は不要。**ただし公開元は `$CI_DEFAULT_BRANCH`（git ホストの default＝出荷時 main）連動**で、develop のみにマージした設計書は main 到達まで公開されない。develop を継続公開したいなら下記ゲートの「公開元ブランチを変更して有効化」で `glab repo edit --default-branch develop` を実行する（design-doc-check と同様、無効化時のみ include 行を外す）。
3. **人間ゲート（`AskUserQuestion`・可視性確認を含む）**：
   - header: `docs サイト`
   - question:「ドキュメントサイト（Astro+Starlight）を有効化しますか？ `docs/` を Pages へ公開し、`docs/design/` の frontmatter をスキーマ検証します。Node が前提要件になります。**Pages サイトの可視性はリポジトリの可視性と独立**です — GitHub では Enterprise Cloud 以外アクセス制御不可＝private リポジトリでも URL を知る誰でも閲覧できます（GitHub Free の private では Pages 自体が有効化不可＝docs-deploy は失敗します）。GitLab は Settings → General → Visibility の Pages access control でメンバー限定にできます。docs/（要件定義書・運用ガイド・テスト証跡・スクリーンショット）を公開して問題ない内容ですか？」
   - options（**multiSelect は使わない**）:「有効化する（公開内容を確認済み）／メンバー限定で有効化（GitLab=Pages access control の設定を案内。GitHub=Enterprise Cloud のみ・不可なら無効化を案内）／公開元ブランチを変更して有効化（ホスト default を develop に＝`gh repo edit --default-branch develop`／GitLab は `glab repo edit --default-branch develop`。develop の生きた設計書を継続公開したい場合）／無効化する（`docs-site/`・Pages CI を削除し `docsSite.enabled` を false に）」
4. **無効化が選ばれた場合**：`docs-site/`・`.github/workflows/docs-deploy.yml`・`.gitlab/ci/pages.yml`・`docs/index.md` を削除し、**ルート `.gitlab-ci.yml` の `- local: .gitlab/ci/pages.yml` の include 行も外す**（snippet を消すだけだと include が「local file does not exist」で落ちる）。`.claude/project-profile.json` の `docsSite.enabled` を `false` にする（`designDoc`＝設計書「更新」規律はそのまま残す）。

`【完了条件】npm ci の成功ログ（有効化を選んだ場合は加えて `npm run check`〔astro check〕が緑のログ＝依存導入だけでなく、サイトと「ドキュメント雛形生成」タスクが作った設計書〔`docs/design/`〕の frontmatter スキーマがビルド検証を通ること）（または無効化の判断）と、Pages 設定（GitHub=Source を「GitHub Actions」にする案内。公開元 branch は default 自動追従／GitLab=ルート `.gitlab-ci.yml` 同梱済みの確認、無効化時は include 行も除去）を提示し、利用者が可視性（公開範囲）の確認を含めて有効化/無効化を AskUserQuestion で選んだことを示す。【証拠なしに completed 禁止】判断を得る前に completed にしない。`
