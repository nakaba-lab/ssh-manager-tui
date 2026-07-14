---
title: ui 領域 設計
area: ui
status: draft
relatedIssues: [43]
updated: 2026-07-14
---

# ui 領域 設計（`src/ui/` — TUI 描画）

> status: draft — 初期骨子。**TUI（ratatui）であり Web フロントではない**（`frontendDir: none`）。ブラウザ向けの a11y/レスポンシブ検証（`frontend-reviewer`）は対象外で、UI 配慮は本領域の記述で担保する。

## 責務

`App.screen` でディスパッチし、ベース画面 → モーダルオーバーレイの順に描く**純粋描画**。ドメイン状態を mutate しない（widget のスクロール/選択状態だけ）。

## 構成要素

| モジュール | 役割 |
|-----------|------|
| `mod.rs` | `draw()` ディスパッチ。固定 3 行フレーム（breadcrumb・body・footer hints）＋モーダル重ね |
| `theme.rs` | 全色の単一真実源（Tokyo Night）・`selection()`/`border()`/`SELECT_SYMBOL`。色ハードコード禁止 |
| `widgets.rs` | `panel`/`modal_block`/`footer_hints`/`kv_line`/`section_header`/`input_line`/`responsive_split` |
| 画面別 | `list`・`edit`・`diff`・`vault`・`sftp`・`keys`・`known_hosts`・`inspect`・`confirm`・`connect_override`・`help` |

## UI/画面設計（TUI）

- **画面遷移**: `Screen` enum が駆動。モーダルは `App::prev_screen` + `open_overlay`/`close_overlay`。画面/モード追加時は `Screen`（app.rs）・dispatch（update.rs）・draw（ui/mod.rs）の 3 箇所を触る。
- **主要画面**: ホスト一覧（検索・到達性列・詳細ペイン）／編集フォーム／保存前 diff ／vault ／SFTP ブラウザ（2 ペイン）／鍵マネージャ／known_hosts ／**実効設定インスペクタ（`ssh -G` ビュー・#43）**／ヘルプ。キーバインドの一覧は [README](../../README.md#keybindings) が真実源。
- **実効設定インスペクタ（`Screen::Inspect`・#43）**: ホスト一覧で `i` を押すと、選択ホストの `ssh -G` 実効設定を**フルスクリーンのベース画面**（`known_hosts` と同じ流儀＝`app.screen = Screen::Inspect` を直接設定・Esc で List へ戻る。モーダルオーバーレイではない）で表示する。`resolve.rs::resolve_full` の順序付き key/value を `/` の部分一致フィルタ（`kh_filtered` と同じ大小無視の substring）と j/k・g/G スクロールで閲覧する（v1 は閲覧専用＝値コピー等は対象外）。
  - **開く前の安全ゲート（fail-safe）**: `i` 押下時、`has_match_exec(app.config.render())` が真、**または** `app.config.include_count() > 0` の**いずれか**なら `ssh -G` を実行せず sticky エラートーストで退避する。前者は `ssh -G` が `Match exec` の述語を実行してしまうため、後者は `has_match_exec` がメインファイルの render しか走査せず Include 先の `Match exec` を見逃す（#43 リスク#1）ため。`ssh -G` がタイムアウト/非ゼロ/ssh 不在で失敗したときも sticky トーストを出し、インスペクタは開かない（半端な画面を出さない）。解決は開く瞬間に 1 回だけ走らせ（500ms 上限・UI スレッドは tick ごとにブロックしない設計を崩さない）、結果を App に載せてから描画する（`diff` プレビューと同じ「一度計算して載せる」流儀）。
  - **表示の正直さ（#43 リスク#2）**: `ssh -G` はキーを小文字化し値を正規化しコンパイル時デフォルトも出すため、「書いた値」との単純比較で由来をハイライトすると誤分類する。由来ハイライトはせず、ヘッダで「`ssh -G` 正規化の**近似**」であることを明示する。
- **状態設計**: 空（0 件）・ローディング（到達性 `checking`）・エラー（`Toast`）・成功（自動失効 Toast）を各画面で扱う。
- **レスポンシブ**: `responsive_split` が `WIDE_MIN_WIDTH`（90 桁）以上で横並び・未満で縦積み。フッターは 80 桁以内で描ける（`footers_fit_80_cols` テスト）。
- **配色/コントラスト**: `theme.rs` の Tokyo Night パレットに集約。

## 主要な設計判断（現行の理由）

- **描画と mutation の分離**: `ui/` は状態を変えず、全 mutation を `update.rs`/`app.rs` に集約（Elm 風）。テスト・見通しのため。
- **色の単一真実源**: 画面ごとの色ハードコードを禁止し `theme.rs` に集約（テーマ一貫性）。
- **インスペクタはベース画面（オーバーレイではない）（#43）**: `ssh -G` 出力は 40 行以上になりやすく、full-height の一覧が読みやすいため、Help/DiffPreview のような中央モーダルではなく `known_hosts` と同じベース画面にした（filterable-list パターンを踏襲＝`kh_search`/`kh_state` と同型の inspect 状態を App に持つ）。画面追加の 3 箇所（`Screen`＝app.rs／dispatch＝update.rs／draw＝ui/mod.rs）を触る一般則に従う。
