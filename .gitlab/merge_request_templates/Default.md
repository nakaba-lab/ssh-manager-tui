<!-- MR 説明の雛形（手動で MR を作るメンバー向け。Default テンプレートとして新規 MR に自動適用される）。
     /ship・/pr-description も同系の構成（概要・変更内容・テスト方法・Closes #N）で説明文を生成する —
     本テンプレートはそれに設計書（docs/design/）節と CI 逃がし（Test: none／Design: none）の案内を加えた完全版。
     注意: 逃がし行（「Test: none」「Design: none」）は行頭に書いたときだけ CI に効く（コメント内・行中では発火しない） -->

## 概要

<!-- この変更の背景・目的を1〜3文で -->

## 変更内容

- 

## テスト方法

<!-- 実行したテスト・確認手順をチェックリストで書く（テストコマンドは CLAUDE.md「プロジェクト設定」参照）。
     feat/fix にはテストの追加・更新が必須（app-test CI の test-required ジョブが検査する）。
     テスト不要な変更なら、この節に行頭から「Test: none」と 1 行書くか、MR に「test:none」ラベルを付ける -->

- [ ] 

## 設計書（docs/design/）

<!-- feat/fix は docs/design/（生きた設計書）の更新を同じ MR に含める（design-doc-check CI が検査する。
     詳細は .claude/rules/design-doc.md）。設計変更を伴わない場合は、
     この節に行頭から「Design: none」と 1 行書くか、MR に「design:none」ラベルを付ける -->

- [ ] `docs/design/` を現状に合わせて更新した（設計変更なしの場合は上記の逃がしを明記）

## レビュー記録

<!-- 「コードレビュー」タスク（code-reviewer）の指摘一覧と対応結果をここに転記する。
     接頭辞は .claude/rules/code-review.md に従う（must:/should:/nit:/question:）。
     must: は全件対応済み（0 件）であること。指摘が無ければ「指摘なし」と 1 行書く -->

| 指摘（接頭辞付き） | 対応結果 |
|--------------------|----------|
|                    |          |

## スクリーンショット（UI 変更時は必須）

<!-- UI を変更した場合は Before / After を貼る。画像は「ビジュアル/UX 確認」タスク／frontend-reviewer が
     docs/screenshots/ に保存した <issue>-<画面>-<ブレークポイント>-<before|after>.png を使う。
     frontendDir 配下（"." の場合は UI ファイル）に変更が無ければこの節は削除してよい -->

| Before | After |
|--------|-------|
|        |       |

- [ ] 主要ブレークポイント（モバイル/デスクトップ）で表示確認済み

## 関連 Issues

<!-- 「Closes #N」の自動クローズはデフォルトブランチへのマージ時のみ発火する。マージ後は /gitlab-finalize で明示クローズする -->

Closes #
