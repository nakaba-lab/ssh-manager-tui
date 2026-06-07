# sshm UI ビジュアルリデザイン 設計

- 日付: 2026-06-07
- 対象: `sshm`(ratatui TUI)の見た目の洗練・現代化
- スコープ: 純粋に視覚面のみ。機能・キーバインド・画面構成・レイアウト比率は変更しない。

## 1. 目的とスコープ

### 目的

現状の UI は機能的に完成しているが、配色・記号がコード全体にハードコードされ、画面ごとに統一感が弱い。これを **Tokyo Night** を基調とした「設計された」配色に統一し、枠・余白・選択・フッターなどのクロームを現代的に洗練させる。

### 決定済みの前提

- **配色**: 設計された RGB(truecolor)パレット。基調は **Tokyo Night**。
- **背景**: 端末の背景色は塗りつぶさない(前景色・枠線・選択バー背景のみ制御)。ライト端末でも破綻させない方針。
- **アイコン/記号**: 標準 Unicode のみ。Nerd Font・絵文字は使わない(既存の `🔒` 絵文字は廃止する)。
- liveness 記号 `● ○ … · —` は既に標準 Unicode なので維持し、色だけパレットへ。

### 非対象(やらないこと)

- 機能追加・削除、キーバインド変更、画面遷移の変更。
- レイアウトの分割比率変更(一覧 58/42、キー 45/55 等は維持)。
- `config/`・`os/` レイヤーへの変更(`ui/` 配下 + 新規 `theme.rs` のみ)。
- 端末背景の塗りつぶし、テーマ切り替え UI(将来拡張の余地は残すが今回は実装しない)。

## 2. アーキテクチャ

### 新規モジュール `ui/theme.rs`

色のハードコードを廃し、セマンティックな役割名に集約する。CLAUDE.md の「`ui/` は純粋なレンダリングのみ・ドメイン状態を変更しない」方針に沿い、`theme.rs` は色定数と描画ヘルパのみを提供する純粋モジュールとする。

役割名 → Tokyo Night の RGB:

| 役割 | hex | 用途 |
|---|---|---|
| `text` | `#c0caf5` | 主要テキスト |
| `dim` | `#565f89` | ラベル・副次情報 |
| `faint` | `#414868` | プレースホルダ・非アクティブ・微弱な区切り |
| `accent` | `#7aa2f7` | フォーカス・主アクション・アプリ名・選択バー |
| `accent2` | `#bb9af7` | 補助アクセント(任意) |
| `sel_bg` | `#283457` | 選択行の背景 |
| `border` | `#3b4261` | 通常の枠線 |
| `up` | `#9ece6a` | liveness up・成功 |
| `down` | `#f7768e` | liveness down・エラー・破壊的操作 |
| `warn` | `#e0af68` | 警告(`[PATH ssh]` 等) |
| `checking` | `#7dcfff` | liveness checking |

実装方針:
- 各役割を `Color`(`Color::Rgb`)を返す `const`/関数、もしくは `Style` を返すヘルパとして公開する。
- 既存の全 UI ファイル(`ui/mod.rs`・`list.rs`・`edit.rs`・`keys.rs`・`known_hosts.rs`・`confirm.rs`・`help.rs`・`widgets.rs`)の色参照を `theme` 経由へ置換する。
- フォーカス枠ヘルパ(通常=`border` / フォーカス=`accent`)など、繰り返し使うスタイルは `theme.rs` か `widgets.rs` に共通関数として置く。

### 既存の依存方向

`config/` ← `os/` ← `ui/`/`update.rs` の一方向依存は不変。`theme.rs` は `ui/` 内に閉じる。

## 3. 共通クローム

全画面に共通して効く部分。`ui/mod.rs` と `widgets.rs` が中心。

### タイトルバー(`draw_title`)

- 反転バッジ(黒地シアンの ` SSH Manager `)をやめ、ブレッドクラム風に。
  - `sshm`(accent・太字) + `›`(faint) + 画面名(text・太字) + カウント(faint)。
  - 例: `sshm  ›  Hosts  3/12`
- `[PATH ssh]` 警告は `warn` 色。

### 枠線

- 全パネルを `BorderType::Rounded` に統一。
- 枠色: 非フォーカス=`border`、フォーカス=`accent`。
- パネル内側に左右 1 セルの padding を入れて窮屈さを解消。

### 選択行

- 現状の「青ベタ背景 + 白文字 + 太字」をやめる。
- **控えめな `sel_bg` 背景 + 左端に `accent` の縦バー `▎` + 文字色は通常(`text`)維持**。
- リスト/テーブル/メニューの選択表現をこの方式で統一する。
- 実装メカニズム(`highlight_symbol` を使うか、行頭に accent span を差し込むか)は実装時に決める。意図は「控えめ背景 + accent の左マーカー」。

### モーダル

- `BorderType::Double` をやめ、**rounded + `accent` 枠**に統一。タイトルは accent・太字。
- 破壊的操作(ホスト削除・キー削除・known_host 削除・変更破棄)のモーダルだけ枠を `down`(赤)にして危険を示す。

### フッターヒント(`widgets::footer_hints`)

- 全キーが黒地シアンのバッジで騒がしいので、フラット化する。
  - `key`=`accent`、`label`=`dim`、ペア区切り=`faint` の `·`。
  - 例: `j/k move · / search · ↵ connect`

### トースト(`draw_toast`)

- 成功: `✓ <msg>` を `up` 色文字 + 控えめ背景(`sel_bg`)。自動消滅は現状維持。
- 失敗: `✗ <msg>` を `down` 背景 + 太字で視認性を維持。sticky(次キーで消える)は現状維持。

## 4. 画面別の仕上げ

### ホスト一覧(`ui/list.rs`)

- テーブルヘッダ行を `dim`。
- liveness 記号 `● ○ … · —` をパレット色へ(up=`up`・down=`down`・checking=`checking`・skipped/unknown=`faint`)。
- 行選択は共通クロームの「控えめ背景 + accent バー」方式。
- 詳細ペイン: kv ラベル=`dim`、値=`text`、status 行は状態色(up=`up`・down=`down` 等)。
- 空状態・ProxyJump ピッカーの強調キーは `accent`。

### 編集フォーム(`ui/edit.rs`)

- フォーカス中ラベル=`accent`・太字 + `▸`、非フォーカス=`dim`。
- 複数値行の選択マーカー(`›`)は `accent`。
- バリデーションエラーは `down` の `⚠`。

### キー管理(`ui/keys.rs`)

- `🔒` 絵文字を廃止(Unicode-only 方針)。
  - 秘密鍵あり= `accent` の `●`、公開のみ= `faint` の `○` で表現。
- `type`/`bits`/`fingerprint` などの副次情報は `dim`。
- 生成ウィザードのラジオ `(•)/( )` は選択中を `accent`。
- ピッカーも共通モーダル方針に統一。

### known_hosts(`ui/known_hosts.rs`)

- リスト選択・枠・検索プレースホルダを共通方針に合わせる(検索は alias 系と異なり部分一致のまま。挙動は変えない)。

### 確認 / アクションメニュー / ヘルプ(`ui/confirm.rs`・`ui/help.rs`)

- モーダル枠・選択表現を共通方針に統一。
- ヘルプのセクション見出しは `accent`、キー名は `text`・太字、説明は `dim`。

## 5. エラーハンドリング・後方互換

- truecolor は Windows Terminal で問題なし。truecolor 非対応端末では `Color::Rgb` が近似 16 色へ落ちる(ratatui/端末側の標準挙動)ため、破綻はしない。
- 背景を塗らないため、ライト/ダーク端末いずれでも前景色が読める範囲に保つ(Tokyo Night の前景はダーク端末前提だが、背景非塗りで最低限の可読性は確保)。

## 6. テスト・検証

- 既存のユニットテストは `config/`・`os/` 層が中心で、色・描画は検証していないため、本変更による失敗は想定しない。
- 検証は手動が中心:
  - `cargo build` / `cargo clippy -D warnings`(Linux/Windows 両方で dead_code に注意)/ `cargo fmt`。
  - `cargo run -- --config <throwaway>` で各画面(一覧・詳細・編集・キー・known_hosts・各モーダル・トースト)を目視確認。
- 可能なら theme の役割→色マッピングに対する軽いユニットテスト(回帰防止)を `theme.rs` に追加してもよい(任意)。

## 7. 想定する変更ファイル

- 新規: `src/ui/theme.rs`
- 変更: `src/ui/mod.rs`・`src/ui/widgets.rs`・`src/ui/list.rs`・`src/ui/edit.rs`・`src/ui/keys.rs`・`src/ui/known_hosts.rs`・`src/ui/confirm.rs`・`src/ui/help.rs`
- 不変: `config/`・`os/`・`app.rs`・`update.rs`・`event_loop.rs`(ロジックは触らない)
