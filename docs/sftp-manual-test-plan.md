# sshm SFTP 手動テストシナリオ

CI には `ssh`/`sftp` バイナリが無く、SFTP のライブ挙動（実サーバ相手の接続・転送・ブラウズ）は
自動テストできない。純粋ロジック（`parse_ls_l` / `sftp_quote` / `apply_sftp_event` /
`build_sftp_args` / footer 幅 等）は `cargo test` 済みなので、ここでは **実サーバ相手の挙動**と
**プラットフォーム固有の挙動**だけを手動で確認する。

各シナリオは `[ID] 目的 / 前提 / 手順 / 期待結果` の形。`☐` をチェックして使う。
レビューで「実機検証が必須」とされた指摘（F3/F9, F4, F5, F8, L2, L3 …）には末尾に `→ <id>` で
トレースを付けた。

---

## 0. 前提とセットアップ

### 0.1 必要なもの
- OpenSSH クライアント（`ssh`, `ssh-keygen`, `sftp`）が PATH 上にあること。
  - Windows: `System32\OpenSSH` が優先される。Git/MSYS の `sftp` だと挙動が違うので、
    起動時に `[PATH ssh]` 警告が出ていないこと（出ていたら System32 OpenSSH を入れる）。
  - 新タブ起動を試すなら `wt.exe`（Windows Terminal）。
- テスト用 SFTP サーバ（下記のいずれか）。

### 0.2 テストサーバの用意（いずれか）
- **A. localhost の OpenSSH サーバ**（最も手軽）: ローカルに `sshd` を立て、自分のアカウントへ。
- **B. Docker**: `docker run -p 2222:22 -d atmoz/sftp foo:pass:::upload`
  （`foo`/`pass`、ポート 2222、`upload` ディレクトリ付き）。パスワード認証ホストの確認に便利。
- **C. 使い捨て VM / 既存の検証用ホスト**。
- 任意で **多段** 用に踏み台（bastion）を 1 台用意できると ProxyJump を確認できる。

### 0.3 スクラッチ config（本物の `~/.ssh/config` を汚さない）
`scratch-config` スキル、または手書きで使い捨て config を作り、`sshm --config <path>` で起動する。
**sshm は渡したパスへ書き込む**ので必ず使い捨てを使うこと。

認証バリアントを網羅できるよう、次のようなホストを定義しておく:

```sshconfig
# 鍵認証（パスフレーズ無し）
Host t-key
    HostName 127.0.0.1
    User youruser
    IdentityFile ~/.ssh/test_key

# 鍵認証（パスフレーズ付き → vault で自動入力を確認）
Host t-key-pp
    HostName 127.0.0.1
    User youruser
    IdentityFile ~/.ssh/test_key_pp

# パスワード認証（vault + 同意ゲートを確認）
Host t-pass
    HostName 127.0.0.1
    Port 2222
    User foo

# 多段（ProxyJump 透過を確認）
Host t-proxy
    HostName 10.0.0.5
    User youruser
    ProxyJump t-key

# スペース・特殊文字を含むパスのテスト用（同じ t-key を使ってもよい）
```

### 0.4 vault のセットアップ（パスフレーズ／パスワード自動入力の確認用）
1. `P` で vault を開き、マスターパスワードを設定（新規作成）。
2. `t-key-pp` の鍵パスフレーズを **Passphrase** として登録。
3. `t-pass` のログインパスワードを **Password** として登録。
4. パスワード自動入力はデフォルト OFF。`P` → `p` で **password auto-fill を ON** にしておく
   （Phase 2 の同意フローを確認するため）。

### 0.5 テストデータ（リモート側に用意）
- 通常ファイル: `hello.txt`
- スペース入りファイル: `my notes.txt`
- ディレクトリ: `sub/` とその中に `inner.txt`
- ディレクトリへの symlink: `current -> sub`
- ファイルへの symlink: `latest -> hello.txt`
- （任意）読めないディレクトリ: `mode 700` の他ユーザ home など → 失敗リスティング確認用

ローカル側にも `up me.txt`（スペース入り）等を置いておく。

---

## 1. Phase 1 — インタラクティブ SFTP セッション（`F` / アクションメニュー）

- **[P1-1] 鍵認証ホストでインライン起動**
  前提: `t-key` 選択。手順: `F` を押す。
  期待: TUI が一旦サスペンドし `sftp>` プロンプトが出る。`ls` / `pwd` が効く。`bye` で TUI に復帰し、
  終了トーストが出るか無言で戻る（exit 0 ならトースト無し）。

- **[P1-2] パスフレーズ自動入力**
  前提: `t-key-pp` 選択、vault unlock 済み。手順: `F`。
  期待: パスフレーズを **聞かれずに** sftp プロンプトに入れる（askpass がパスフレーズを供給）。

- **[P1-3] パスワードホスト（手入力）**
  前提: `t-pass` 選択。手順: `F`。
  期待: サスペンドした端末で `...@...'s password:` が出て、**自分でパスワード入力**して入れる
  （インラインなのでハングしない）。

- **[P1-4] 多段（ProxyJump）透過**
  前提: `t-proxy` 選択。手順: `F`。
  期待: 追加フラグ無し（`sftp -- t-proxy`）で踏み台経由の接続が成立する。

- **[P1-5] 新タブ起動（Windows）**
  前提: `wt.exe` あり、`t-key` 選択。手順: アクションメニュー（`o`）から、または `t` 相当の経路で
  新タブ sftp を起動（v1 では auto-fill OFF）。
  期待: 新しい Windows Terminal タブで sftp が開く。タイトルが `sftp: t-key`。

- **[P1-6] アクションメニュー経由**
  手順: `o` → 「SFTP (inline)」を選ぶ。
  期待: P1-1 と同じ起動。

---

## 2. Phase 2 — ガイド付きインライン転送（`o` → 「SFTP transfer…」）

- **[P2-1] ダウンロード（get）**
  手順: フォームで direction=Get、Remote=`/path/hello.txt`、Local=ローカルの保存先 → `Ctrl-S`。
  期待: サスペンド中に **sftp 本来の進捗バー**が見え、完了後 TUI 復帰＋成功トースト。
  ローカルにファイルが出来ている。

- **[P2-2] アップロード（put）**
  手順: direction=Put、Local=`up me.txt`（スペース入り）、Remote=`/upload/` 配下 → `Ctrl-S`。
  期待: スペース入りパスが正しく引用されて転送成功。リモートに `up me.txt` が出来る。

- **[P2-3] パスフレーズ自動入力での転送**
  前提: `t-key-pp`。期待: パスフレーズを聞かれずに転送完了。

- **[P2-4] パスワード自動入力（同意ゲート）** → L3
  前提: `t-pass`、vault unlock、auto-fill ON。手順: まず一度 `Enter` で通常接続し
  パスワード同意（PasswordConfirm）を済ませる → その後 SFTP transfer。
  期待: 同意済みターゲットなのでパスワードが **自動入力**されて転送完了。
  （未同意なら自動入力されず、端末で手入力になる＝安全側。）

- **[P2-5] 不正パスの拒否** → N1/N2
  手順: Local か Remote に `"` を含むパス、または改行を含むパスを入れて `Ctrl-S`。
  期待: 「paths must not contain a double-quote or control character」エラートースト、転送せず。

- **[P2-6] 必須項目バリデーション**
  手順: Local か Remote を空のまま `Ctrl-S`。
  期待: 「both a local and a remote path are required」、フォームは開いたまま。

---

## 3. Phase 3 — デュアルペインブラウザ（`b`）

### 3.1 基本ナビゲーション
- **[P3-1] 起動と初期表示** → L4
  前提: `t-key` 選択。手順: `b`。
  期待: 左=ローカル home、右=リモート。右ペインのタイトルが最初 `~ …`(loading) → 解決後に
  **絶対パス**（例 `/home/you`）に変わり、エントリ一覧＋先頭に `..` 行。

- **[P3-2] ペイン切替とカーソル移動**
  手順: `Tab` で左右フォーカス切替、`j`/`k`（↑/↓）で移動。
  期待: フォーカス枠がアクセント色、選択バーが動く。ディレクトリは先頭側・名前順。

- **[P3-3] ディレクトリ descend / 上へ**
  手順: リモートで `sub/` 上で `Enter`、その後 `Backspace`（または `..` で `Enter`）。
  期待: `sub` の中身が表示 → 親へ戻る。リモートパスは絶対パスで正しく構築される。

- **[P3-4] リフレッシュ**
  手順: リモートでファイルを外部から追加 → `r`。
  期待: 一覧が更新される。ローカルペインで `r` も同様。

### 3.2 転送（ペイン間）
- **[P3-5] リモート→ローカル（Enter on file）**
  手順: リモートペインで `hello.txt` 上で `Enter`。
  期待: ローカル cwd へインライン転送（進捗バー）→ 完了後ローカルペインが自動更新され、
  選択が範囲内に保たれる。 → L5

- **[P3-6] ローカル→リモート（Enter on file）**
  手順: ローカルペインで `up me.txt` 上で `Enter`。
  期待: リモート cwd へアップロード → リモートペインが自動更新。 → L6

- **[P3-7] ControlMaster 再利用（unix）** → L2
  前提: unix。手順: ブラウズで何度かディレクトリ移動してから P3-5 の転送をする。
  期待: 2 回目以降は再認証/再プロンプトが起きない（共有マスター経由で高速）。
  パスワードホストでも、最初の op で確立後は転送で再入力を求められない。

### 3.3 symlink の扱い → F5 / F8 / M1
- **[P3-8] ディレクトリへの symlink を descend（リモート）**
  手順: リモートで `current`（→`sub`）上で `Enter`。
  期待: `current` の中身（=`sub` の中身）が表示される（descend 成功）。

- **[P3-9] ファイルへの symlink の挙動（リモート, 既知の制約）**
  手順: リモートで `latest`（→`hello.txt`）上で `Enter`。
  期待: **descend してしまい単一エントリ表示になる**（`ls -l` がターゲット種別を持たない仕様上の制約）。
  `Backspace` で戻れる。ファイルとして直接 DL したい場合は **転送モーダルでパス指定**するのが回避策。
  → ここが想定どおりの「無害な制約」であることを確認する。

- **[P3-10] ローカルの symlink-to-dir**
  前提: ローカルに `ld -> somedir` を作る。手順: ローカルペインで `ld` 上で `Enter`。
  期待: `metadata` がリンクを辿るので **ディレクトリとして descend** できる（アップロード扱いにならない）。

### 3.4 失敗・エッジ → F3/F9 / L4 / N3 / 完全性指摘
- **[P3-11] 失敗リスティング後も `..` が残る** → F3/F9
  手順: 権限の無いディレクトリ（mode 700 の他人 home 等）へ `Enter` で入ろうとする。
  期待: ステータス行に失敗理由（例 `Permission denied`）が出るが、ペインは空にならず
  **先頭に `..` 行が残り**、`Enter`/`Backspace` で戻れる。

- **[P3-12] 未知ホスト鍵のヒント** → N3
  前提: known_hosts に未登録のホスト。手順: `b` でブラウズ。
  期待: リスティングが失敗し、ステータスに
  「host key not trusted — connect once (Enter/F) to accept it, then browse」。
  → 一度 `Enter` で通常接続し鍵を受理してから再度ブラウズ可能になる。

- **[P3-13] home 未解決時のアップロード拒否**（完全性指摘）
  手順: 接続が遅いホストで `b` 直後、初回リスティング到着前に `Tab`→ローカルファイルで `Enter`。
  期待: 「remote not ready yet — wait for the listing」で転送拒否（ルート `/` へ書かない）。

- **[P3-14] パスワードホストでのブラウズ（BatchMode fail-fast）**
  前提: `t-pass`（鍵/agent 無し）。手順: `b`。
  期待: ハングせず、認証できなければ **即座に失敗**してステータスに理由が出る
  （ブラウズは鍵/agent/ControlMaster 前提。パスワードホストはインラインシェル/転送を使う）。

- **[P3-15] クローズとクリーンアップ** → F1(drop)
  手順: ブラウズ中に `Esc`。
  期待: 即座にホスト一覧へ戻り、UI が固まらない（ControlMaster の `ssh -O exit` は
  バックグラウンドスレッドで実行される）。残存マスターソケットが片付く。

- **[P3-16] ヘルプ到達性** → M4
  手順: ブラウザ画面で `?`。
  期待: SFTP ブラウザ用のヘルプ（Tab/j/k/Enter/Backspace/r/Esc）が表示され、`?`/`Esc` で閉じる。

- **[P3-17] フッターが切れない（80桁端末）** → F10/F11
  手順: 端末を 80 桁ちょうどにして一覧画面とブラウザ画面のフッターを見る。
  期待: フッターの末尾ヒントが切れない（一覧 77 桁 / ブラウザ 79 桁に収まっている）。

---

## 4. ControlPath / temp の堅牢性（unix） → F4 / F7 / L7

- **[P4-1] 深い TMPDIR でのフォールバック**
  手順: `TMPDIR=/very/deeply/nested/...（100バイト超）` を設定して `b`。
  期待: ControlMaster ソケットパスが上限を超える場合は **マスター無し**で個別接続にフォールバックし、
  ブラウズ自体は動く（ハングや bind エラーで止まらない）。

- **[P4-2] バッチ一時ファイル**
  手順: 転送中に `$TMPDIR` を観察。
  期待: `sshm-sftp-op-<pid>-<nonce>.txt` が作られ、完了後に消える。パーミッションは `0600`。
  事前に同名 symlink を仕込んでも `create_new` で **fail-closed**（書き先がすり替わらない）。

---

## 5. Windows 固有

- **[W-1] sftp バイナリ解決**: `[PATH ssh]` 警告が出ないこと（System32 OpenSSH 使用）。
- **[W-2] ローカルパス**: ドライブレター/バックスラッシュのローカルパスで転送できる
  （`C:\Users\me\up me.txt` 等、スペース入りも）。
- **[W-3] リモートは POSIX**: リモートパスは常に `/` 区切りで、ローカルの `\` と混ざらない。
- **[W-4] 新タブ**: `wt.exe` で sftp タブが開く（P1-5）。
- **[W-5] ブラウザ**: Windows には ControlMaster が無いので各 op が個別接続。鍵/agent 認証で動くこと。
- **[W-6] ジャンクション/symlink**: ローカルの junction が descend 可能（P3-10 相当）。

---

## 6. 既存 ssh 機能の回帰確認（Protocol 共通化の影響）

SFTP 対応で connect パイプラインを `Protocol`（Ssh/Sftp）で共通化したため、**既存の ssh 接続が
壊れていないか**を必ず確認する。

- **[R-1] 通常の ssh インライン接続**（`Enter`）が従来どおり動く。
- **[R-2] 新タブ ssh 接続**（`t`）が従来どおり。
- **[R-3] パスフレーズ/パスワード自動入力**が ssh 接続で従来どおり（同意ゲート含む）。
- **[R-4] 多段（ProxyJump）の ssh 接続**が従来どおり。
- **[R-5] 終了トーストの文言**が `ssh ...`（sftp 経路では `sftp ...`）と正しく出る。
- **[R-6] アクションメニュー**: 既存項目（Connect inline / new tab / overrides / Copy ssh command /
  Edit / Delete）が正しい動作にルーティングされる（インデックスずれが無い）。

---

## 付録: 失敗時の切り分け
- ハングする → BatchMode/askpass の経路を疑う。ブラウズはハングしない設計（fail-fast）。
- 文字化け/誤った一覧 → `parse_ls_l` は best-effort。サーバの `ls -l` ロケール/フォーマットを確認。
- 認証ループ/失敗 → vault unlock 状態・auto-fill ON/OFF・同意（PasswordConfirm）状態を確認。
- Windows で挙動が違う → Git/MSYS の `ssh`/`sftp` を拾っていないか（`[PATH ssh]` 警告）。
