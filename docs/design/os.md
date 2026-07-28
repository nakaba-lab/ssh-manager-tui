---
title: os 領域 設計
area: os
status: active
relatedIssues: [43, 48, 52, 65]
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
| `deploy.rs`（#48） | 公開鍵のリモート配布（ssh-copy-id 相当）の**純粋部**＝`.pub` 行の検証／リモート sh スニペット組み立て／終了コードの意味づけ。プロセス起動そのものは `update.rs` の既存インライン経路（`suspend_tui`→`run_inline`→`restore_tui`）が担い、本モジュールは I/O を持たない |
| `known_hosts.rs` | 解析・内容アドレスでの削除（`.old` バックアップ） |
| `liveness.rs` | ワーカースレッドプールで TCP 到達性プローブ・`mpsc` 報告・`App::hosts` index キー |
| `resolve.rs` | `ssh -G` による設定解決。型付き `ResolvedConfig`（vault 自動入力用の抽出サブセット）と、実効設定インスペクタ（#43）向けの `resolve_full`（順序付き `Vec<(String, String)>`＝全 key/value を欠かさず保持）の 2 経路。`run_ssh_g` を「生ダンプ取得」と「型付きパース」に分割 |
| `history.rs`・`prefs.rs` | 非秘密の平 JSON（接続履歴・オートフィル opt-in） |
| `binaries.rs`・`clipboard.rs` | ツール探索（System32 優先）・クリップボード |
| `vault.rs`・`askpass.rs` | 暗号化秘密ストア・接続時オートフィル（→ [security.md](./security.md)） |

## データフロー・主要シーケンス

到達性プローブ: UI スレッドは tick ごとに `App::drain_liveness()` を呼ぶだけ（非ブロック）。プロキシ経由（`is_proxied`）は `Skipped`。

公開鍵配布（#48）は**リモート往復 1 回**で完結する。スニペット組み立て・検証・終了コード解釈という判断はすべて `deploy.rs` の純粋関数に閉じ、副作用（TUI の suspend／`ssh` 起動）は `update.rs` に残す（`config/`・`os/` から ratatui に手を伸ばさない層規律のまま）:

```mermaid
sequenceDiagram
    participant U as ユーザー
    participant KM as 鍵マネージャ (update.rs)
    participant D as os/deploy.rs (純粋)
    participant SSH as ssh (inline)
    participant R as リモート ~/.ssh/authorized_keys

    U->>KM: p（配布）
    KM->>D: deploy_snippet(pub_line)
    D-->>KM: Err(NotAPublicKey / UnsafeBody) → 拒否トースト（コマンドを組み立てない）
    D-->>KM: Ok(snippet)  ※本体は allowlist 検証済み・コメントは通過時のみ保持
    KM->>U: 確認モーダル（alias・鍵名・fingerprint・コメント保持の有無）
    U->>KM: y / Enter
    KM->>SSH: suspend_tui → run_inline(ssh, [alias, snippet])
    Note over SSH,R: stdio 継承なのでパスワード認証・<br/>初回 host-key 確認がそのまま見える
    SSH->>R: umask 077; mkdir -p ~/.ssh; grep -qF <body> || printf >> authorized_keys
    R-->>SSH: exit 0（追記）／3（既にあり）／その他（失敗）
    SSH-->>KM: ExitStatus → restore_tui
    KM->>D: classify_exit(code)
    D-->>KM: Added / AlreadyPresent / SshError / RemoteFailed
    KM->>U: 結果トースト（失敗は sticky）
```

## 外部依存・インターフェース

- OpenSSH（`ssh`/`ssh-keygen`/`sftp`）・`wt.exe`（Windows 新規タブ）。
- `config/`（接続対象の解決に `HostView` を参照）。

## 主要な設計判断（現行の理由）

- **config を接続の真実源に**: 保存済みは `ssh <alias>` で OpenSSH に config を読ませる（ProxyJump/forwards/IdentityFile が自動適用）。
- **秘密鍵本体を読まない**: 鍵ペアリングは公開フィンガープリント照合のみ。暗号化 PEM は `Unverified`（エラーにしない）。
- **liveness index キーの脆さ**: ホスト追加/削除で index がずれるため `rebuild_hosts()` が liveness マップをクリアし再プローブ。
- **`resolve_full` は型付き経路と生ダンプ取得を共有し、パースだけ分ける（#43）**: `run_ssh_g` の subprocess 実行部（500ms タイムアウト・stdin null・kill-on-timeout・`--` センチネル＋先頭ダッシュ拒否）を生ダンプ取得として括り出し、型付き `parse_ssh_g_output`（抽出サブセット）と `resolve_full` 用の全 key/value パース（順序保持）が同じダンプを読む。インスペクタは「書いた値」との由来比較をせず `ssh -G` の正規化出力をそのまま近似として見せる（キー小文字化・値正規化・コンパイル時デフォルト混入があるため、単純比較は誤分類する＝Issue #43 リスク#2）。ratatui 非依存を維持し、パース分割はヘッドレステスト可能。
- **公開鍵配布は「純粋なスニペット組み立て」と「既存インライン実行」に分ける（#48）**: `deploy.rs` は I/O を持たず `deploy_snippet()`（`.pub` 行 → リモート sh 文字列）と `classify_exit()`（終了コード → 結果）だけを提供し、ヘッドレスにユニットテストする。実行は `execute_sftp_transfer` と同じインライン経路（`suspend_tui`→`run_inline`→`restore_tui`）を使う。**バックグラウンドワーカー案（`BatchMode=yes`）を採らない**のは、鍵をまだ配っていないホスト＝この機能が要る当の場面では対話的パスワード認証（と初回の host-key TOFU 確認）が必要で、非対話実行はほぼ必ず失敗するため。**ハイブリッド案（非対話で試して失敗したら対話で再実行）**も、Issue のリスク欄が挙げる「1 往復」方針に反し失敗分類の分岐が増えるため不採用。
- **結果の判別は stdout ではなく終了コードで運ぶ（#48）**: インライン実行は stdio を子に継承するため親プロセスは出力を読めない。そこでスニペット側が「追記した＝0／既にあった＝3」を終了コードで返し、`classify_exit` が `Added`/`AlreadyPresent`/`SshError`（255＝ssh 自身の失敗）/`RemoteFailed`（その他）に写す。重複判定は `grep -qF` に**コメントを除いた `<algo> <blob>` 本体**を渡すので、リモート側のコメントが違っても・オプション前置きが付いていても同じ鍵とみなす（`authorized_keys` の全文検索であり行構造は解釈しない＝コメントアウトされた行にも一致しうる近似。ssh-copy-id と同程度の割り切り）。
- **公開鍵行は「本体は検証して拒否・コメントは通れば保持」（#48）**: `.pub` は `~/.ssh` 配下の——攻撃者に影響されうる——ファイルであり、その中身がリモートで実行される sh コマンドに埋め込まれる（CWE-88 系）。`<algo> <blob>` 本体が allowlist（英数と `+/=@.-_`）から外れたら**配布自体を拒否**し、コメントは同じ allowlist（＋空白・長さ上限）を通ったときだけ添えて送り、通らなければ黙って落として本体だけ送る。**エスケープは一切しない**（`tokens.rs`・`sftp_quote` と同じ「エスケープせず拒否」方針）。コメントを常に捨てる案より優れるのは、リモートの `authorized_keys` に人が読めるラベルが残り後の棚卸し・失効ができる点。
- **`ssh -G` 実行前の安全ゲートは app 層に集約（#43 → #52 → #65）**: `os/resolve.rs` が提供するのは `has_match_exec`（widen-only 正規化＝クオート splice 除去・`=`→空白・コメント除外・インデント非依存の text scan）**のみ**で、ゲートの判断そのものは `App::ssh_g_exec_risk()` が持つ。3 つの `ssh -G` 呼び出し経路（接続時 autofill・SFTP arm・実効設定インスペクタ）はすべてこの単一ゲートを通り、included ファイルまで走査したうえで退避を決める（判定順・`blind_spot` の定義は [includes.md](./includes.md) が正）。#43 当時の `inspect_block_reason` / `has_include`（`Include` があれば一律退避する text scan）は #52 で included ファイルを実際に読めるようになったため**削除済み**。
