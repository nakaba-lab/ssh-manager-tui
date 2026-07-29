# Changelog

All notable changes to this project are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

<!-- マージ毎にここへ利用者向けの変更を追記し、リリース時に /release-notes で版に確定する。
     カテゴリ: Added / Changed / Deprecated / Removed / Fixed / Security -->

### Added

- ホスト鍵の事前スキャン＆ピン留め: アクションメニュー（`o`）の「Scan host key」で接続前に `ssh-keyscan` を実行し、フィンガープリント（SHA256）と randomart を確認したうえで `y` で `known_hosts` に追記できる。初回接続時の TOFU プロンプトが不要になり、保存済み秘密の自動入力ブロックも解消する。**ピン留めできるのはまだピンの無いホストだけ**（スキャンは応答者が本物かを検証しないため、既存のピンの隣に鍵を足すことはしない。鍵の入れ替えが正当な場合は Known hosts 画面〔`H`〕で既存ピンを削除してから再スキャンする）。取得鍵が既存ピンと矛盾する場合（HOST KEY CHANGED）や取り消し済み（`@revoked`）の場合も警告のみで、上書き手段は提供しない。ピン留め先はそのホストに有効な `UserKnownHostsFile` で、モーダルには追記先ファイルと**どのホスト名で記録されるか**（`HostKeyAlias` を使う設定では別名になる）を表示する。なお、`Match exec`／`Match localnetwork` のように**接続のたびに `UserKnownHostsFile` の解決が変わりうる設定**では、スキャン時に見えなかったピンが接続時に読まれる可能性があり、この機能では検出できない — その場合は手動でのピン留めを推奨する（#46）

### Changed

- ホスト鍵未信頼で自動入力が保留されたときのメッセージが、「Scan host key」でピン留めできることを案内するようになった（#46）

---

> このファイルは v1.2.0 以降の変更を記録する。v1.0.0〜v1.2.0 のリリース履歴は
> [GitHub Releases](https://github.com/nakaba-lab/ssh-manager-tui/releases) を参照。

[Unreleased]: https://github.com/nakaba-lab/ssh-manager-tui/compare/v1.2.0...HEAD
