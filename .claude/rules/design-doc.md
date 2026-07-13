# 生きた設計書（`docs/design/`）規則

## 基本概念

**`docs/design/` は「いま実装がどうなっているか（最新の設計・構造）」を表す生きた文書**。実装が変わるたびに上書き更新し、常に現状を正しく反映する。過去の経緯ログ（ADR）ではなく、**現行の設計**を一覧できる場所。

実装前の仕様（Issue）・初期合意（要件定義書）とは別レイヤーで、役割が重複しないように分ける。

## ドキュメント体系（棲み分け）

| ドキュメント | 役割 | 残る場所 | 更新タイミング |
|------------|------|---------|--------------|
| `docs/要件定義書.md` | **WHAT/WHY**（初期合意） | repo | `/project-setup` 生成。要件が変わったら該当節を改訂し「変更履歴」節に追記（乖離・必須節の欠落は `/project-resync` が点検） |
| Issue 本文の AC（`[x]`） | 実装の達成状況 | git ホスト | 実装完了時に同期 |
| **`docs/design/`** | **HOW（最新の設計・構造）** | repo | **マージ毎（強制）** |
| `CHANGELOG.md` | 利用者向けの変更履歴 | repo | マージ毎に `[Unreleased]`、リリースで確定 |
| `CLAUDE.md` | Claude への作業指示・設定 | repo | アーキ詳細は持たず `docs/design/` を参照 |

> **アーキテクチャの詳細記述の真実源は `docs/design/`**。`CLAUDE.md`・要件定義書からは「設計の詳細は `docs/design/` を参照」と一方向に誘導し、同じ内容を二重に持たない。

## ファイル構成（領域分割）

```
docs/design/
  README.md      ← 索引（領域一覧 ＋ 各ファイルへのリンク ＋ 最終更新日）
  <領域>.md      ← 領域ごとの設計（例: architecture.md / auth.md / api.md / data-model.md / ui.md）
```

- 「領域」の境界はプロジェクトの構造に応じて決める（言語・フレームワーク非依存）。
- **全プロジェクトで `architecture.md`（システム全体構成）を必須領域として持つ**: コンポーネント関係図（Mermaid flowchart）・デプロイ構成・技術スタックと配置・非機能設計（性能・可用性・監視等）の構成への落とし込みを記す（`/project-setup` が初期生成する）。
- **Web/フロントを含むプロジェクトでは、画面・UI を扱う領域（例: `ui.md`／`screens.md`）を `api.md`・`data-model.md` と同格に立て、その領域ファイルに「UI/画面設計」節を必須で持たせる**（frontmatter に `kind: ui` を付けると CI〔`design-doc-check`〕が節の存在を検査する）。
- **auth／security 系の領域**（例: `auth.md`）には、**認可マトリクス**（ロール×操作の対応表）と**信頼境界**（外部入力がどこを通るか）を持たせる。`security-reviewer` が実装との整合をこの記述と照合する。
- 該当するプロジェクトのみ立てる領域の例: `reports.md`（帳票・出力設計）／`migration.md`（データ移行設計: 対象データ・マッピング・投入手順・リハーサル・切替計画）。
- 索引 `README.md` は領域ファイルの追加・削除時のみ更新する。

## 各領域ファイルの frontmatter（必須・スキーマ検証）

各領域ファイルは先頭に次の frontmatter を持つ。`docs-site` の Zod スキーマ（真実源 =
`docs-site/src/content.config.ts`）が必須フィールド・型・enum を検証し、不適合は `astro check`
／Pages ビルドが落とす（下記「更新タイミングと強制」）。索引 `README.md` は検証対象外。

```markdown
---
title: <領域名> 設計      # 必須（string）
area: <領域キー>          # 必須（string。例: auth / api / data-model）
status: active            # 必須（enum: active | deprecated | draft）
relatedIssues: [42, 58]   # 必須（number[]。無ければ []）
updated: 2026-06-05       # 必須（date）
kind: ui                  # 任意（enum: ui | api | data | architecture | operations | other。CI の節構造チェックが参照）
---
```

## 各領域ファイルの本文（節テンプレート）

```markdown
## 責務（このユニットは何をするか）
## 構成要素（主要コンポーネント／モジュール）
<!-- 構成要素の関係は Mermaid classDiagram／flowchart で図示する（下記「図の標準記法」） -->
## データフロー・主要シーケンス
<!-- 主要シーケンスは Mermaid sequenceDiagram で図示する（下記「図の標準記法」） -->
## データモデル（DB を持つ領域のみ・必須）
<!-- 主要エンティティ・カラム・制約・インデックス。Mermaid erDiagram 推奨。
     frontmatter に kind: data を付けると CI〔design-doc-check の data-model-section-check〕が
     この節の存在を検査する（kind:ui の「## UI/画面設計」と対称）。中身（カラム・制約の妥当性）は
     スキーマで強制しないためレビューで担保する -->
## 外部依存・インターフェース
<!-- 機械可読スキーマ（OpenAPI 等）があればそれを真実源とし、設計書からはリンクと要約のみ持つ（同じ定義を二重に持たない） -->
## 横断的関心事（任意）
<!-- エラーハンドリング方針・ログ設計・トランザクション境界・i18n/タイムゾーン。
     プロジェクト全体に共通する分は architecture.md（または専用領域 cross-cutting.md）にまとめ、領域固有の差分だけをここに書く -->
## 主要な設計判断（現行の理由 — ※ADR ではなく「今こうなっている理由」を簡潔に）
## UI/画面設計（web/フロント領域のみ・必須）
<!-- （docs/要件定義書.md の「UI/UX 方針」節があればそれを起点に）画面一覧と画面遷移／主要コンポーネント分割と責務／状態設計（初期・ローディング・空・成功・エラー・バリデーション）／
     コンポーネントカタログ（Storybook 等。非対応スタックは同等手段に読み替え、不可なら opt-out 理由を記す）への参照／
     対象画面のスクリーンショット（モバイル/デスクトップ。frontend-reviewer が docs/screenshots/ に保存した分を相対参照）／
     デザイントークン参照（色・タイポ・間隔の出所）／レスポンシブ方針（ブレークポイント）／
     アクセシビリティ（キーボード操作・コントラスト・aria/ラベル）／
     帳票・出力物があればレイアウトと項目定義（規模が大きければ reports.md に分離する）。バックエンド専用領域では省略してよい -->
```

> **「関連 Issue」「最終更新」は frontmatter（`relatedIssues`／`updated`）に移した。** 本文見出しは
> スキーマ検証の対象外（慣習として維持。Zod が強制するのは frontmatter のみ）。
> **ただし web/フロントを含む領域では「UI/画面設計」節を必須**とし、`code-reviewer`／`frontend-reviewer`
> のレビュー観点として確認する（スキーマでは強制しないため、レビューで担保する）。
>
> **図の標準記法は Mermaid コードブロック**（flowchart / sequenceDiagram / erDiagram / classDiagram /
> stateDiagram）。テキストのコードブロックは diff レビュー・エージェントによる生成/更新と相性が良く、
> 描画環境が無くても読める。使い分けの目安: データフロー・主要シーケンス＝`sequenceDiagram`、
> 構成要素の関係＝`classDiagram`／`flowchart`、データモデル＝`erDiagram`、状態遷移＝`stateDiagram`。
>
> **（任意・提案）トークン可視化**: デザイントークンを JSON 等のデータに集約していれば、設計書を `.mdx` に
> して**色チップ・余白バー・文字見本**として描画できる（**フレームワーク非依存＝データ→HTML**。実アプリ
> コンポーネントの埋め込みとは別物で、クロスプロジェクト結合やビルド脆弱性が無い）。導入手順:
> ① `cd docs-site && npm i @astrojs/mdx` し `astro.config` に `mdx()` を追加、② `docs-site/src/content.config.ts`
> の `docs`/`design` の glob を `**/*.md` → `**/*.{md,mdx}` に拡張、③ 設計書（`.mdx`）でトークン JSON を
> `import` し `.map()` で展開する。最小例:
>
> ```mdx
> import tokens from '../../../<frontendDir>/tokens.json';
>
> ## 色トークン
> <div style="display:grid;grid-template-columns:repeat(auto-fill,110px);gap:8px">
>   {Object.entries(tokens.color).map(([name, hex]) => (
>     <figure style="margin:0">
>       <div style={`background:${hex};height:44px;border-radius:6px;border:1px solid #0001`} />
>       <figcaption style="font:12px monospace">{name}<br/>{hex}</figcaption>
>     </figure>
>   ))}
> </div>
> ```
>
> **Mermaid レンダリング（既定同梱）**: 同梱の docs-site は `astro-mermaid` を**既定で同梱**しており、
> Mermaid コードブロックは公開サイトで**図として描画される**（設計書の標準記法＝Mermaid と公開体験を一致させる）。
> 追加設定は不要（`astro.config.mjs` の `integrations` に `mermaid()` を Starlight より前に組み込み済み）。
> 描画が不要（コードブロック表示で十分）なら無効化できる — 手順の正は `docs-site/README.md`「Mermaid レンダリング」。
>
> **「主要な設計判断」節は ADR（決定の時系列ログ）ではない**。過去の却下案や履歴は残さず、
> 「現行の設計がなぜこうなっているか」だけを簡潔に書く。

## 更新タイミングと強制

- **更新タイミング**: マージ毎。実装の各 Issue で、`dev-tasks` の「ドキュメント更新（設計書・変更履歴）」タスクが、触れた領域の `docs/design/<領域>.md` を現状に合わせて更新する（同一コミットに含める）。新領域は `area` を採番し frontmatter を付ける。
- **実装前設計（draft 先行）**: 設計に影響する Issue は、実装前に該当領域の `docs/design/<領域>.md` を **`status: draft`** で先行作成/更新し、設計承認（人間ゲート）を得てから実装に入る（`/dev-tasks` の「実装前設計」タスク。UI 変更を含む Issue はワイヤーフレーム〔Mermaid flowchart か ASCII〕と画面遷移図を ui 領域の draft に含める）。実装完了後の「ドキュメント更新」タスクで実装の現状に合わせて確定し、`status: active` に更新する。
- **強制（三段ガード）**: 役割の異なる 3 つで守る。
  - **更新したか（ローカル・`commit-msg`）**: `feat`/`fix` コミットで `docs/design/` 配下が未更新だと `exit 1` で拒否（`designDoc` 駆動）。
  - **構造が正しいか（ローカル・`pre-commit`）**: `docs/design/` をステージしたコミットで `docs-site` の `astro check` を実行し、frontmatter スキーマ不適合（必須欠け・enum 違反・型不正）を `exit 1` で拒否（`docsSite` 駆動）。
  - **リモート（CI）**: PR/MR で `design-doc-check`（更新規律）と Pages ビルド（スキーマ＝ビルド成功条件）が二重に守る（`--no-verify` 等のすり抜け対策）。
- **逃がし（escape）**: 設計変更を伴わない `feat`/`fix`（typo 修正・内部リファクタ等）は、コミット本文（body）または PR/MR 本文に **`Design: none`** の行を書けば `commit-msg` の更新規律を通せる（行頭の空白は許容）。ただし `docs/design/` を実際に編集した場合は、`Design: none` の有無に関わらずスキーマ検証（`pre-commit`）は走る。
  - **`git commit --amend` の注意**: 判定はステージ差分（`git diff --cached`）で行うため、設計書が既に直前コミットに入っていて amend 時に再ステージしないと「未更新」と見なされる。その場合は設計書を再ステージするか、body に `Design: none` を書く。
- **無効化**: 更新規律が不要なら `designDoc.enabled`、スキーマ検証が不要なら `docsSite.enabled` を `false` にする。
- **worktree**: `pre-commit` の `astro check` は `docs-site/node_modules` を要する。worktree ごとにコピーされないため、設計書を初めて編集する worktree では `cd docs-site && npm ci` を一度実行する（フックが案内する）。

> 設定キーの詳細は `.claude/project-profile.schema.json` の `designDoc`（`enabled`/`dir`/`enforceTypes`。`escapeKeyword` は表示用ラベルのみの予約キー＝値を変えても判定・案内・CI は `Design: none` 固定）と `docsSite`（`enabled`/`dir`/`designGlob`/`checkCommand`）を参照。

## 関連

- 実装フロー全体（AC/DoD 同期・設計書更新・CHANGELOG の位置）は `.claude/rules/spec-driven.md`。
- コミット規律（`Design: none` フッタ）は `.claude/rules/git-workflow.md`。
