---
paths:
  - "**/*.java"
---

# Java 命名規則

Javaコードを書く際は以下の命名規則に従うこと。

## 基本規則

| 種別 | ルール | 例 |
|------|--------|-----|
| クラス | `PascalCase` | `UserProfile`, `HttpClient` |
| インターフェース | `PascalCase` | `Serializable`, `UserRepository` |
| メソッド・変数 | `camelCase` | `userId`, `calculateTotal()` |
| 定数（`static final`） | `SCREAMING_SNAKE_CASE` | `MAX_RETRY_COUNT`, `API_ENDPOINT` |
| パッケージ | すべて小文字 | `com.example.service`, `com.example.util` |
| enum型 | `PascalCase` | `OrderStatus`, `UserRole` |
| enum定数 | `SCREAMING_SNAKE_CASE` | `OrderStatus.PENDING`, `UserRole.ADMIN` |
| 型パラメータ | 大文字1文字 | `T`, `E`, `K`, `V`, `N`, `R` |
| アノテーション | `PascalCase` | `@Override`, `@NotNull` |

## クラス種別ごとのサフィックス慣習

| 種別 | サフィックス | 例 |
|------|------------|-----|
| 例外クラス | `Exception` | `UserNotFoundException`, `ValidationException` |
| 抽象クラス | `Abstract` プレフィックス | `AbstractService`, `AbstractRepository` |
| テストクラス | `Test` | `UserServiceTest`, `OrderRepositoryTest` |
| ファクトリ | `Factory` | `UserFactory`, `ConnectionFactory` |
| ユーティリティ | `Utils` or `Helper` | `StringUtils`, `DateHelper` |
| DTO | `Dto` or `Request`/`Response` | `UserDto`, `CreateUserRequest` |

## boolean変数・メソッド

`is` / `has` / `can` プレフィックスを使用すること。

```java
boolean isEnabled;
boolean hasPermission;
boolean canExecute;

// JavaBeans getter
public boolean isEnabled() { ... }
```

## インターフェースの命名指針

- 能力・性質を表す場合: `-able` / `-ible` 形容詞  
  例: `Serializable`, `Comparable`, `Runnable`
- 役割・契約を表す場合: 名詞  
  例: `UserRepository`, `PaymentService`
- `I` プレフィックス（`IUserService`）は使わない

## 禁止事項

- ハンガリアン記法（`strName`, `iCount`, `bFlag`）
- ローマ字変数名（`syouhin`, `torihiki`, `kanri`, `shori`）
- 過度な省略（`usr`, `cnt`, `val`, `tmp`, `flg`）
- インターフェースへの `I` プレフィックス（`IUserService`）

## 許容される省略語

`msg`, `info`, `config`, `params`, `env`, `auth`, `api`, `db`, `dto`, `dao`, `vo`, `id`, `ctx`
