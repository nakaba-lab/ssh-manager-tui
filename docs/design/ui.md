---
title: ui 領域 設計
area: ui
status: draft
relatedIssues: [46]
updated: 2026-07-27
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
| 画面別 | `list`・`edit`・`diff`・`vault`・`sftp`・`keys`・`known_hosts`・`confirm`・`connect_override`・`help`・`keyscan`（#46 で新規: ホスト鍵スキャンモーダル） |

## UI/画面設計（TUI）

- **画面遷移**: `Screen` enum が駆動。モーダルは `App::prev_screen` + `open_overlay`/`close_overlay`。画面/モード追加時は `Screen`（app.rs）・dispatch（update.rs）・draw（ui/mod.rs）の 3 箇所を触る。
- **主要画面**: ホスト一覧（検索・到達性列・詳細ペイン）／編集フォーム／保存前 diff ／vault ／SFTP ブラウザ（2 ペイン）／鍵マネージャ／known_hosts ／ヘルプ。キーバインドの一覧は [README](../../README.md#keybindings) が真実源。
- **状態設計**: 空（0 件）・ローディング（到達性 `checking`）・エラー（`Toast`）・成功（自動失効 Toast）を各画面で扱う。
- **ホスト鍵スキャンモーダル（#46・`Screen::KeyScan` オーバーレイ）**: 導線は ActionMenu の「Scan host key」（明示起動のみ。接続時の「host key not yet trusted」トーストに案内文言を追記するが接続フローには割り込まない＝案A）。モーダル内状態は `Scanning`（spinner 文言）→ `Results`（全鍵一括ピン）／`Error`（sticky 表示）。既存鍵と一致は「trusted」表示・不一致は **CHANGED 警告のみ**（上書き操作は置かない）。プロキシ経由ホストではスキャン不可の案内を出す。

  採択ワイヤーフレーム（案1: 全鍵一括ピン。案2 の鍵ごと選択式は、部分ピンだと鍵種ネゴ次第で TOFU が再発しうるため不採用）:

  ```
  ┌─ Scan host key: web-prod ──────────────────────┐
  │ host.example.com:22 — 3 keys found             │
  │                                                │
  │ ED25519 SHA256:Rlb8…xQ4  +--[ED25519 256]--+   │
  │                          |   .o+o.. (art)  |   │
  │ ECDSA   SHA256:aa11…     +--[ECDSA 256]---+    │
  │ RSA     SHA256:bb22…     +--[RSA 3072]----+    │
  │                                                │
  │ Verify against a trusted source before         │
  │ pinning (server console / provider docs).      │
  │                                                │
  │ [y] pin all 3 keys   [Esc] cancel              │
  └────────────────────────────────────────────────┘
  ```

  画面遷移（#46 追加分）:

  ```mermaid
  stateDiagram-v2
      List --> ActionMenu: Enter/a
      ActionMenu --> KeyScan: Scan host key
      KeyScan --> KeyScan: scanning → results/error
      KeyScan --> List: y（ピン留め→成功 Toast）
      KeyScan --> List: Esc（変更なし）
  ```
- **レスポンシブ**: `responsive_split` が `WIDE_MIN_WIDTH`（90 桁）以上で横並び・未満で縦積み。フッターは 80 桁以内で描ける（`footers_fit_80_cols` テスト）。
- **配色/コントラスト**: `theme.rs` の Tokyo Night パレットに集約。

## 主要な設計判断（現行の理由）

- **描画と mutation の分離**: `ui/` は状態を変えず、全 mutation を `update.rs`/`app.rs` に集約（Elm 風）。テスト・見通しのため。
- **色の単一真実源**: 画面ごとの色ハードコードを禁止し `theme.rs` に集約（テーマ一貫性）。
