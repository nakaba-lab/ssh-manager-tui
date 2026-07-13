---
name: pr-description
description: 既存ブランチのコミット履歴と差分から PR/MR の説明文とタイトル（Conventional Commits 形式）を生成するとき使用する。「PR説明文を作って」「PRの説明を書いて」「マージリクエストの説明文を準備して」が発火ワード。説明文の生成のみで push・PR/MR の作成・マージはしない（それは /ship）。コミットメッセージ作成のみなら /git-commit。
---

コミット履歴を分析して、プルリクエストの説明文を生成します。

## 手順

1. ベースブランチを決定する（`$ARGUMENTS` が指定された場合はそれを使用、なければ CLAUDE.md「プロジェクト設定」の `defaultBranch`（未設定なら `develop`）。`release/*` / `hotfix/*` の場合のみ `main`）
2. `git log $ARGUMENTS..HEAD --oneline` を実行してコミット一覧を取得する
3. `git diff $ARGUMENTS...HEAD --stat` で変更ファイルを確認する
4. テンプレートに従ってPR説明文を生成する

## PR テンプレート

```markdown
## 概要
<!-- この変更の背景・目的を1〜3文で -->

## 変更内容
- 

## テスト方法
- [ ] 

## 関連 Issues
Closes #
```

生成後、PRタイトルもConventional Commits形式で提案する。
