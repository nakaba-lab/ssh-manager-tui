---
name: git-commit
description: コミットを依頼されたとき使用する。「コミットして」「commitして」「変更を記録して」「コミットメッセージを作って」が発火ワード。既存コミットのamend依頼やgit操作の説明依頼では使わない。
---

ステージング済みの変更を分析して Conventional Commits 形式のメッセージを作り、確認のうえ `git commit -F` でコミットします（**1 行だけの `-m` は使わない**＝body が落ちるため）。

## 手順

1. `git diff --staged` で変更内容を確認する（未ステージなら `git add` の要否を確認する）
2. 変更の性質を判断する（新機能/バグ修正/ドキュメント/リファクタリング等）
3. 下記フォーマットでメッセージを**提案**する（subject + 空行 + body。必要なら footer）
4. 利用者の確認を得る（`$ARGUMENTS` に方針があれば反映する）
5. 確認後、**メッセージ全文を一時ファイルに書いて `git commit -F <file>` でコミット**し、結果（`git log -1 --stat`）を提示する

## Conventional Commits フォーマット

フォーマット・コミットタイプ・ブランチ対応・Breaking Change の書き方は `.claude/rules/git-workflow.md` の「コミットメッセージ」セクションを参照すること。**subject `<type>(<scope>): <説明>`（scope 必須）に加え、body（WHY と影響範囲）を必ず書く。**

## コミットの実行（多行を落とさない）

**`git commit -m "<subject>"`（1 行）は使わない**。全文を一時ファイルに書いて `-F` で渡す：

```bash
MSG="$(git rev-parse --git-dir)/COMMIT_MSG"
cat > "$MSG" <<'EOF'
feat(auth): ログイン API を追加

未認証ユーザーのアクセス導線が無かったため追加。/login 経路に影響する。

Refs #42
EOF
git commit -F "$MSG"
```

- **footer はコミットでは `Refs #<issue>`（追跡用の参照）**。Issue を閉じる `Closes #<issue>` は **PR/MR 本文**に書く（squash マージで個々のコミット footer が重複しないため。`.claude/rules/git-workflow.md` 参照）。
- ブランチ名にチケット番号があれば、出荷の `commit-msg` フックが `Refs #N` を自動補完する（`git config core.hooksPath .githooks` 設定時。`/project-setup` が設定する）。フックは subject 形式・body 必須も検証し、不適合ならコミットを拒否する。

## フック未設定時の確認（body を落とさない）

`commit-msg` フックは body 必須・scope 必須を強制する**唯一のガード**だが、`git config core.hooksPath .githooks` 未設定の環境では発火しない（git config はクローンに継承されない／`/project-setup` は初回コミット後に設定するため、設定漏れが起きやすい）。フックが無いと subject 1 行だけのコミットが通ってしまう。

- コミット前に `git config --get core.hooksPath` を確認する。`.githooks` を返さなければ、「commit-msg フックが無効＝body・scope が強制されない」旨を利用者に伝え、`git config core.hooksPath .githooks` の実行を**案内する**（git config の変更は外部影響なので、勝手に設定せず利用者に委ねる）。
- フックの有無に関わらず、`-F` で **body（WHY と影響範囲）を必ず書く**。コミット後に `git log -1 --format=%B` を確認し、subject の後に空行＋body が揃っていることを目視する。body が欠落していたら `git commit --amend -F <file>` で補う。

## 出力形式

コミットメッセージをコードブロックで提示し、選択した type の理由を一言添える。確認後にコミットを実行し、`git log -1 --stat` の結果を提示する。

`$ARGUMENTS` がある場合はそれをコンテキストとして考慮する。
