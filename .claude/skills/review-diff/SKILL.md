---
name: review-diff
description: テンプレート同梱の軽量な差分レビュー。`.claude/rules/code-review.md` の観点・接頭辞に従って変更差分をレビューするとき使用する。「差分をレビューして」「変更をチェックして」「この diff を見て」が発火ワード。コードの説明依頼（「このコードを解説して」）では使わない。
---

`.claude/rules/code-review.md` のレビュー標準（観点・コメント接頭辞）に従って、変更差分を軽量にレビューします。

> **使い分け**: 本スキルはテンプレート同梱の軽量レビュー（rules 準拠・追加プラグイン不要）。より網羅的なレビューが欲しい場合は `pr-review-toolkit:review-pr` または `code-review:code-review` コマンド（Skill ツール／スラッシュで起動）、プロジェクト固有チェックを伴うレビューは `code-reviewer` エージェント（Task ツールで起動）を使う。（**Workflow ツールが使える場合**は、CLAUDE.md「Workflow ツールによる fan-out（可用時・任意）」に従い多観点レビューを per-task fan-out してよい＝出力は同じ接頭辞付き指摘一覧。使えなければ本スキルの順次レビューのまま。）

## レビュー対象

$ARGUMENTS が指定された場合はそのファイルまたは diff、指定がない場合は `git diff <defaultBranch>...HEAD` を確認します（`<defaultBranch>` は CLAUDE.md「プロジェクト設定」の値。未設定なら `develop`。`release/*` / `hotfix/*` は `git diff main...HEAD`）。

## 実施手順

1. 変更内容を把握する
2. `.claude/rules/code-review.md` に定義されたレビュー観点・コメント接頭辞に従ってレビューする
3. 各コメントに接頭辞（`must:` / `should:` / `nit:` / `question:`）を付けて記述する
