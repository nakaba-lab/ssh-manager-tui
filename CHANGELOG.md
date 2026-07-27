# Changelog

All notable changes to this project are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

<!-- マージ毎にここへ利用者向けの変更を追記し、リリース時に /release-notes で版に確定する。
     カテゴリ: Added / Changed / Deprecated / Removed / Fixed / Security -->

### Added

- 鍵マネージャの `p` で鍵のパスフレーズを追加・変更できるようになった（`ssh-keygen -p` をインライン実行し、パスフレーズは OpenSSH 自身が聴取する）。(#47)
- 鍵生成ウィザードに「Passphrase: none / interactive」トグルを追加。`interactive` を選ぶとパスフレーズ付きの鍵を生成できる。(#47)
- パスフレーズ変更後、その鍵を `IdentityFile` に使うホストの保存済みパスフレーズが古くなった場合に検出し、新しいパスフレーズ 1 回の入力でまとめて更新できるようになった（スキップ可）。(#47)

### Changed

- 鍵詳細ペインの `unverified` 表示の説明を「パスフレーズ無しでは検証できない（エラーではない）」に変更。パスフレーズで保護した鍵が壊れたように見える誤解を防ぐ。(#47)

---

> このファイルは v1.2.0 以降の変更を記録する。v1.0.0〜v1.2.0 のリリース履歴は
> [GitHub Releases](https://github.com/nakaba-lab/ssh-manager-tui/releases) を参照。

[Unreleased]: https://github.com/nakaba-lab/ssh-manager-tui/compare/v1.2.0...HEAD
