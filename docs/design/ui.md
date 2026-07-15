---
title: ui 領域 設計
area: ui
status: draft
relatedIssues: [43, 44]
updated: 2026-07-15
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
  - **開く前の安全ゲート（fail-safe）**: `i` 押下時、`inspect_block_reason(app.config.render())` が理由を返す（`has_match_exec` または `has_include` が真）なら `ssh -G` を実行せず sticky エラートーストで退避する。前者は `ssh -G` が `Match exec` の述語を実行してしまうため、後者は `has_match_exec` がメインファイルの render しか走査せず Include 先の `Match exec` を見逃す（#43 リスク#1）ため。**Include 検出は `has_match_exec` と同じ widen-only 正規化（クオート splice 除去・`=`→空白・コメント除外・インデント非依存）の text scan で行う**（パーサの `include_count()` は素の**トップレベル** Include しか数えず、`ssh -G` が honor する**ブロック内ネスト**・**クオート装飾**の Include を取りこぼすため＝これらを塞ぐ）。`ssh -G` がタイムアウト/非ゼロ/ssh 不在で失敗したときも sticky トーストを出し、インスペクタは開かない（半端な画面を出さない）。解決は開く瞬間に 1 回だけ走らせ（500ms 上限・UI スレッドは tick ごとにブロックしない設計を崩さない）、結果を App に載せてから描画する（`diff` プレビューと同じ「一度計算して載せる」流儀）。
  - **表示の正直さ（#43 リスク#2）**: `ssh -G` はキーを小文字化し値を正規化しコンパイル時デフォルトも出すため、「書いた値」との単純比較で由来をハイライトすると誤分類する。由来ハイライトはせず、ヘッダで「`ssh -G` 正規化の**近似**」であることを明示する。
- **マスターパスワード変更・KDF 昇格モーダル（`Screen::VaultRekey`・#44）**: vault 一覧（`Screen::Vault`＝アンロック済みでのみ到達）から開くモーダルオーバーレイ。`VaultUnlock` パターンを踏襲する（フォーム状態は `Screen` に載せず `App::vault_rekey` に持ち、`Drop` で zeroize・`Debug` で redact）。
  - **導線 2 つ（KDF 昇格モデル B）**: 一覧で `m` → **Change master password**（current/new/confirm の 3 フィールド）。一覧で `u` → **Upgrade vault KDF**（current の 1 フィールド・**`needs_kdf_upgrade()` が真のときだけ有効**）。両者は同じ `Screen::VaultRekey` の 2 モード（`mode: RekeyMode { ChangePassword, UpgradeKdf }`）で、内部はどちらも `os::vault::rekey()` を呼ぶ（KDF 昇格は `new_pw == current`）。
  - **可視性ヒント**: `needs_kdf_upgrade()` が真のとき、vault 一覧タイトルに `⚠ older KDF (u: upgrade)` を出す（`theme.rs` の色を使う。ハードコードしない）。デフォルト以上の KDF では出さない。
  - **画面遷移（ASCII ワイヤーフレーム）**:

    ```
    Screen::Vault  ── m ─▶  Change master password        Screen::Vault ── u(needs_upgrade) ─▶ Upgrade vault KDF
    ┌─ Change master password ───────────────┐            ┌─ Upgrade vault KDF ─────────────────────┐
    │  Re-encrypt the vault under a new pw.   │            │  Re-derive with current defaults.       │
    │  Current   ••••••••                     │            │  Confirm current password to continue.  │
    │  New       ••••••                       │            │  Current   ••••••••                     │
    │  Confirm   ••••••                       │            │  Enter upgrade · Esc cancel             │
    │  Tab move · Enter change · Esc cancel   │            └─────────────────────────────────────────┘
    └─────────────────────────────────────────┘
    Enter→verify current→rekey(new)→成功で overlay を閉じ Vault へ／Esc で破棄しスクラブ
    ```

  - **状態**: 入力中（マスク表示）／検証エラー（不一致・空・現行PW誤り＝sticky トースト、モーダルは開いたまま入力PWをスクラブ）／成功（overlay を閉じ成功トースト）。全離脱経路でフォームをスクラブ（Esc・`L`ロック・アイドル自動ロック・終了）。
- **画面追加の 3 箇所**: `Screen::VaultRekey`（app.rs）・`handle_vault_rekey` dispatch（update.rs）・`draw_rekey`（ui/vault.rs＋ui/mod.rs のディスパッチ）。開くキー `m`/`u` は `handle_vault` に追加。
- **状態設計**: 空（0 件）・ローディング（到達性 `checking`）・エラー（`Toast`）・成功（自動失効 Toast）を各画面で扱う。
- **レスポンシブ**: `responsive_split` が `WIDE_MIN_WIDTH`（90 桁）以上で横並び・未満で縦積み。フッターは 80 桁以内で描ける（`footers_fit_80_cols` テスト）。
- **配色/コントラスト**: `theme.rs` の Tokyo Night パレットに集約。

## 主要な設計判断（現行の理由）

- **描画と mutation の分離**: `ui/` は状態を変えず、全 mutation を `update.rs`/`app.rs` に集約（Elm 風）。テスト・見通しのため。
- **色の単一真実源**: 画面ごとの色ハードコードを禁止し `theme.rs` に集約（テーマ一貫性）。
- **rekey は VaultUnlock 踏襲のオーバーレイ・2 モード 1 画面（#44）**: 中央モーダル（`open_overlay`/`close_overlay`）とし、`m`（パスワード変更）と `u`（KDF 昇格）を `Screen::VaultRekey` の 2 モードに集約（別 Screen を増やさず dispatch/draw を 1 本化）。KDF 昇格は `needs_kdf_upgrade()` 真のときだけ導線を出し、手動強化 vault にダウングレードを勧めない。フォームを `Screen` に載せないのは `Screen` の `Debug`/`Clone` 導出に平文パスワードを漏らさないため（`VaultUnlock` と同じ規律）。
- **インスペクタはベース画面（オーバーレイではない）（#43）**: `ssh -G` 出力は 40 行以上になりやすく、full-height の一覧が読みやすいため、Help/DiffPreview のような中央モーダルではなく `known_hosts` と同じベース画面にした（filterable-list パターンを踏襲＝`kh_search`/`kh_state` と同型の inspect 状態を App に持つ）。画面追加の 3 箇所（`Screen`＝app.rs／dispatch＝update.rs／draw＝ui/mod.rs）を触る一般則に従う。
