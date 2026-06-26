# sshm SFTP 実装計画

## 1. 概要 / ゴール

### スコープ
`sshm` に SFTP 機能を追加し、`~/.ssh/config` に保存済みのホストへ対してファイル転送を行えるようにする。中核思想は既存の SSH 接続パスと同一に保つ:

- **config file is the source of truth**: 保存済みホストへは `sftp -- <alias>` で接続し、OpenSSH 自身が `~/.ssh/config` を読む。ProxyJump / ProxyCommand / IdentityFile / User / Port は OpenSSH 側で自動適用され、`sshm` 側でフラグを再導出しない。
- **既存の認証パイプラインを再利用**: `arm_connect` → `run_*_inline` → `stop_and_join`。`sftp` は `ssh` と同じ `SSH_ASKPASS` プロンプトを発するため、askpass / vault 機構はそのまま動く。
- **新規依存ゼロ**: 外部 OpenSSH `sftp` バイナリを spawn する。SFTP ライブラリ（russh 等）は採用しない。

### v1 の明示的な非ゴール
- デュアルペイン（local | remote）ファイルブラウザ（Phase 2 へ延期）。
- リモートツリーのインタラクティブ閲覧、アプリ内 `ls`/`cd`/`mkdir`/`rename`/`delete`（Phase 2）。
- バイト単位の進捗バー / 速度 / ETA。OpenSSH `sftp` はパイプ越しに progress meter を抑制するため、機械可読な進捗は得られない。
- `russh`/`tokio` などの SFTP プロトコルライブラリの導入。
- new-tab での `SSH_ASKPASS` 環境変数継承（既存の connect_new_tab と同じく未解決スパイクのため、new-tab は v1 で auto-fill オフ）。

### v1 で提供する価値
保存済みホストに対し、正しいフラグ・vault auto-fill 済みパスワード・プロキシ透過で `sftp` REPL を起動する。ユーザーは suspend されたターミナル内で `get`/`put`/`ls`/`mkdir` を直接操作できる。「設定が正しいエイリアスへ即座に sftp する」便利機能。

---

## 2. 推奨アプローチと選定理由

### 推奨: `hybrid-minimal` を Phase 1 として出荷 → 後に `external-sftp` のデュアルペインを上に積む

3 つの judge レンズ（windows-first-fit / architecture-and-deps / ux-and-shippability）すべてで `hybrid-minimal` が最高評価（9 / 8 / 8）、`external-sftp` が中位（6 / 6 / 5）、`rust-sftp-lib` が最低（2 / 2 / 2、anti-recommended）。

#### `rust-sftp-lib`（russh + tokio）を却下する理由
- **依存ツリーの爆発**: `russh` + `russh-sftp` + `tokio` + crypto backend を、async ランタイムを一切持たない小さな headless プロジェクトへ投入する。ビルド時間・バイナリサイズ・監査面・MSRV 1.94 の再検証コストが bump ごとに増える。
- **「config is source of truth」原則への直接違反**: russh は `~/.ssh/config` を読まない。user/port/identity/ProxyJump を `ssh -G` から自前再実装することになり、OpenSSH との恒久的な乖離税が発生する。
- **ProxyCommand をネイティブに扱えない**: 結局 OpenSSH `sftp` へのシェルアウト（= `external-sftp` のコスト全体）を**第二のエンジンとして併走**させる羽目になる。すべてのコストを払った上に russh ツリーが乗る。
- **新たなセキュリティ責務**: host-key TOFU、known_hosts 書き換え、cert/agent/FIDO-sk 鍵の扱いを自前で持つ。ssh-agent / ハードウェア鍵ユーザーは `ssh <alias>` と挙動が乖離して壊れる可能性が高い。
- Windows では aws-lc-rs が C/cmake ツールチェーンを要求（ring backend で回避は可能だが MSRV 検証が未済）。

#### `external-sftp`（永続 `sftp -b -` セッション + `parse_ls_l`）を Phase 1 にしない理由
最高の UX 天井（実デュアルペインブラウザ）だが、最も bug-prone な機構を v1 に front-load する:
- `parse_ls_l` は OpenSSH の人間向け `ls -l` 出力を screen-scrape する。機械可読モードが無く、locale / カラム / symlink suffix / 日付フォーマットに依存して本質的に best-effort。
- sentinel echo で永続インタラクティブ子プロセスの可変長応答をフレーミングするのは「最もバグの出る部分」かつ OpenSSH バージョン間で「文書化された契約ではない」。Windows の ConPTY / コンソールパイプのバッファリング差異は CI カバレッジが無い。
- 1 つの cut で stateful worker protocol + parse_ls_l + デュアルペイン UI + 複数の新 Screen が着地し、プロジェクトの「incremental / surgical」価値に反する。

→ `external-sftp` は**正しい最終目的地**だが、**最初の PR としては大きく・プラットフォームリスクが高すぎる**。Phase 1 の共有基盤（`SshTools.sftp`、`build_sftp_args`）の上に additive に積む。

#### `hybrid-minimal` を選ぶ理由
新規依存ゼロ・新 Screen ゼロ・新 ui モジュールゼロ・async ゼロ・出力パースゼロ。`run_ssh_inline` と構造的に同一の、既にプロジェクトが信頼しているパスをほぼそのまま使う。REPL が suspend されたターミナル内で動くため、(a) vault ロック時に `sftp -b` が prompt でハングする問題、(b) 機械可読な進捗が無い問題、の両方を回避する（ユーザーが自分で prompt に応答する）。

#### 共通の必須リファクタ
`run_*_inline` と `connect_new_tab` を**プログラムパス（`PathBuf`）でパラメタライズ**し、`ssh` と `sftp` が 1 つのコードパス（`--` sentinel、`escape_wt_arg`、`describe_exit_code`）を共有する。これにより ssh/sftp の引数・new-tab・終了コード処理が drift しない。

### フェーズ概要
- **Phase 1（MVP, 出荷）**: インタラクティブ `sftp` 起動（inline + new-tab）。新 Screen なし。
- **Phase 2（任意, bolt-on）**: バッチ転送モーダル（`sftp -b`、liveness パターンの worker）。
- **Phase 3（差別化, 任意）**: `external-sftp` デュアルペインブラウザ。`parse_ls_l` / `build_sftp_args` を先に pure・cross-platform でユニットテストしてから stateful worker を配線。

---

## 3. アーキテクチャ設計

一方向依存規則を厳守: `os/` と `config/` は ratatui 非依存、`ui/` は純レンダリングでドメイン状態を変更しない、ドメイン変更は `update.rs` / `app.rs` のみ。

### Phase 1: 変更点（最小）

#### `src/os/binaries.rs`
`SshTools` に `sftp` フィールドを追加し、両 `resolve()` arm で populate する。`sftp` は全プラットフォームで使うため dead_code トラップに掛からない。

```rust
pub struct SshTools {
    pub ssh: PathBuf,
    pub ssh_keygen: PathBuf,
    pub ssh_keyscan: PathBuf,
    pub sftp: PathBuf,      // 新規
    pub is_system32: bool,
}
```

- Windows arm: System32 OpenSSH を優先（`ssh.exe` が存在する分岐）で `sftp: base.join("sftp.exe")`。フォールバック arm で `sftp: PathBuf::from("sftp")`（`is_system32: false` が既存の `[PATH ssh]` 警告を駆動）。
- non-windows arm: `sftp: PathBuf::from("sftp")`。

#### `src/os/connect.rs`（共有パスにパラメタライズ）
```rust
// build_ssh_args と対をなす sftp フラグセット。-P(大文字)でポート、-i/-J/-o を
// サポート、-L/-R/-D/-v は非対応。-- sentinel + [user@]alias。
pub fn build_sftp_args(host: &HostView, ov: &ConnectOverrides) -> Vec<String>;

// run_ssh_inline を一般化: プログラムパスを引数に取り、ssh/sftp で共有。
pub fn run_inline(
    program: &Path,
    args: &[String],
    env: &[(OsString, OsString)],
) -> io::Result<std::process::ExitStatus>;
// 既存 run_ssh_inline は run_inline(&tools().ssh, ...) の薄いラッパに。

// connect_new_tab をプログラムパスでパラメタライズ（両 cfg arm）。
// 既存呼び出しは program = &tools().ssh で維持。
pub fn connect_new_tab_with(
    program: &Path,
    title: &str,
    args: &[String],
    env: &[(OsString, OsString)],
) -> io::Result<()>;
```

`describe_exit_code` / `escape_wt_arg` / `command_line` は再利用（sftp 用に `command_line` のプレフィックスのみ可変にする小改修は任意）。

#### `src/os/sftp.rs`（新規, ratatui 非依存）
Phase 1 は薄い。`build_sftp_args` をここに置くか `connect.rs` に置くかは任意（クロスプラットフォームテストの所在を統一するため `connect.rs` 推奨）。Phase 1 の本体は薄いオーケストレーション補助のみで、実体は `connect.rs` の共有関数。`src/os/mod.rs` に `pub mod sftp;` を登録。

#### `src/update.rs`
- `handle_list` にエントリポイントキーバインドを追加（例 `KeyCode::Char('F')`）。選択中ホストを解決し、SFTP 接続パスへ。
- `arm_and_connect_inline` を `sftp` でも使えるよう、`run_inline(&tools().sftp, args, &env)` を渡せる形に小汎用化（program パスを引数化、または sftp 版の薄いコピー `arm_and_connect_sftp_inline`）。
- 認証/プロキシ/exit toast は既存のロジックをそのまま流用。

#### `src/ui/mod.rs`
- `draw_footer` の list arm に `F  sftp` ヒントを追加。
- `confirm::draw_action_menu` の per-host ActionMenu に `sftp` 項目を追加。
- **新 Screen・新 ui モジュールは不要**。

### Phase 2: バッチ転送モーダル（任意, cuttable）

新 Screen + os worker + per-tick drain を導入する唯一の部分。

- `app.rs`: `Screen` enum に `SftpTransfer`（進捗オーバーレイ）と `SftpBatch`（パス入力フォームオーバーレイ）を追加。`App` に `sftp_transfers: Vec<SftpTransfer>`（`probes: Vec<LivenessProbe>` を鏡像）+ 入力バッファ/カーソルフィールド。`App::new` で初期化。`App::drain_sftp()` を追加し `on_tick`/event loop から呼ぶ。
- `update.rs`: `handle_key` に `Screen::SftpTransfer` / `Screen::SftpBatch` の routing arm。`handle_sftp_transfer` / `handle_sftp_batch`。
- `ui/mod.rs`: `pub mod sftp;` 登録、`draw` 第二 match にオーバーレイ arm、`base_screen` で両者を `prev_screen` に解決（host List をオーバーレイ）、`draw_footer` に専用 arm。
- `src/ui/sftp.rs`: `draw_transfer`（`modal_block` + `kv_line` 統計行 + ハンド描画の進捗バー）、`draw_batch_form`（`modal_block` + `input_line`）。色はすべて `theme.rs` 経由。

### Phase 3: デュアルペインブラウザ（差別化, 任意）

`external-sftp` 設計を採用。3 タッチポイント規則に従い:
- `app.rs`: `Screen::SftpBrowser { host: usize }`（base）+ `SftpMkdir` / `SftpRename`（入力オーバーレイ）+ `SftpTransfer`（進捗）。`SftpPane` focus enum（`ListFocus` を鏡像）。`App` に `sftp_focus`、`sftp_local_state`/`sftp_remote_state: ListState`、`sftp_local_cwd: PathBuf`、`sftp_remote_cwd: String`、`sftp_local_entries`/`sftp_remote_entries`、`sftp_session: Option<SftpSession>`。
- `os/sftp.rs`: 永続 worker、`parse_ls_l`、`SftpSession::spawn/send/drain`。
- `ui/sftp.rs`: `responsive_split` でデュアルペイン、`panel(name, focused)`、`theme::selection()` + `SELECT_SYMBOL`。
- 上書き確認は `ConfirmAction` に新バリアントを追加し `handle_confirm` で処理。

---

## 4. 並行処理

### Phase 1: 並行処理なし（設計上ブロッキング）
インタラクティブ起動は完全同期: `suspend_tui` → `run_inline(&tools().sftp, ...)`（stdio 継承）→ wait → `restore_tui`。`run_ssh_inline` と全く同じ。TUI は suspend 中に描画しないため UI スレッド懸念は無い。

### Phase 2 / 3: liveness パターン（mpsc worker + per-tick drain）
ブロッキング SFTP I/O を UI スレッドから外す方法は `os/liveness.rs` を鏡像する。worker thread が blocking `sftp` 子プロセスを所有し、結果を mpsc::Sender へ push。`App` が `try_recv` ループで tick ごとに drain（`event_loop.rs` の `app.drain_liveness()` 呼び出しの隣に配線）。

#### Phase 2（バッチ転送、自己完結ジョブ）
```rust
pub enum Direction { Put, Get }

pub struct SftpJob {
    pub host_idx: usize,
    pub direction: Direction,
    pub local: PathBuf,
    pub remote: String,
}

pub enum SftpProgress {
    Started,
    Line(String),                       // sftp の stdout 行（粗い進捗）
    Done(std::process::ExitStatus),
    Failed(String),
}

pub struct SftpTransfer { /* rx: Receiver<SftpProgress>, child: Child, _handle: JoinHandle<()> */ }

impl SftpTransfer {
    // batch を temp file に書き、`sftp -b <file> -- <alias>` を spawn、stdout を
    // mpsc で report、終了時に temp を掃除。
    pub fn spawn(args: Vec<String>, env: Vec<(OsString, OsString)>, batch: String) -> io::Result<Self>;
    // 非ブロッキング try_recv ループ。bool=true で channel disconnected（worker 完了）。
    pub fn drain(&self) -> (Vec<SftpProgress>, bool);
    pub fn cancel(&mut self);            // Child を kill（JoinHandle のみの liveness と異なる差分）
}
```
結果はジョブ自己完結（モーダルがジョブを所有）なので、liveness の host-index シフト問題は回避される。`Done`/`Failed` で transfer は drop（channel disconnect）。

#### Phase 3（永続ブラウザセッション）
```rust
pub enum SftpCmd {
    ListDir, Cd(String), Mkdir(String),
    Rename { from: String, to: String }, Rm(String),
    Get { remote: String, local: PathBuf }, Put { local: PathBuf, remote: String },
    Cancel, Quit,
}

pub enum SftpEvent {
    Connected { cwd: String },
    Listing { cwd: String, entries: Vec<RemoteEntry> },
    Progress { name: String, done: u64, total: Option<u64> },
    TransferDone { name: String },
    OpError { op: &'static str, msg: String },
    Disconnected { code: Option<i32> },
}

pub struct RemoteEntry { pub name: String, pub is_dir: bool, pub is_link: bool, pub size: u64, pub raw_mode: String }

pub struct SftpSession { /* events: Receiver<SftpEvent>, cmd_tx: Sender<SftpCmd>, _handle: JoinHandle<()> */ }

impl SftpSession {
    pub fn spawn(args: Vec<String>, env: Vec<(OsString, OsString)>, initial_cwd: Option<String>) -> io::Result<Self>;
    pub fn send(&self, cmd: SftpCmd);                  // 非ブロッキング
    pub fn drain(&self) -> (Vec<SftpEvent>, bool);     // LivenessProbe::drain を鏡像
}

fn parse_ls_l(block: &str, cwd: &str) -> Vec<RemoteEntry>;  // pure, unit-tested
```
liveness が host index でキーするのに対し、Phase 3 セッションは単一ホストでブラウザが開いている間だけ生きるので stale-index 問題は限定的。`rebuild_hosts()` でセッションを閉じる。ハングした子を OpError に変換する read timeout を持つ。

`App::drain_sftp()` は `drain_liveness`（app.rs:880）を鏡像し、`event_loop.rs` で `drain_liveness()` の隣に配線。`draw()` は決してブロックしない。

---

## 5. 認証

接続パスを**そのまま再利用**する。`sftp` は `ssh` と同一の `SSH_ASKPASS` プロンプト（`Enter passphrase for key '<path>': ` と `<user>@<host>'s password: `）を発するため、`askpass.rs` の `classify()`/`decide()`/`arm_connect` は無改修で動く。認証は `SSH_ASKPASS` / `SSH_ASKPASS_REQUIRE=force` env 経由でのみ渡る（secret は env に入らない）。

### Phase 1 のフロー（`arm_and_connect_inline` を鏡像）
1. `handle_list` で `F` 押下時に選択中 `HostView` を解決。
2. **vault は unlock-first（unlock-on-demand API は無い、update.rs:372-375 の v1 制約）**。
   - `app.vault.is_none()` の場合、Phase 1 では inline REPL なので **plain 起動**でよい: `run_inline(&tools().sftp, &args, &[])`。suspend されたターミナル内で sftp が自分で prompt するため問題ない。（バッチパス Phase 2 は非インタラクティブなので vault unlock + armed secret を必須にする、後述。）
3. vault unlock 済みなら:
   - `app.vault_secret_kinds(&host)`（app.rs:640）で candidacy を計算。
   - `gather_secrets(app, &host, kinds)`（update.rs:746）で Password / Passphrase の `Secret` を取得（host.patterns を走査、glob/negation をスキップ、`vault.secrets_for_host(pat)` で exact-match）。
   - secret が無ければ plain 接続。
   - `resolved_identity(rc, alias, &os_tokens())`（askpass.rs:283）を `ssh -G -- <alias>` 解決から構築。
   - `arm_connect(identity, password, passphrase)`（askpass.rs:760）で listener を bind + env bundle 取得。
   - `app.record_connect(host.alias())` → `suspend_tui` → `run_inline(&tools().sftp, &args, &env)` → `restore_tui` → `app.last_activity = Instant::now()` → `listener.stop_and_join()` で secret を zeroize + outcome toast。
4. パスワード auto-fill は server-facing なので、`Screen::PasswordConfirm` / `confirmed_password_targets` の consent gate を connect と同様に通す（passphrase は connect と同じく無確認 auto-fill）。

### ロック時の unlock フロー
v1 制約上、ロック中の vault を silent に unlock しない。Phase 1 inline では plain 起動で逃げる。Phase 2/3（非インタラクティブ / セッション）で auto-fill が必須の場合は、既存の `open_vault`（update.rs:1732）→ `Screen::VaultUnlock` オーバーレイへルーティングし、`handle_vault_unlock`/`submit_vault_unlock`（update.rs:1888）で unlock 後に SFTP を開く。`os/sftp.rs` は**既に取得済みの `Secret` を受け取るのみ**で、vault や App には決して触れない（一方向依存維持）。

---

## 6. Proxied ホスト（ProxyJump / ProxyCommand）

**特別扱い一切なし。** `sftp -- <alias>` を起動し、OpenSSH が `sshm` の書いた `~/.ssh/config` を読むため、ProxyJump / ProxyCommand / IdentityFile / User / Port が自動適用される。`ssh <alias>` connect パスと完全に同一。

- `HostView::is_proxied()`（model.rs:195）は SFTP では参照しない（liveness の直接 TCP probe をスキップするためだけのもの）。
- ad-hoc `ConnectOverrides` の場合のみ `build_sftp_args` が `-J` を `build_ssh_args` 同様に転送する。保存済みホストで ProxyCommand を extras から再導出することはしない。
- proxied ホストは追加ホップ分だけ接続が遅いだけ。Phase 3 の worker は接続タイムアウトで「ProxyCommand 自体が TTY を要求してハング」を `OpError`/`Disconnected` に変換し、worker を wedge させない。

これが `external-sftp` を選び `rust-sftp-lib` を却下する決定的理由の一つ: russh は ProxyCommand をネイティブに扱えず、結局シェルアウトを併走させる必要がある。

---

## 7. Windows-first 注意点

### バイナリ解決
`SshTools.sftp` を両 `resolve()` arm で populate。Windows は `base.join("sftp.exe")` を System32\OpenSSH 優先（Git/MSYS の `sftp` は config/`-J` を異なって解釈する既知ハザード）、System32 OpenSSH 不在時は bare `"sftp"` フォールバック（`is_system32: false` が `[PATH ssh]` 警告を駆動）。`sftp` フィールドは全 OS で使うので dead_code トラップに掛からない。

### パス処理
- **リモートパスは常に POSIX**（`/` 区切り）。`sftp_remote_cwd` は `String` で保持し `/` で join、`std::path` を通さない。
- **ローカルパスは native**（Windows は backslash / ドライブレター）。`PathBuf` で保持。
- 2 つのペインパス型を決して交差させない（remote name を local PathBuf へ join しない、逆も同様）。これは頻出 Windows バグ。
- ローカル home の seed は `dirs::home_dir()`（`$HOME` は MSYS で誤る、`binaries.rs:68` の `ssh_dir()` と一貫）。
- sftp REPL/batch の引数でスペースを含むパスは二重引用符で囲む。sftp REPL に backslash エスケープは無い（ssh_config の no-escape 規則と同型）ので、リテラル `"` を含む名前は config writer と同じく reject する（破損させない）。

### wt.exe / new-tab
`connect_new_tab` は現状 `tools().ssh` をハードコード（connect.rs:207）。`connect_new_tab_with(program, ...)` へパラメタライズし、`sftp` を wt.exe タブで起動できるようにする。各引数は `escape_wt_arg`（backslash 維持、whitespace で quote）を通す — Windows sftp パスに正しい。v1 では new-tab は `&[]` env（auto-fill オフ、env 継承スパイク未解決）。

### cfg-gating と clippy dead_code トラップ
CLAUDE.md が警告する罠: `#[cfg(windows)]` からのみ参照されるヘルパー（new-tab sftp launcher、`find_wt` 再利用など）は、Linux CI の `clippy -D warnings` で dead_code / unused_imports を踏む（Windows-only ローカル clippy では見えない）。

- `connect_sftp_new_tab` / sftp new-tab launcher は `#[cfg(windows)]`、テストも回すなら `#[cfg(any(windows, test))]`（`escape_wt_arg`/`find_wt` と全く同様）でゲート。
- `build_sftp_args` と（Phase 3）`parse_ls_l` は**クロスプラットフォームのユニットテスト**で必ず exercise し、どちらの OS でも dead と flag されないようにする。
- 両 CI clippy ターゲット（Linux + Windows）を確認。

### キーイベント
変更不要。ブラウザハンドラも既存 event_loop の `KeyEventKind::Press` フィルタを通るので Windows のキーアップ重複は既に処理済み。

### アトミックなローカル書き込み（Phase 2/3 の Get）
temp へ書いて rename する場合、`writer.rs` の delete-then-rename パターンを再利用（Windows は dest 存在時 rename 失敗 → 先に削除）。bare `std::fs::rename` は使わない。perms 0o600 は unix のみ。cancel/error で半書きファイルを残さない。

---

## 8. 段階的実装計画

各マイルストーンは独立して出荷可能。各段階で CI ゲートを通す:
```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all
```

### Phase 1 — インタラクティブ sftp 起動（MVP, 出荷）

**M1.1: 共有 spawn パスのパラメタライズ**
- `os/binaries.rs`: `SshTools` に `sftp` フィールド追加、両 arm で populate。
- `os/connect.rs`: `run_inline(program, args, env)` を導入し `run_ssh_inline` をそのラッパに。`connect_new_tab_with(program, title, args, env)` を導入し既存 `connect_new_tab` をラッパに。
- テスト（`os/connect.rs` の `#[cfg(test)] mod tests`）: `run_ssh_inline` 既存テストが緑のままを確認。

**M1.2: `build_sftp_args`**
- `os/connect.rs`: `build_sftp_args(host, ov)` 実装。`-i`(+`-o IdentitiesOnly=yes`) / `-P`（大文字）/ `-J` / `-o k=v`、`-L`/`-R`/`-D`/`-v` なし、`--` sentinel + `[user@]alias`。
- テスト（クロスプラットフォーム, inline `#[cfg(test)]`）:
  - デフォルト override → `["--", alias]` のみ。
  - port override → `-P`（小文字 `-p` でないこと）。
  - identity → `-i <path> -o IdentitiesOnly=yes`。
  - proxy_jump → `-J`。
  - forwards/verbose が**含まれない**こと。
  - `-` で始まる alias が sentinel 後に来ること（CWE-88 回帰）。

**M1.3: 起動の配線**
- `update.rs`: `handle_list` に `KeyCode::Char('F')`。選択中ホスト解決 → SFTP inline 接続。
- `arm_and_connect_inline` を program パラメタ化（または `arm_and_connect_sftp_inline` を作り `run_inline(&tools().sftp, ...)` を使用）。vault 未 unlock 時は `run_inline(&tools().sftp, &args, &[])` で plain 起動。
- `ui/mod.rs`: list footer に `F  sftp`、`confirm::draw_action_menu` に sftp 項目。
- new-tab sftp（任意）: `connect_new_tab_with(&tools().sftp, ...)`、`&[]` env。
- 手動テスト: `scratch-config` スキルでスクラッチ config を作り、`cargo run -- --config <scratch>` で `F` 起動を検証。

**Phase 1 完了基準**: 保存済みホストで `F` → suspend されたターミナルで sftp REPL が開き、vault unlock 済みならパスワードが auto-fill され、proxied ホストも透過で動く。CI 3 ゲート緑。

### Phase 2 — バッチ転送モーダル（任意, cuttable）

**M2.1: `SftpTransfer` worker（`os/sftp.rs`）**
- `SftpJob` / `Direction` / `SftpProgress` / `SftpTransfer::spawn/drain/cancel` を liveness パターンで実装。batch を temp file へ、`sftp -b <file> -- <alias>` spawn、stdout を mpsc。`cancel` 用に `Child` を保持。
- temp ファイル掃除を scope guard で（cancel/panic でも）。Windows rename quirk + unix 0o600。
- テスト（inline）: batch スクリプト生成（引数の quoting）、drain の disconnect フラグ。

**M2.2: Screen + UI**
- `app.rs`: `Screen::SftpTransfer` / `Screen::SftpBatch`、`App` に `sftp_transfers`・入力フィールド、`App::new` 初期化、`App::drain_sftp()`、`event_loop.rs` で per-tick drain 配線。
- `update.rs`: routing arm + `handle_sftp_transfer` / `handle_sftp_batch`。`SftpBatch` の入力は `insert_char`/`backspace`/`next_boundary` 再利用。vault ロック時は `open_vault` へルーティング（非インタラクティブなので unlock 必須）。パスワード auto-fill は `PasswordConfirm` consent gate を通す。
- `ui/mod.rs`: `pub mod sftp;`、オーバーレイ arm、`base_screen` で `prev_screen` 解決、footer arm。
- `ui/sftp.rs`: `draw_transfer`（`centered` + `Clear` + `modal_block` + `kv_line` + ハンド描画進捗バー、`theme` 経由）、`draw_batch_form`。進捗は v1 では spinner / 最終 exit toast（機械可読バイト進捗は無い、正直に）。

**Phase 2 完了基準**: モーダルから put/get の単発転送が UI をブロックせず実行され、進捗 spinner と完了 toast が出る。CI 緑。

### Phase 3 — デュアルペインブラウザ（差別化, 任意）

**M3.1: pure パース層を先に**
- `os/sftp.rs`: `parse_ls_l(block, cwd)` を pure 関数として実装し、**クロスプラットフォームで徹底ユニットテスト**（config 層のテスト規律に倣う）: 通常ファイル / dir / symlink `-> target` suffix / `.`・`..` フィルタ / 異常カラム耐性。stateful worker 配線前に緑にする。

**M3.2: 永続セッション worker**
- `SftpSession::spawn/send/drain`、sentinel フレーミング、bounded read + timeout で OpError 変換。`rebuild_hosts()` でセッションクローズ。

**M3.3: デュアルペイン Screen + UI**
- `app.rs`: `Screen::SftpBrowser { host }` + `SftpMkdir`/`SftpRename`/`SftpTransfer`、`SftpPane` focus enum、ペイン状態フィールド、`App::new` 初期化。上書き確認は `ConfirmAction` 新バリアント。
- `update.rs`: `handle_sftp`（Tab / j・k / Enter / Backspace / g・p / m / r / d / Esc）、サブモーダルハンドラ、`handle_confirm` に上書き分岐。
- `ui/sftp.rs`: `responsive_split` でデュアルペイン、`panel(name, focused)`、`theme::selection()` + `SELECT_SYMBOL`、`draw_mkdir`/`draw_rename`（`list::draw_jump_picker` を雛形に）。
- `ui/mod.rs`: 3 タッチポイント完備（モジュール登録、body arm、overlay arm、`base_screen`、`draw_title`、`draw_footer`）。

**Phase 3 完了基準**: ブラウザでリモートツリー閲覧・mkdir/rename/delete・get/put が UI ブロックなしに動く。`parse_ls_l` が異常サーバーで graceful degrade。CI 緑。

---

## 9. リスクと未解決事項

### Phase 1（低リスク）
- **new-tab の `SSH_ASKPASS` env 継承**: `connect_new_tab` と同じ未解決スパイク。wt.exe がタブの子へ env を継承するか不明。v1 では new-tab sftp は `&[]` env（auto-fill オフ）で逃げる。確認後に有効化。
- **ssh/sftp コードパス drift**: `run_*_inline`/`connect_new_tab` の program パラメタ化で 1 パス共有。複製しないこと（drift 防止）。
- **vault ロック時の体験**: Phase 1 inline では plain 起動（ユーザーが prompt 入力）で許容。バッチ/セッションでは unlock 必須。

### Phase 2
- **`sftp -b` は prompt でハングする**: 非インタラクティブなので、vault ロック / auto-fill 未 arm 時に認証 prompt でハングする。バッチモーダルは vault unlock + armed secret を前提にするか、key-auth ホストに限定。これにより connect の認証機構（`gather_secrets` / `resolved_identity` / `PasswordConfirm`）の大半をモーダルに引き込むため「最小」の主張は誇張気味 → v1 で force-ship しない。
- **機械可読な進捗が無い**: `sftp -b` はパイプ越しに progress meter を抑制。進捗バーは spinner / 最終 exit toast に留まる。`Transferred:` サマリ行をパースすれば粗い進捗は可能だが本質的に脆い。
- **cancel の半端ファイル**: put 中の cancel は子プロセス kill が必要（`Child` 保持、liveness の JoinHandle-only と差分）。リモートに partial ファイルが残る可能性を warn。

### Phase 3（高リスク、慎重に）
- **`parse_ls_l` の脆弱性**: OpenSSH に機械可読 SFTP 属性モードが無く、locale / カラム / 日付フォーマット / symlink suffix が変動。本質的に best-effort。異常サーバーで pane が誤描画。先に pure ユニットテスト + graceful degrade を担保。
- **sentinel フレーミングの desync**: 永続インタラクティブ子の可変長応答をフレーミングするのは最もバグの出る部分かつ文書化された契約ではない。バナー / マルチライン prompt / エラー行で parser が desync しうる。bounded read + resync-on-sentinel が必要。
- **Windows のライブ sftp 子に CI カバレッジ無し**: ConPTY / コンソールパイプのバッファリング差異は本プロジェクトが過去に踏んだ類のプラットフォーム乖離。Windows での手動検証が必須。
- **認証 re-prompt エッジ**: askpass は kind/path 単位で single-shot。長いブラウジングセッションでの re-key や二重 password 要求が armed secret を枯渇させハングしうる。op タイムアウトで OpError 化。
- **古い OpenSSH クライアント**: `sftp -b -` のパイプ読みは <8.x で挙動が異なる。password auto-fill は connect が gate する `ssh_kbdint_prefix_supported()` と同じ gate が必要かもしれない。
- **stale host-index keying**: liveness 同様、ブラウザ開放中に config 編集（`rebuild_hosts` がマップをクリア）でセッションの host index が stale 化。`rebuild_hosts` でセッションを閉じる。

### 全フェーズ共通
- **REPL の no-escape**: リテラル `"` を含む名前は config writer と同じく reject（破損より拒否）。
- **MSRV**: 新規依存ゼロ方針を維持する限り MSRV 1.94 リスクは無い（`rust-sftp-lib` 却下の主因の一つ）。
