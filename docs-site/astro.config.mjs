// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import mermaid from 'astro-mermaid';
import { existsSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

// Pages（GitHub/GitLab）の公開 URL・ベースパスは環境変数で注入する。
//   SITE: サイトの絶対 URL（例: https://<user>.github.io）
//   BASE: ベースパス（project pages は通常 /<repo>/。ローカル dev は '/'）
// 各ホストの CI（docs-deploy.yml / pages.yml）と /project-setup がこれらを設定する。
const site = process.env.SITE || undefined;
const base = process.env.BASE || '/';

// サイドバー（目次）を明示する理由:
// 実コンテンツは ../docs にあり src/content.config.ts の glob loader が読む。
// Starlight の自動生成サイドバーは「物理ファイルパス」でグループを作るため、
// ../docs が '..' > 'docs' という無意味な 2 段グループとして目次に出てしまう
// （glob の id/slug を変えても自動サイドバーは物理パスを見るため消えない）。
// そこで明示 sidebar でフラットな目次にする。
//
// 動的に組み立てる理由（頑健性＋ゼロメンテ）:
// - 存在しない slug/ディレクトリを sidebar に書くと astro build が落ちる
//   （出荷直後は docs/ が index.md のみ。/project-setup の既存モードでは
//   要件定義書.md を作らない場合もある）。→ docs/ に実在するものだけを積む。
// - docs/ 直下の .md（ホーム index.md は左上タイトルに集約するため除く）を
//   走査してフラットなトップレベル項目にする。新しいトップレベル文書
//   （例: 運用ガイド.md）を足してもこの config は触らなくてよい。
// - 設計書は docs/design/ を autogenerate でグループ収集する。領域ファイル
//   （docs/design/<領域>.md）を足しても自動で目次に出る。
// slug は Starlight 既定（小文字化）に合わせる。日本語ファイル名は不変。
const docsDir = fileURLToPath(new URL('../docs', import.meta.url));
const sidebar = [];
if (existsSync(docsDir)) {
  for (const name of readdirSync(docsDir).sort()) {
    if (!name.endsWith('.md') || name === 'index.md') continue;
    sidebar.push({ slug: name.slice(0, -3).toLowerCase() });
  }
}
if (existsSync(`${docsDir}/design`)) {
  // サイドバーのグループ記法は Starlight の 0.x マイナー（pre-1.0 では実質メジャー＝破壊的変更を運ぶ）に結合している:
  //   0.36–0.38: { label, autogenerate: {...} }（現行・autogenerate は label の兄弟）
  //   0.39+:     { label, items: [{ autogenerate: {...} }] }（items でラップ必須）
  // 出荷は ^0.36.0 固定なので現行記法が正（新記法は 0.36 が Invalid config で拒否する＝両対応の単一記法は無い）。
  // Starlight を 0.39+ へ bump するときはこの行を items 形へ切り替える（PR 時の docs-site-schema-check
  // ＝astro check がこの非互換を検出して赤くするので、bump PR で気づける）。
  sidebar.push({ label: '設計書', autogenerate: { directory: '../docs/design' } });
}

// https://astro.build/config
export default defineConfig({
  site,
  base,
  // 設計書など実コンテンツは ../docs にあり、src/content.config.ts の
  // glob loader が読む（このリポジトリ＝機構と、docs＝中身を分離する）。
  integrations: [
    // Mermaid コードブロックを図として描画する（astro-mermaid。Starlight より前に置くのが要件）。
    // 設計書の標準記法は Mermaid（.claude/rules/design-doc.md）。これを既定同梱して公開サイトで図を描画する。
    mermaid(),
    starlight({
      title: 'Project Docs',
      // 利用者プロジェクトで /project-setup がタイトル等を上書きする。
      // sidebar 未指定だと ../docs の物理パスが '..' > 'docs' の二重グループになるため明示する。
      sidebar,
    }),
  ],
});
