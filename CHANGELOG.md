# Changelog

All notable changes to this project are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

<!-- マージ毎にここへ利用者向けの変更を追記し、リリース時に /release-notes で版に確定する。
     カテゴリ: Added / Changed / Deprecated / Removed / Fixed / Security -->

### Added

- ホスト一覧で `i` を押すと、選択ホストの実効設定（`ssh -G` の解決結果）を
  フィルタ・スクロール可能な画面で確認できる実効設定インスペクタを追加。ワイルドカードや
  `Match` があっても「実際に効く設定」が一目で分かる。`Match exec` や `Include` を含む
  config では、`ssh -G` が述語を実行しうるため安全のためスキップする（#43）。
- パスワード vault のマスターパスワードを変更できるようになった（vault 画面で `m`）。
  現在のパスワードを確認のうえ、新しい salt と最新の KDF 設定で vault 全体を再暗号化する。
  あわせて、古い KDF パラメータで作られた vault を最新デフォルトへ再鍵化する「KDF 昇格」を
  追加（現デフォルトより弱い場合のみ vault 画面に導線と `u` キーが現れる）。保存に失敗した
  場合はメモリ上の鍵を元に戻すので、鍵とディスクが食い違って vault が開けなくなることはない（#44）。

---

> このファイルは v1.2.0 以降の変更を記録する。v1.0.0〜v1.2.0 のリリース履歴は
> [GitHub Releases](https://github.com/nakaba-lab/ssh-manager-tui/releases) を参照。

[Unreleased]: https://github.com/nakaba-lab/ssh-manager-tui/compare/v1.2.0...HEAD
