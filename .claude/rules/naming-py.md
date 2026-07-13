---
paths:
  - "**/*.py"
---

# Python 命名規則（PEP 8）

Pythonコードを書く際は以下の命名規則に従うこと。

| 種別 | ルール | 例 |
|------|--------|-----|
| 変数・関数 | `snake_case` | `user_id`, `calculate_total()` |
| クラス | `PascalCase` | `UserProfile`, `HttpClient` |
| 定数 | `SCREAMING_SNAKE_CASE` | `MAX_RETRY_COUNT`, `API_ENDPOINT` |
| プライベート | `_snake_case` | `_internal_method`, `_cache` |
| モジュール | `snake_case` | `user_service.py`, `data_loader.py` |

## 禁止事項

- ハンガリアン記法（`strName`, `iCount`, `bFlag`）
- ローマ字変数名（`syouhin`, `torihiki`, `kanri`, `shori`）
- 過度な省略（`usr`, `cnt`, `val`, `tmp`, `flg`）

## 許容される省略語

`msg`, `info`, `config`, `params`, `env`, `auth`, `api`, `db`
