---
title: 設計書
---

# 生きた設計書（`docs/design/`）

**「いま実装がどうなっているか（最新の設計・構造）」を表す生きた文書。** 実装が変わるたびに上書き更新し、常に現状を反映する（過去の経緯ログ〔ADR〕ではない）。規律・frontmatter スキーマ・節テンプレートの真実源は [`.claude/rules/design-doc.md`](../../.claude/rules/design-doc.md)、スキーマ実体は `docs-site/src/content.config.ts`。

## 領域一覧

| 領域 | ファイル | 状態 | 最終更新 |
|------|---------|------|---------|
| アーキテクチャ（全体構成） | [architecture.md](./architecture.md) | active | 2026-07-14 |
| config（無損失往復） | [config.md](./config.md) | active | 2026-07-15 |
| os（外部連携） | [os.md](./os.md) | draft | 2026-07-14 |
| security（vault 暗号・askpass・信頼境界） | [security.md](./security.md) | active | 2026-07-15 |
| ui（TUI 画面） | [ui.md](./ui.md) | active | 2026-07-15 |
| includes（Include 展開・read-only・#52） | [includes.md](./includes.md) | active | 2026-07-15 |

> 領域を追加したら、`docs/design/<領域>.md` を必須 frontmatter 付きで作り、この表に 1 行足す（索引の更新は領域ファイルの追加・削除時のみ）。追加候補の例: `cross-cutting`（エラーハンドリング・ログ・トランザクション境界）。draft の領域は実装の現状に合わせて確定し `status: active` にする。

## 領域ファイルの雛形

各領域ファイルは先頭に必須 frontmatter を持つ（`astro check`／Pages ビルドが検証する）:

```markdown
---
title: <領域名> 設計
area: <領域キー>          # 例: config / os / security / ui
status: draft            # active | deprecated | draft
relatedIssues: []        # number[]（無ければ []）
updated: 2026-07-14      # date
# kind: architecture     # 任意（ui / api / data / architecture / operations / other）
---
```

本文の節テンプレート（`.claude/rules/design-doc.md` に準拠）:

```markdown
## 責務
## 構成要素（Mermaid classDiagram／flowchart）
## データフロー・主要シーケンス（Mermaid sequenceDiagram）
## 外部依存・インターフェース
## 主要な設計判断（現行の理由。ADR ではない）
```

> **設計に影響する Issue は実装前に `status: draft` で先行作成**し、承認を得てから実装に入る（`/dev-tasks`「実装前設計」）。実装完了後に現状へ確定し `status: active` にする。`feat`/`fix` コミットは `docs/design/` 更新を同梱すること（`commit-msg` フックが未更新を拒否。設計変更なしは body に `Design: none`）。
