---
paths:
  - "**/*.ts"
  - "**/*.tsx"
  - "**/*.js"
  - "**/*.jsx"
  - "**/*.vue"
---

# JavaScript/TypeScript 命名規則

JS/TS/Vueコードを書く際は以下の命名規則に従うこと。

| 種別 | ルール | 例 |
|------|--------|-----|
| 変数・関数 | `camelCase` | `userId`, `fetchData()` |
| クラス・コンポーネント | `PascalCase` | `UserList`, `HttpService` |
| 定数 | `SCREAMING_SNAKE_CASE` | `MAX_RETRY_COUNT`, `API_ENDPOINT` |
| コンポーネントファイル | `PascalCase` | `UserList.vue`, `DataTable.tsx` |
| その他ファイル | `camelCase` or `kebab-case` | `userService.ts`, `api-client.ts` |

## 禁止事項

- ハンガリアン記法（`strName`, `iCount`, `bFlag`）
- ローマ字変数名（`syouhin`, `torihiki`, `kanri`, `shori`）
- 過度な省略（`usr`, `cnt`, `val`, `tmp`, `flg`）

## 許容される省略語

`btn`, `img`, `msg`, `info`, `config`, `params`, `props`, `env`, `auth`, `api`
