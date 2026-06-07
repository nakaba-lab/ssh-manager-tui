# sshm UI レイアウト・余白・密度の刷新 設計

- 日付: 2026-06-07
- 対象: `sshm`(ratatui TUI)のレイアウト・余白・情報密度の刷新(第2弾)
- 前提: 第1弾「ビジュアルリデザイン」(Tokyo Night 配色 / 共通ウィジェット `panel`・`modal_block` / `theme` モジュール)は `feature/ui-visual-redesign` で完了済み。本設計はその上に積む。
- アプローチ: **B(レスポンシブ + グルーピング)**。構造に踏み込むが挙動は壊さない。

## 1. 目的とスコープ

### 目的

第1弾で色・枠・記号は整えたが、レイアウト比率・余白・情報の並べ方(密度)は未着手だった。本設計では:

1. ホスト一覧とキー管理を**端末幅に応じてレスポンシブ化**(広い=横並び、狭い=縦積み)し、どの端末幅でも破綻しないようにする。
2. 詳細ペインと編集フォームを**セクション分け**して情報をスキャンしやすくする。
3. 節間の空行で**縦のリズム**を整える。

### 決定済みの前提

- 構造変更可。ただし**挙動は維持**(新キーバインド無し・`App` 状態追加無し・純粋な描画変更)。
- レスポンシブは「詳細を隠す」のではなく「縦に積む」方式。Tab フォーカス・accent 枠・`detail_scroll`・スクロール追従はすべて不変。
- セクション化は**描画専用**。編集フォームのフィールドモデル(`app.rs` の `FIELD_LABELS`/`form_idx`/`MULTI_FIELDS`)は一切変更しない。
- 色は第1弾で完了 → 本設計で配色は変えない。

### 非対象(やらないこと)

- 新キーバインド・挙動追加(詳細の開閉トグル等は将来の C 案送り)。
- フィールドモデルの変更・並べ替え(`form_idx`/`MULTI_FIELDS` 不変)。
- `known_hosts` 画面のレイアウト(単一列で既に適切 → 変更なし)。
- モーダルのサイズ変更(`centered()`/`centered_pct()` が既に端末サイズへクランプ済み)。
- 狭い端末でのフッターヒント溢れ対策(現状の切り詰め表示のまま。将来拡張候補)。
- `config/`・`os/` レイヤー(不変)。

## 2. アーキテクチャ

`ui/` の描画に閉じる。一方向依存(`config/` ← `os/` ← `ui/`)は不変。`App` への状態追加なし。各 `draw*` 関数は描画時に `area: Rect` を受け取るので、幅依存のレイアウト選択は**追加状態なしの純粋な描画**で実現できる。

### `widgets.rs` への追加

```
/// area を 2 ペインに分割する。十分広ければ横並び、狭ければ縦積み。
/// 戻り値は (primary, secondary)。
/// side_pct  = 横並び時の primary の幅%
/// stack_pct = 縦積み時の primary の高さ%
pub fn responsive_split(area: Rect, side_pct: u16, stack_pct: u16) -> (Rect, Rect)

/// 端末幅がこれ以上なら横並び、未満なら縦積み。調整可能な定数。
pub const WIDE_MIN_WIDTH: u16 = 90;

/// セクション小見出しの 1 行(dim・太字)。詳細ペインと編集フォームで共用。
pub fn section_header(title: &str) -> Line<'static>
```

- `responsive_split` の分岐: `if area.width >= WIDE_MIN_WIDTH { 横 } else { 縦 }`。横は `Layout::horizontal([Percentage(side_pct), Percentage(100 - side_pct)])`、縦は `Layout::vertical([Percentage(stack_pct), Percentage(100 - stack_pct)])`。
- `section_header` は `theme::DIM` + `Modifier::BOLD`。先頭にインデント空白を付ける表現は実装側で統一する。
- `WIDE_MIN_WIDTH = 90` の根拠: 90 桁で詳細 42% ≈ 内側 33 桁。14 幅ラベル + 値が収まる目安。未満では縦積みに切り替えて窮屈さを解消。

## 3. レスポンシブ分割

### ホスト一覧(`list.rs::draw`)

現在:
```rust
let cols = Layout::horizontal([Percentage(58), Percentage(42)]).split(area);
draw_list_pane(f, app, cols[0]);
draw_detail_pane(f, app, cols[1]);
```

変更後:
```rust
let (list_area, detail_area) = widgets::responsive_split(area, 58, 60);
draw_list_pane(f, app, list_area);
draw_detail_pane(f, app, detail_area);
```

- 広い: 左右 58/42(現状維持)。
- 狭い: 上下 60/40(一覧が上、詳細が下)。
- `draw_list_pane` 内部の「検索 1 行 + テーブル」構造はそのまま(与えられた領域に追従)。
- `draw_empty`(ホスト 0 件)は分割しない全面表示のまま。

### キー管理(`keys.rs::draw`)

現在 `Layout::horizontal([Percentage(45), Percentage(55)])`(詳細が広め)。

変更後(`cols[0]`/`cols[1]` を `list_area`/`detail_area` に差し替えるだけ):
```rust
let (list_area, detail_area) = widgets::responsive_split(area, 45, 55);
// ... 既存の List 構築はそのまま ...
f.render_stateful_widget(list, list_area, &mut app.keys_state);
draw_detail(f, app, detail_area);
```
- 広い: 左右 45/55(現状維持)。
- 狭い: 上下 55/45(鍵リストが上、詳細が下)。
- 鍵 0 件の空状態は全面表示のまま。

### 挙動の不変性

- Tab による `ListFocus::Hosts`/`Detail` 切替、フォーカス枠(accent)、`detail_scroll`、ステートフルウィジェットの選択状態はいずれも領域の置き方に依存しないため、横→縦に変わっても完全に維持される。

## 4. 詳細ペインのグルーピング(`list.rs::draw_detail_pane`)

status 行(先頭・グループ外)の下に、**中身がある節だけ** dim 小見出し + 節前の空行で表示する。

- **Connection**: alias / HostName / User / Port / ProxyJump(ProxyJump は値がある時のみ行を出す)
- **Identity**: IdentityFile × N(1 つ以上ある時のみ節ごと表示)
- **Forwarding**: LocalForward / RemoteForward / DynamicForward(1 つ以上ある時のみ)
- **Other**: extras(1 つ以上ある時のみ)

例:
```
    status  up (12 ms)

  Connection
     alias  web-prod
  HostName  10.0.0.5
     User   admin
     Port   2222
 ProxyJump  web-prod

  Identity
 IdentityFile  ~/.ssh/id_ed25519

  Forwarding
 LocalForward  8080 localhost:80
```

- 小見出しは `widgets::section_header`(dim・太字)。
- kv 行は既存の `widgets::kv_line`(14 幅 dim ラベル + TEXT 値)を維持。`.wrap(Wrap{trim:false})` と `.scroll((app.detail_scroll, 0))` も維持。
- 「Connection」節は常に表示(alias 等は必ずある)。Identity/Forwarding/Other は該当データがある時のみ小見出しごと出す(空節の見出しは出さない)。

## 5. 編集フォームのセクション化(`edit.rs::draw`)

10 フィールドは連続インデックスなので、フィールドモデルを触らず描画側に小見出しを差し込むだけで節に分けられる。各節の前に小見出し + 空行(先頭節の前は空行なし)。

- **Connection**: `HOST`(0) / `HOSTNAME`(1) / `USER`(2) / `PORT`(3)
- **Identity & routing**: `IDENTITY`(4) / `PROXYJUMP`(5)
- **Forwarding**: `LOCAL_FWD`(6) / `REMOTE_FWD`(7) / `DYNAMIC_FWD`(8)
- **Advanced**: `EXTRAS`(9)

実装方針:
- フィールド描画ループの各反復で、`idx` が節の先頭(0 / 4 / 6 / 9)なら、その前に `section_header(...)`(と先頭以外は空行)を `lines` に push してからフィールド行を push する。
- フォーカス移動・Tab・バリデーションエラー表示・複数値行の展開は**フィールド index ベースのまま不変**。小見出し/空行は `lines` に積まれるだけなので、`focus_line = lines.len()`(フォーカス中フィールド時点の行数)によるスクロール追従に自然に含まれ、正しく可視化される。
- `inner_h = area.height.saturating_sub(2)` のスクロール計算は不変。
- 小見出しは詳細ペインと同じ `widgets::section_header`。

## 6. 余白のリズムと一貫性

- 縦のリズムは §4・§5 の**節間の空行**で表現する。専用の縦パディングは導入しない。
- パネルの**左右 1 セル padding は維持、縦 padding は追加しない**。理由: 編集フォームのスクロール計算(`inner_h = height - 2`、枠線のみを想定)とテーブルの行数前提を壊さないため。
- 小見出しは詳細/フォームで `widgets::section_header` を共用し、見た目を統一(重複実装を避ける)。

## 7. エラーハンドリング・後方互換

- すべて描画ロジックのみ。失敗経路は増えない。
- 極端に低い高さの端末で縦積みにすると各ペインが数行になるが、ratatui がクリップするため破綻はしない(横並びでも同様の既存挙動)。
- `WIDE_MIN_WIDTH` 付近の幅で横/縦が切り替わる。実値は目視で微調整可能な定数とする。

## 8. テスト・検証

- **ユニットテスト**: `responsive_split` は純粋関数なので、`widgets.rs` に `#[cfg(test)]` を追加してテストする:
  - 幅 ≥ `WIDE_MIN_WIDTH` の `Rect` を渡すと、2 ペインが**横並び**(合計幅が元の幅、高さは元の高さと同じ)になること。
  - 幅 < `WIDE_MIN_WIDTH` の `Rect` を渡すと、2 ペインが**縦積み**(合計高さが元の高さ、幅は元の幅と同じ)になること。
  - 既存の `centered`/`centered_pct` のテスト有無に合わせ、最小限の検証で可。
- それ以外(グルーピング/セクション化)は描画専用のため、`cargo build` / `cargo clippy --all-targets -- -D warnings` / `cargo fmt --check` + 手動目視で検証。
- **手動目視**(使い捨て config で):
  - 広い端末(≥90 桁): ホスト一覧/キー管理が従来どおり横並び。
  - 狭い端末(<90 桁): 一覧↑/詳細↓・鍵↑/詳細↓に縦積みされ、Tab フォーカス・スクロールが効く。
  - 詳細ペイン: Connection/Identity/Forwarding/Other の小見出しが出て、空節の見出しが出ないこと。
  - 編集フォーム: 4 節の小見出しが出て、Tab 移動・スクロール追従・バリデーション表示が正しいこと。

## 9. 想定する変更ファイル

- `src/ui/widgets.rs`: `responsive_split`、`section_header`、`WIDE_MIN_WIDTH`、`responsive_split` のユニットテスト。
- `src/ui/list.rs`: `draw` のレスポンシブ分割、`draw_detail_pane` のグルーピング。
- `src/ui/keys.rs`: `draw` のレスポンシブ分割。
- `src/ui/edit.rs`: `draw` のセクション化。
- 不変: `src/ui/mod.rs`・`known_hosts.rs`・`confirm.rs`・`help.rs`・`theme.rs`・`app.rs`・`config/`・`os/`。
