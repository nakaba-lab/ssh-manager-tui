---
title: ui 領域 設計
area: ui
status: draft
relatedIssues: [49]
updated: 2026-07-28
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
| 画面別 | `list`・`edit`・`diff`・`vault`・`sftp`・`keys`・`known_hosts`・`confirm`・`connect_override`・`help` |

## UI/画面設計（TUI）

- **画面遷移**: `Screen` enum が駆動。モーダルは `App::prev_screen` + `open_overlay`/`close_overlay`。画面/モード追加時は `Screen`（app.rs）・dispatch（update.rs）・draw（ui/mod.rs）の 3 箇所を触る。
- **主要画面**: ホスト一覧（検索・到達性列・詳細ペイン）／編集フォーム／保存前 diff ／vault ／SFTP ブラウザ（2 ペイン）／鍵マネージャ／known_hosts ／ヘルプ。キーバインドの一覧は [README](../../README.md#keybindings) が真実源。
- **状態設計**: 空（0 件）・ローディング（到達性 `checking`）・エラー（`Toast`）・成功（自動失効 Toast）を各画面で扱う。
- **レスポンシブ**: `responsive_split` が `WIDE_MIN_WIDTH`（90 桁）以上で横並び・未満で縦積み。フッターは 80 桁以内で描ける（`footers_fit_80_cols` テスト）。
- **配色/コントラスト**: `theme.rs` の Tokyo Night パレットに集約。

### 鍵マネージャの ssh-agent ブロック（#49）

detail ペイン下部に `section_header` で **独立したブロック**を切る。agent 全体状態・サービス状態は**鍵別ではなく全体**の情報なので、鍵別の kv リストには混ぜない（スコープの混在を避ける）。

```
┌ Keys ──────────────┐┌ Key detail ───────────────┐
│▶ ● id_ed25519      ││          name  id_ed25519 │
│    ED25519 256 agent││          type  ED25519    │
│  ● id_rsa          ││          bits  256        │
│    RSA 4096        ││   fingerprint  SHA256:Rl… │
│  ○ work.pub        ││   private key  present    │
│    ED25519 256     ││          pair  verified … │
└────────────────────┘│                           │
                      │ ── ssh-agent ──────────── │
                      │        status  running (2)│
                      │       service  running    │
                      │      this key  loaded     │
                      └───────────────────────────┘
```

- **一覧のバッジ**: 既存の `mismatch` バッジと同じ作り（`Span::styled` のテキスト＋`theme` 色）で `agent` を出す。グリフではなくテキストにするのは既存踏襲＋幅計算の安定のため。
- **行は `widgets::kv_line_colored` で組む**: 値が「判定」を担う行（`pair`・`status`・`service`・`this key`）は色付き、素の行は `kv_line`。両者は同一実装（`kv_line` が委譲）なので、ラベル列（14 桁）の桁揃えが 1 箇所定義になる。
- **状態設計**（このブロックが空/不明/エラーを集約する）:

  | 状態 | `status` 行 | `this key` 行 |
  |---|---|---|
  | プローブ中 | `checking…` | `—` |
  | agent 稼働・鍵あり | `running (N keys)` | `loaded` / `not loaded` |
  | agent 稼働・鍵なし | `running (no keys)` | `not loaded` |
  | agent 未起動 | `not running` | `—` |
  | `ssh-add` が 0/1/2 以外で終了・起動不可 | `unavailable` | `—` |
  | fingerprint 不明・pair 不一致の鍵 | （agent 状態は出る） | `unknown` |

  **判定の優先順位**: `this key` は fingerprint 不明／pair 不一致を **agent 状態より先に**見る。したがって「agent 未起動 ＋ fingerprint 不明」は `—` ではなく `unknown` になる（どちらも「分からない」だが、鍵側の理由の方が具体的なため）。

- **サービス行の案内**（Windows のみ・`service` の値で決まる。`status` ではない）:
  - `stopped or disabled` → `Set-Service -StartupType Automatic` → `Start-Service` の **2 行**（素の Windows は ssh-agent を無効で出荷し、`sc query` は無効も `STOPPED` と報告するため片方だけでは失敗する）。
  - `status = not running` かつ `service = running` → 昇格の食い違いを疑う 1 行（サービスは動いているのに到達できない典型）。
  - sshm は起動を代行しない（昇格が要るため）。
- **非 Windows**: `service` 行を出さない。実装は `#[cfg(windows)]` ではなく **`AgentSnapshot::service: Option<_>` が `None`** になることで抑止する（cfg で囲うと `parse_service_state` のテストが Linux CI で 1 件も走らなくなるため。ランタイム判定の方が単体テスト可能）。
- **レスポンシブ**: 既存 `responsive_split(area, 45, 55)` をそのまま使う。縦積み（90 桁未満）でも agent ブロックは detail 内の最下部に収まる。
- **フッターは 80 桁以内**（`footers_fit_80_cols` が門番）。`a load`・`D unload` の 2 つを足すと **87 桁**になり 80 桁端末で末尾のヒントが黙って切れたため、`generate`→`gen`・`copy pub`→`copy` に短縮して 78 桁に収めた。完全なラベルは `?`（ヘルプ画面）が持つ。**フッターは `KEY_MANAGER_FOOTER` 定数に切り出した** — インラインのリテラルのままだと 80 桁ガードの対象外で、この回帰が素通りする。

## 主要な設計判断（現行の理由）

- **描画と mutation の分離**: `ui/` は状態を変えず、全 mutation を `update.rs`/`app.rs` に集約（Elm 風）。テスト・見通しのため。
- **色の単一真実源**: 画面ごとの色ハードコードを禁止し `theme.rs` に集約（テーマ一貫性）。
