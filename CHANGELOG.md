# Changelog

All notable changes to this project are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

<!-- マージ毎にここへ利用者向けの変更を追記し、リリース時に /release-notes で版に確定する。
     カテゴリ: Added / Changed / Deprecated / Removed / Fixed / Security -->

### Added

- ホスト鍵の事前スキャン＆ピン留め: アクションメニュー（`o`）の「Scan host key」で接続前に `ssh-keyscan` を実行し、フィンガープリント（SHA256）と randomart を確認したうえで `y` で `known_hosts` に追記できる。初回接続時の TOFU プロンプトが不要になり、保存済み秘密の自動入力ブロックも解消する。既存の鍵と一致する鍵は重複追記しない。**取得した鍵が既存のピンと矛盾する場合（HOST KEY CHANGED）や取り消し済み（`@revoked`）の場合は、その結果セットからは一切ピン留めしない**（警告のみ・上書き手段は提供しない）。ピン留め先はそのホストに有効な `UserKnownHostsFile`（#46）

### Changed

- ホスト鍵未信頼で自動入力が保留されたときのメッセージが、「Scan host key」でピン留めできることを案内するようになった（#46）

---

> このファイルは v1.2.0 以降の変更を記録する。v1.0.0〜v1.2.0 のリリース履歴は
> [GitHub Releases](https://github.com/nakaba-lab/ssh-manager-tui/releases) を参照。

[Unreleased]: https://github.com/nakaba-lab/ssh-manager-tui/compare/v1.2.0...HEAD
