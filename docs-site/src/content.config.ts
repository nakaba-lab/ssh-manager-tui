import { defineCollection, z } from 'astro:content';
import { docsSchema } from '@astrojs/starlight/schema';
import { glob } from 'astro/loaders';
import type { Loader } from 'astro/loaders';

// frontmatter に title が無い Markdown のためのフォールバック。
// Starlight は公開ページごとに title を必須にする（docsSchema）。一方で利用者が
// title 無しの素の Markdown（要件定義書・メモ・README 索引など）を docs/ に置くのは
// 自然な操作で、それ 1 つで Pages ビルド全体を落とすのは過剰。そこで glob ローダを
// ラップし、検証（parseData）の前に「title が無ければファイル名（id 末尾）で補う」。
// → title 欠落で astro build / Pages が落ちない（描画コレクションは“緩い”）。
//   設計書（design）は別コレクションで title/area/status… を厳密検証するため、
//   この補完は設計書の規律を緩めない（docs=描画は緩く、design=検証は厳しく）。
function withTitleFallback(loader: Loader): Loader {
  return {
    ...loader,
    name: `${loader.name}-title-fallback`,
    load: (context) =>
      loader.load({
        ...context,
        parseData: ((props: { id: string; data: Record<string, unknown> }) => {
          const title = props.data?.title;
          if (title === undefined || title === null || title === '') {
            // id は base からの相対パス・拡張子なし（例: "要件定義書" / "guides/setup"）。
            const leaf = String(props.id).split('/').pop() || props.id;
            const data = { ...props.data, title: leaf };
            return context.parseData({ ...props, data });
          }
          return context.parseData(props);
        }) as typeof context.parseData,
      }),
  };
}

// 単一の真実源（Zod スキーマ）。
// - docs:   ../docs 配下すべてを Starlight サイトとして「描画」する（緩い）。
//           title 欠落はファイル名で補完し、設計書の追加 frontmatter は未知キーとして無視される。
// - design: ../docs/design 配下（索引 README.md は除外）を「厳密に検証」する。
//           必須フィールド欠け・enum 違反・型不正は astro check／build が非ゼロ終了で落とす。
//           このコレクションは検証専用でページは描画しない。
export const collections = {
  docs: defineCollection({
    loader: withTitleFallback(glob({ pattern: '**/*.md', base: '../docs' })),
    schema: docsSchema(),
  }),
  design: defineCollection({
    loader: glob({ pattern: ['**/*.md', '!**/README.md'], base: '../docs/design' }),
    schema: z.object({
      title: z.string(),
      area: z.string(),
      status: z.enum(['active', 'deprecated', 'draft']),
      relatedIssues: z.array(z.number()),
      updated: z.coerce.date(),
      // kind: 領域の種別（任意。optional なので既存の設計書を壊さない＝後方互換）。
      // 付けておくと CI（design-doc-check）が kind: ui のファイルに「## UI/画面設計」
      // 見出しの存在を検査できる。enum 外の値は astro check／build が落とす。
      kind: z.enum(['ui', 'api', 'data', 'architecture', 'operations', 'other']).optional(),
    }),
  }),
};
