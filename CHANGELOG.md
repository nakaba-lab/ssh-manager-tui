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
  `Match` があっても「実際に効く設定」が一目で分かる。`Match exec` を含む config
  （`Include` 先のファイルにあるものも含む）では、`ssh -G` が述語を実行しうるため安全のため
  スキップする。この安全判定は接続時オートフィル・SFTP・インスペクタの 3 経路で共通で、
  sshm が追えない `Include` 形式のときも安全側に退避する（#43・#65）。
- パスワード vault のマスターパスワードを変更できるようになった（vault 画面で `m`）。
  現在のパスワードを確認のうえ、新しい salt と最新の KDF 設定で vault 全体を再暗号化する。
  あわせて、古い KDF パラメータで作られた vault を最新デフォルトへ再鍵化する「KDF 昇格」を
  追加（現デフォルトより弱い場合のみ vault 画面に導線と `u` キーが現れる）。保存に失敗した
  場合はメモリ上の鍵を元に戻すので、鍵とディスクが食い違って vault が開けなくなることはない（#44）。
- ホストにタグと 1 行の説明を付けられるようになった。`~/.ssh/config` のホスト直上コメント
  （`# sshm:tags prod,db` / `# sshm:desc …`）に永続化するため、並行 DB を持たず実ファイルが
  真実の源泉のまま。タグは一覧でエイリアス右にチップ表示され、`/` 検索の対象にもなる（既存の
  ファジー検索に畳み込み）。編集フォームの Metadata 欄で編集でき、`# sshm:` 接頭辞の付かない
  第三者コメント（`# Managed by Ansible` 等）はバイト単位で保持され書き換えない（#45）。
- `~/.ssh/config` の `Include` で分割された構成（1Password / Ansible 等が生成する `config.d/*`
  など）のホストも一覧・閲覧できるようになった。included ファイルのホストは元ファイル名を添えて
  表示され、閲覧専用（誤って別ファイルへ書き込まないよう、編集・削除は元ファイルのメインホストのみ）。
  同名エイリアスの重複は OpenSSH の先勝ちを `⊘` で明示する。チルダ・相対パス・glob（`config.d/*`）を
  OpenSSH 準拠で解決し、循環や深いネストも安全に打ち切る（#52）。

### Security

- Windows で System32 OpenSSH が見つからず PATH の `ssh`（Git/MSYS 由来）にフォールバック
  している場合、実効設定インスペクタ（`i`）を開かず安全のためスキップするようにした。未検証の
  `ssh` は `%HOME%` を優先するため、sshm が安全確認した config とは別のファイルを `ssh -G` が
  読み、未走査の `Match exec` を実行しうるため。これで接続時オートフィル・SFTP・インスペクタの
  3 経路が同じクライアント信頼基準に揃った（#73）。

---

> このファイルは v1.2.0 以降の変更を記録する。v1.0.0〜v1.2.0 のリリース履歴は
> [GitHub Releases](https://github.com/nakaba-lab/ssh-manager-tui/releases) を参照。

[Unreleased]: https://github.com/nakaba-lab/ssh-manager-tui/compare/v1.2.0...HEAD
