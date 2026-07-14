---
title: os 領域 設計
area: os
status: draft
relatedIssues: [43]
updated: 2026-07-14
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
| `known_hosts.rs` | 解析・内容アドレスでの削除（`.old` バックアップ） |
| `liveness.rs` | ワーカースレッドプールで TCP 到達性プローブ・`mpsc` 報告・`App::hosts` index キー |
| `resolve.rs` | `ssh -G` による設定解決。型付き `ResolvedConfig`（vault 自動入力用の抽出サブセット）と、実効設定インスペクタ（#43）向けの `resolve_full`（順序付き `Vec<(String, String)>`＝全 key/value を欠かさず保持）の 2 経路。`run_ssh_g` を「生ダンプ取得」と「型付きパース」に分割 |
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
- **`resolve_full` は型付き経路と生ダンプ取得を共有し、パースだけ分ける（#43）**: `run_ssh_g` の subprocess 実行部（500ms タイムアウト・stdin null・kill-on-timeout・`--` センチネル＋先頭ダッシュ拒否）を生ダンプ取得として括り出し、型付き `parse_ssh_g_output`（抽出サブセット）と `resolve_full` 用の全 key/value パース（順序保持）が同じダンプを読む。インスペクタは「書いた値」との由来比較をせず `ssh -G` の正規化出力をそのまま近似として見せる（キー小文字化・値正規化・コンパイル時デフォルト混入があるため、単純比較は誤分類する＝Issue #43 リスク#2）。ratatui 非依存を維持し、パース分割はヘッドレステスト可能。
- **インスペクタの安全ゲートは text scan で判定する（#43 リスク#1）**: `inspect_block_reason` は `has_match_exec`（既存）と `has_include`（新規）で config テキストを走査し、`Match exec` または `Include` があれば `ssh -G` 実行を退避する。両者とも同じ widen-only 正規化（クオート splice 除去・`=`→空白・コメント除外・インデント非依存）を使い、パーサの `include_count()`（素のトップレベル Include のみ）では取りこぼす**ブロック内ネスト**・**クオート装飾**の Include も捕捉する。接続時の autofill 経路（`has_match_exec` のみで Include を見ない）より厳格＝閲覧専用の診断ビューは保守的に倒す。
