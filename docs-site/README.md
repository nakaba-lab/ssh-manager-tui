# docs-site（ドキュメントサイト / 設計書スキーマ検証）

このディレクトリは **Astro + Starlight** のドキュメントサイトであり、プロジェクトの `docs/`
（要件定義書・生きた設計書など）を**公開サイトとして描画**し、同時に
**生きた設計書（`docs/design/`）の frontmatter を Zod スキーマで厳密検証**する。

- **真実源**: `src/content.config.ts`（Zod スキーマ一つ）。
  - `docs` コレクション … `../docs/**` を全描画（緩い。`title` frontmatter が無いページはファイル名で補完するため、素の Markdown を置いてもビルド／Pages は落ちない）。
  - `design` コレクション … `../docs/design/**`（索引 `README.md` 除外）を必須フィールド強制。
- 設計書の中身（`../docs`）と、それを検証・描画する機構（この `docs-site/`）は分離されている。
- Node 依存はこのディレクトリに閉じる（利用者プロジェクトのビルド系とは独立）。

## 前提

Node.js（`package.json` の `engines.node` 参照）。`docs/design/` を編集してコミットすると、
git の `pre-commit` フックがこのサイトで `astro check` を実行し、スキーマ不適合を弾く。

## コマンド

```bash
npm ci          # 依存インストール（package-lock.json から再現）
npm run dev     # ローカルプレビュー（http://localhost:4321）
npm run build   # 本番ビルド（dist/ 出力。Pages CI が使用）
npm run check   # スキーマ + 型チェック（pre-commit / CI と同じ検証）
```

## worktree での注意

Issue ごとに git worktree を作る運用では、`node_modules` は worktree にコピーされない。
`docs/design/` を初めて編集する worktree では、`pre-commit` の案内に従って
`cd docs-site && npm ci` を一度実行すること（オンデマンド導入）。

## サイドバー（目次）

目次は `astro.config.mjs` の `sidebar` で**明示**している。Starlight の自動生成サイドバーは
「物理ファイルパス」でグループを作るため、`../docs` が `..` > `docs` という無意味な 2 段
グループとして目次に出てしまう（glob の id/slug を変えても自動サイドバーは物理パスを見るため
消えない）。明示することでフラットな目次にしている。

構造（`astro.config.mjs` が `docs/` を走査して動的に組み立てる）:

- **トップレベル**: `docs/` 直下の `.md`（ホーム `index.md` は除く）をフラットに自動掲載。
  新しいトップレベル文書（例: `運用ガイド.md`・`FAQ.md`）を足しても**目次は自動更新**（config 編集不要）。
- **`設計書` グループ**: `docs/design/` 配下を `autogenerate` で自動収集。
  領域ファイル（`docs/design/<領域>.md`）を足しても**目次は自動更新**。

→ 通常運用で `sidebar` を手編集する必要はない（ゼロメンテ）。slug は Starlight 既定に合わせて
小文字化する（`FAQ.md` → `/faq/`。日本語ファイル名は不変）。

存在しない slug／ディレクトリを `sidebar` に書くと `astro build` が落ちる（出荷直後は `docs/` が
`index.md` のみ。`/project-setup` の既存プロジェクトモードでは `要件定義書.md` を作らない場合も
ある）。そのため実在を見て動的に積む。ホーム（`docs/index.md`）は左上のサイトタイトルから到達
するため目次には載せていない。

## Mermaid レンダリング（既定同梱）

設計書の図は **Mermaid コードブロック**が標準記法（`.claude/rules/design-doc.md`）。このサイトは
`astro-mermaid` を**既定で同梱**しており、Mermaid コードブロックは公開サイトで**図として描画される**
（`astro.config.mjs` の `integrations` に `mermaid()` を **Starlight より前に**組み込み済み。標準記法と
公開体験を一致させるため）。追加設定は不要。

描画が不要（コードブロック表示で十分）なら無効化する：`package.json` の `dependencies` から
`astro-mermaid`・`mermaid` を外し、`astro.config.mjs` の `import mermaid …` と `integrations` の
`mermaid()` を削除する（`npm install` で `package-lock.json` を更新してコミット）。

## Pages 公開（base パス）

`astro.config.mjs` は `SITE` / `BASE` 環境変数を読む（project pages は通常 `/<repo>/`）。
GitHub は `.github/workflows/docs-deploy.yml`、GitLab は `.gitlab/ci/pages.yml` が設定する。
両ホスト分の Pages CI を同梱済みで、どちらも既定で有効（GitHub は push で起動——リポジトリ設定の
Pages Source を「GitHub Actions」にする必要あり。GitLab はルート `.gitlab-ci.yml` が include 済みで
追加設定不要）。`/project-setup` の「docs サイト有効化」タスクは依存導入（`npm ci`）と Pages 設定の
確認を行い、docs サイトが不要なら無効化（`docs-site/`・Pages CI の削除＋ルート `.gitlab-ci.yml` の
include 行除去）を案内する。ローカル dev の既定 base は `/`。

> **可視性の注意（private リポジトリ ≠ private サイト）**: Pages サイトの公開範囲はリポジトリの
> 可視性と**独立**。GitHub のアクセス制御は Enterprise Cloud のみ（それ以外は private リポジトリでも
> URL を知る誰でも閲覧でき、Free の private では Pages 自体が有効化不可）。GitLab は
> Settings → General → Visibility の Pages access control でメンバー限定にできる。`docs/` には
> 要件定義書・運用ガイド等の内部文書が載るため、公開可否は `/project-setup` の
> 「docs サイト有効化」タスクが確認する。
