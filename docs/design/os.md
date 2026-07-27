---
title: os 領域 設計
area: os
status: draft
relatedIssues: [46]
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
| `keys.rs` | `~/.ssh` 再帰探索（最初のヘッダ行のみ sniff）・フィンガープリントで公開/秘密をペアリング |
| `known_hosts.rs` | 解析・内容アドレスでの削除（`.old` バックアップ）・スキャン結果の追記 `append_entries`（CRLF/末尾改行を保存・原子的置換） |
| `keyscan.rs`（#46 で新規） | `ssh-keyscan` によるホスト鍵の事前スキャン。専用ワーカー（thread＋`mpsc`・tick drain）で実行し、取得行を `secure_fs::temp_name` の一時ファイル経由で `ssh-keygen -lv -f` に渡してフィンガープリント＋randomart 化 |
| `liveness.rs` | ワーカースレッドプールで TCP 到達性プローブ・`mpsc` 報告・`App::hosts` index キー |
| `resolve.rs` | `ssh -G` による設定解決 |
| `history.rs`・`prefs.rs` | 非秘密の平 JSON（接続履歴・オートフィル opt-in） |
| `binaries.rs`・`clipboard.rs` | ツール探索（System32 優先）・クリップボード |
| `vault.rs`・`askpass.rs` | 暗号化秘密ストア・接続時オートフィル（→ [security.md](./security.md)） |

## データフロー・主要シーケンス

到達性プローブ: UI スレッドは tick ごとに `App::drain_liveness()` を呼ぶだけ（非ブロック）。プロキシ経由（`is_proxied`）は `Skipped`。

ホスト鍵の事前スキャン（#46・keyscan → ピン留め）:

```mermaid
sequenceDiagram
    participant UI as update.rs（ActionMenu）
    participant W as keyscan ワーカー（thread）
    participant KS as ssh-keyscan
    participant KG as ssh-keygen -lv
    participant KH as known_hosts.rs
    UI->>UI: resolve_config_with_options で host/port 解決（先頭ダッシュ拒否）
    UI->>W: scan 要求（mpsc・モーダルは scanning 状態）
    W->>KS: spawn（-p port・-T 秒・数秒バジェットの try_wait/kill ループ）
    KS-->>W: 「host keytype base64」行
    W->>KG: 一時ファイル（secure_fs::temp_name）経由でフィンガープリント+randomart
    KG-->>W: SHA256 + randomart ブロック
    W-->>UI: tick drain でイベント回収（既存鍵と一致/不一致/新規を分類）
    UI->>KH: [y] 承認時のみ append_entries（ホストトークンは tofu_lookup_key に書き換え）
```

## 外部依存・インターフェース

- OpenSSH（`ssh`/`ssh-keygen`/`sftp`）・`wt.exe`（Windows 新規タブ）。
- `config/`（接続対象の解決に `HostView` を参照）。

## 主要な設計判断（現行の理由）

- **config を接続の真実源に**: 保存済みは `ssh <alias>` で OpenSSH に config を読ませる（ProxyJump/forwards/IdentityFile が自動適用）。
- **秘密鍵本体を読まない**: 鍵ペアリングは公開フィンガープリント照合のみ。暗号化 PEM は `Unverified`（エラーにしない）。
- **liveness index キーの脆さ**: ホスト追加/削除で index がずれるため `rebuild_hosts()` が liveness マップをクリアし再プローブ。
- **keyscan は専用ワーカー（#46）**: keyscan は数秒かかるため UI スレッドで実行しない。`SftpSession::request` 型（thread＋`mpsc`・tick drain・`is_finished` reap）を踏襲する。liveness プールはジョブ型・ホスト index キーが固定で不適合、同期実行は `draw()` をブロックするため不採用。
- **ピン留めのホストトークンは `tofu_lookup_key` の出力に書き換え（#46）**: ゲート判定 `is_host_known`（`ssh-keygen -F`）と確実に一致させるため、keyscan の出力行のホスト部を `HostKeyAlias` 優先／非 22 番ポート `[host]:port` の検索キーに正規化して追記する。追記はプレーン形式（ハッシュ化しない）。
