---
title: os 領域 設計
area: os
status: draft
relatedIssues: [47]
updated: 2026-07-27
---

# os 領域 設計（`src/os/`）

> status: draft — 初期骨子。実装の現状に合わせて随時確定する。

## 責務

外界との統合すべて（外部プロセス・ネットワーク・鍵ツール・秘密ストア）。**ratatui 依存ゼロ**。秘密・信頼境界を扱う vault/askpass は [security.md](./security.md) に分離。

## 構成要素

| モジュール | 責務 |
|-----------|------|
| `connect.rs` | `ssh`/`sftp` の起動（保存済みは `ssh <alias>`、override 時のみフラグ発行）・`wt.exe` 新規タブ |
| `sftp.rs` | `ls -l` 一覧パース ＋ ライブ `SftpSession` browse ワーカー（短命 `sftp -b`・circuit-breaker） |
| `keys.rs` | `~/.ssh` 再帰探索（最初のヘッダ行のみ sniff）・フィンガープリントで公開/秘密をペアリング・鍵生成（パスフレーズなし/対話）・パスフレーズ変更の引数構築（`change_passphrase_args`） |
| `known_hosts.rs` | 解析・内容アドレスでの削除（`.old` バックアップ） |
| `liveness.rs` | ワーカースレッドプールで TCP 到達性プローブ・`mpsc` 報告・`App::hosts` index キー |
| `resolve.rs` | `ssh -G` による設定解決 |
| `history.rs`・`prefs.rs` | 非秘密の平 JSON（接続履歴・オートフィル opt-in） |
| `binaries.rs`・`clipboard.rs` | ツール探索（System32 優先）・クリップボード |
| `vault.rs`・`askpass.rs` | 暗号化秘密ストア・接続時オートフィル（→ [security.md](./security.md)） |

## データフロー・主要シーケンス

到達性プローブ: UI スレッドは tick ごとに `App::drain_liveness()` を呼ぶだけ（非ブロック）。プロキシ経由（`is_proxied`）は `Skipped`。

## 外部依存・インターフェース

- OpenSSH（`ssh`/`ssh-keygen`/`sftp`）・`wt.exe`（Windows 新規タブ）。
- `config/`（接続対象の解決に `HostView` を参照）。

## 主要な設計判断（現行の理由）

- **config を接続の真実源に**: 保存済みは `ssh <alias>` で OpenSSH に config を読ませる（ProxyJump/forwards/IdentityFile が自動適用）。
- **秘密鍵本体を読まない**: 鍵ペアリングは公開フィンガープリント照合のみ。暗号化 PEM は `Unverified`（エラーにしない）。
- **liveness index キーの脆さ**: ホスト追加/削除で index がずれるため `rebuild_hosts()` が liveness マップをクリアし再プローブ。
- **パスフレーズ操作は引数ビルダーと実行を分離**（#47）: `change_passphrase_args`／`generate_key_args`（いずれも純粋・`OsString` を返すのでパスを lossy 変換しない）を `keys.rs` に置き、実行は `update.rs` の `run_ssh_keygen_inline`（`suspend_tui` → `run_inline` → `restore_tui` → `describe_exit`）が一手に担う（os 層の ratatui 非依存を維持）。現在/新パスフレーズは **OpenSSH 自身が対話聴取**し、sshm は値を保持も中継もしない（コマンドラインにも載らない）。`generate_key` も同じビルダーを通し、非対話（`-N ""`/`-q`）と対話（両フラグを省く）の差分をビルダー 1 箇所に閉じる。
- **鍵ユーザーの逆引きは config 射影で行う**（#47）: `hosts_using_key` は純粋関数で、`ssh -G` を全ホスト分 spawn する案は起動遅延と副作用（`Match exec` の再実行）を招くため採らない。**照合規則は接続時オートフィルと一致させる**（非 glob パターン全走査・Windows のパス畳み込み・IdentityFile 未宣言時の既定 identity）＝ずれると陳腐化の取りこぼしになる。詳細は [security.md](./security.md)。
