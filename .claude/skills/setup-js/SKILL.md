---
name: setup-js
description: JS/TS プロジェクトに ESLint・Prettier を新規セットアップするとき使用する。「ESLint を入れて」「Prettier を設定して」「JS/TS の Lint を設定して」が発火ワード。Python の Ruff 導入は /setup-python。既存設定の修正や特定の Lint エラー修正では使わない。
---

JavaScript/TypeScript プロジェクトに ESLint（Flat Config）+ Prettier を新規セットアップします。

## 手順

### 1. 対象ディレクトリを確認

`$ARGUMENTS` が指定された場合はそのディレクトリ、なければカレントディレクトリを対象にする。`package.json` の存在を確認する（なければ `npm init -y`）。

### 1.5 既存設定・フレームワークの検出（非破壊ガード）

依存を入れる前に既存の lint 設定を検出する。このスキルは更地（greenfield）への新規導入を前提に設定ファイルを生成・上書きするため、**既存設定やフレームワーク管理の ESLint がある状態で無条件に被せると壊す**（例: Next の `eslint-config-next` に `@eslint/js` を足すと eslint のメジャー不整合で `npm install` が ERESOLVE する）。次のいずれかを検出したら**無条件導入に進まず、`AskUserQuestion` で方針を確認する**（テンプレの「推測で埋めない」規律）:

- **ESLint 設定**: `eslint.config.{js,mjs,cjs}` ／ `.eslintrc.{js,cjs,json,yml}` ／ `package.json` の `eslintConfig`
- **フレームワーク管理 lint**: `dependencies`／`devDependencies` の `eslint-config-next`・`@nuxt/eslint`・`eslint-plugin-astro`・`eslint-plugin-svelte`・`@vue/eslint-config-*`、または `package.json` scripts の `next lint`
- **既存 eslint のバージョン**: `package.json` の `eslint` レンジ（`@eslint/js` は eslint と**同メジャー**を要求するため、旧メジャーにピンされていると不整合になる）
- **Prettier**: `.prettierrc*` ／ `package.json` の `prettier` キー ／ `.editorconfig`

分岐:

- **(A) フレームワーク標準 lint がある**（Next の `eslint-config-next` 等）→ 自前 flat config を新規作成せず `@eslint/js` も入れない。フレームワーク標準（`eslint-config-next/core-web-vitals` 等）を真実源とし、Prettier 無効化（`eslint-config-prettier`）と不足プラグインだけを足す方針を提示する。
- **(B) 既存の flat config／`.eslintrc*` がある**→ 上書き・自動マージはしない。既存設定の尊重を前提に**不足分の追加プラグインの提案までに留め**、設定本体の改変はユーザー承認のうえ行う。依存を足す場合も **既存 eslint と同メジャー**に合わせる（`@eslint/js` 等は不整合なら追加を中止して報告＝上記「既存 eslint のバージョン」の確認に従う）。フレームワーク管理 lint が併存するなら (A) を優先する。
- **(C) 何もない（真の greenfield）**→ 下の手順2以降をそのまま実行する。

### 2. 依存関係のインストール

> §1.5 で **(C) greenfield** と判定したときのみ、この新規導入を行う。既存 eslint があるなら `@eslint/js` は**既存 eslint と同メジャー**を選ぶ（ピン不整合なら導入を中止して報告する。例: eslint 9 系の環境に `@eslint/js@10` を入れると ERESOLVE で失敗する）。

```bash
npm install -D eslint @eslint/js typescript-eslint eslint-config-prettier prettier
```

> Vue を使う場合は `eslint-plugin-vue` も追加する。
> **フロント（React/JSX）を含む場合はアクセシビリティ Lint も入れる**: `npm install -D eslint-plugin-jsx-a11y`（React なら `eslint-plugin-react` も）。コントラスト以外の静的に検出可能な a11y 欠陥（ラベル欠落・不正な `role`・キーボード非対応）を Lint で早期に捕捉できる。

### 3. 設定ファイルを生成する

（§1.5 で **(C) greenfield** と判定したときのみ）`eslint.config.js`（Flat Config）をプロジェクトルートに作成する：

```js
import js from "@eslint/js";
import tseslint from "typescript-eslint";
import prettier from "eslint-config-prettier";

export default tseslint.config(
  js.configs.recommended,
  ...tseslint.configs.recommended,
  // フロント（JSX）を含む場合は a11y ルールを足す（要 eslint-plugin-jsx-a11y）:
  //   import jsxA11y from "eslint-plugin-jsx-a11y";
  //   jsxA11y.flatConfigs.recommended,
  prettier, // フォーマット系ルールを無効化（Prettier に委譲）
  {
    ignores: ["dist/**", "build/**", "node_modules/**", "**/*.min.js"],
  }
);
```

> **デザイン土台（任意・Web プロジェクト）**: デザインの一貫性を保つため、デザイントークン（色・余白・タイポの定義）／コンポーネントカタログ（Storybook 等）／デザインシステムの採否を検討する。具体ライブラリはスタック依存。詳細は `/project-setup` の Web 分岐で扱う。

`.prettierrc`：

```json
{
  "semi": true,
  "singleQuote": false,
  "printWidth": 100,
  "tabWidth": 2,
  "trailingComma": "es5"
}
```

`.prettierignore`：

```
dist
build
node_modules
*.min.js
```

### 4. package.json にスクリプトを追加

```json
{
  "scripts": {
    "lint": "eslint .",
    "lint:fix": "eslint . --fix",
    "format": "prettier --write .",
    "format:check": "prettier --check ."
  }
}
```

### 5. 動作確認

```bash
npm run lint
npm run format:check
```

### 6. プロファイルへ反映

CLAUDE.md「プロジェクト設定」と `.claude/project-profile.json` の `commands.lint`（`npm run lint`）・`commands.format`（`npm run format`）を更新する。
TypeScript を使う場合、`.claude/project-profile.json` の `checks` に型チェックを追加すると編集後に自動実行される：

```json
{ "match": "**/*.ts", "command": "npx tsc --noEmit", "cwdFromRoot": true, "timeout": 60 }
```

設定完了後、適用した内容と次のステップを報告する。
