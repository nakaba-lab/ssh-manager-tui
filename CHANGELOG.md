# Changelog

All notable changes to this project are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

<!-- マージ毎にここへ利用者向けの変更を追記し、リリース時に /release-notes で版に確定する。
     カテゴリ: Added / Changed / Deprecated / Removed / Fixed / Security -->

### Added

- 鍵マネージャに ssh-agent パネルを追加: agent の稼働状態と保持鍵数、選択中の鍵が agent にロード済みかの表示、および Windows では ssh-agent サービスの状態を表示する。ロード済みの鍵は一覧に `agent` バッジが付く ([#49](https://github.com/nakaba-lab/ssh-manager-tui/issues/49))
- 鍵マネージャのキー操作 `a`（agent にロード）/ `D`（agent からアンロード）。パスフレーズ付きの鍵は OpenSSH 自身がターミナルで尋ねる（sshm はパスフレーズを保持しない） ([#49](https://github.com/nakaba-lab/ssh-manager-tui/issues/49))

### Changed

- 鍵マネージャのフッターのラベルを短縮（`generate`→`gen`、`copy pub`→`copy`）。agent の 2 操作を足すと 80 桁端末で末尾のヒントが切れるため。完全な説明は `?` のヘルプ画面にある ([#49](https://github.com/nakaba-lab/ssh-manager-tui/issues/49))

---

> このファイルは v1.2.0 以降の変更を記録する。v1.0.0〜v1.2.0 のリリース履歴は
> [GitHub Releases](https://github.com/nakaba-lab/ssh-manager-tui/releases) を参照。

[Unreleased]: https://github.com/nakaba-lab/ssh-manager-tui/compare/v1.2.0...HEAD
