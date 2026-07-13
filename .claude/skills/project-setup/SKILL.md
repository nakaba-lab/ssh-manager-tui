---
name: project-setup
description: このテンプレートを新規/既存プロジェクトに適用し、プロジェクトプロファイルを生成するとき使用する。「セットアップして」「プロジェクトを初期化して」「このテンプレを適用して」「project-setup」が発火ワード。適合済みプロジェクトの再同期（自動化・CLAUDE.md・プロファイルの更新）は /project-resync。
disable-model-invocation: true
---

このテンプレート（言語非依存の標準開発ワークフロー）を、新規または既存のプロジェクトに適合させる対話型セットアップ。各工程でユーザーの確認を取りながら進め、最後に **プロジェクトプロファイル**（`.claude/project-profile.json` と CLAUDE.md「プロジェクト設定」節）を埋める。

> 原則: ファイル生成（要件定義書・プロファイル・各種設定）は都度ユーザーの承認を取ってから行う。破壊的操作はしない。

---

## 進行管理（重要）

ステップの飛ばし・順序乱れを防ぐため、`/dev-tasks` と同じく **Task システム**で各工程を **1 タスク = 1 役割**に分解し、直列タスク化して管理する。

### ロックの大原則（飛ばし・マージ・無作業完了を起きにくくする）

`blockedBy` はタスクを着手できる**順序**だけを固定する。次の 3 つは `blockedBy` では止められないため、各タスクに以下を埋め込んで「ロック」する（機械的な強制ではなく、Claude を正しく誘導するための規律）。**唯一の構造的な歯止めは AskUserQuestion**（人間入力を待つため、そのターンが必ずそこで切れる＝隣接タスクを 1 ターンで詰め込めなくなる）。

- 各タスクは **1 役割**。複数の作業を 1 タスクに束ねない（束ねると「片方だけやって completed」を検出できない）。
- 各タスクの `description` 末尾に必ず次の 2 行を置く:
  ```
  【完了条件】<観測可能な単一の証拠：コマンド出力／生成ファイルパス／差分／AskUserQuestion の回答>
  【証拠なしに completed 禁止】上の証拠を提示する前に TaskUpdate で completed にしない。提示できないなら in_progress のまま留め、不足を報告する。
  ```
- **【人間ゲート】** 印のタスクは、`AskUserQuestion`／利用者の承認を得るまで `completed` にしない。隣接タスクを 1 ターンに詰め込まない。
- **転記忠実性**: `TaskCreate` の subject はタスク表の subject を一字一句使い、`description` は表のセル（単一責務・DoD・種別）を逐語転記する（要約・言い換え・省略をしない）。表が長い場合も切り詰めない — 完了条件・人間ゲート表記が落ちると進行の歯止めが消える。
- **タスクに着手する直前に、そのタスクの詳細ファイル（タスク表の「詳細」列）だけを Read してから作業する**（全ファイルの先読みはしない＝コンテキスト節約。詳細を読まずに作業しない）。

### 線形チェーン不変条件

- 各タスクは `TaskUpdate` の `addBlockedBy` で**直前タスク 1 本だけ**に繋ぐ（分岐・合流を作らない＝単一の線形鎖）。
- タスク生成直後に `TaskList` を読み、`[先頭] → … → [末尾]` の単一鎖になっており、**役割名と件数（新規 15／既存 14）が想定どおり**であることを名前で 1 つずつ照合する。
- 照合が通って初めて先頭タスクを `in_progress` にし、Step 1 完了とする（チェーン未確認のまま走り出さない）。

> Step 1（新規/既存の判定）だけはタスク集合を決める前段ゲートのため、タスク化せず先に対話で確定する。

---

## Step 1: 判定（新規 or 既存）

ユーザーに確認する：「**新規プロジェクトの作成**ですか、それとも**既存プロジェクトへの導入**ですか？」

- **既存プロジェクトへの導入** → ヒアリング・要件ディスカッション・雛形構築を行わず（環境チェックは軽量版のみ）、**テンプレ導入ブロックの削除＋環境チェック（軽量）＋ AI 初期化（`/init`）から始まる 14 タスク**を生成する。
- **新規プロジェクトの作成** → **15 タスク**を生成する。

### タスク生成（新規プロジェクト：15 タスク）

`TaskCreate` で以下を順に作成する（`description` には呼ぶスキル・確認事項・末尾のロック 2 行を含める。詳細手順は各行の**「詳細」列のファイル**を参照）。

| # | subject | 単一責務（要点） | 単一・検証可能な DoD | 種別 | 詳細 |
|---|---------|------------------|----------------------|------|------|
| 1 | 環境バリデーション | ツール存在確認のみ → OK/未設定 表。LSP/プラグイン行は言語未定なら「保留」 | LSP 行を除く全項目 OK、または未設定項目を利用者が「このまま進む」と明示承認（確認表提示済み） | auto | `.claude/skills/project-setup/tasks/environment-validation.md` |
| 2 | ヒアリング充足ループ | `AskUserQuestion` 反復・前進保証・全体ゲート。**末尾で LSP/プラグイン行を再確認**。内部反復は分割しない | 必須チェックリストが全て「充足」または「適用デフォルト化」になり、未充足必須が 0 件 | 【人間ゲート】 | `.claude/skills/project-setup/tasks/hearing.md` |
| 3 | 要件ディスカッション（論点すり合わせ・理解の照返し） | 聴取内容から論点・盲点を抽出し **1 論点ずつ** AskUserQuestion で確定 → 「私はこう理解した」の構造化サマリを照返し（作法の正は `.claude/rules/alignment.md`。決着しない論点は「未決事項」へ） | 全論点が「確定」または「未決事項に記録」になり、照返しに対し利用者が認識一致を明示（論点なしと判断した場合もその判断を AskUserQuestion で確認済み） | 【人間ゲート】 | `.claude/skills/project-setup/tasks/requirements-discussion.md` |
| 4 | 要件定義書の作成 | 必須再判定 → 承認後 `docs/要件定義書.md` 生成（draft は照返しとともに提示） | `docs/要件定義書.md` が存在し必須 10 節（Web 時は UI/UX 方針を含む 11 節）を含み、利用者が承認 | 【人間ゲート】 | `.claude/skills/project-setup/tasks/requirements-doc.md` |
| 5 | 雛形構築・設定ファイル・後始末 | scaffold ＋ `checks` 参照設定ファイル生成 ＋ `git add` 前のスキャフォールド後始末 | `checks` 候補が設定不足エラー無しで 1 回実行（ログ提示）＋ 判明した test/build（無ければ最小 import スモーク）が依存解決込みで完走（ログ提示）＋ 入れ子 `.git`・重複エントリポイント・未 ignore 成果物が残存なし | auto | `.claude/skills/project-setup/tasks/scaffolding.md` |
| 6 | テンプレ導入ブロックの削除 | `TEMPLATE-ONBOARDING` ブロックを削除＋整形（**`/init` の前**＝生成系の追記が削除領域に入る罠を断つ） | 削除差分を提示し、マーカが 1 つも残っていない（grep 相当）＋ 間が「空行 → `---` → 空行」1 組 | auto | `.claude/skills/project-setup/tasks/template-onboarding-removal.md` |
| 7 | `/init` 実行＋追記確認 | `/init` → 追記レビューゲート | `/init` 完了で CLAUDE.md に追記され、利用者が追記内容を確認 | 【人間ゲート】 | `.claude/skills/project-setup/tasks/init-review.md` |
| 8 | 推奨キュレーション＋適用 | recommender 実行 → 提示 → AskUserQuestion 採否 → 承認分のみ `.claude/` 適用 | 採否回答を取得し、承認分のみが `.claude/` に反映（適用/除外サマリ提示）＋ 適用物がロード可能（settings.json/.mcp.json が valid 等）の最小検証 | 【人間ゲート】 | `.claude/skills/project-setup/tasks/recommendation-curation.md` |
| 9 | CLAUDE.md 改善キュレーション（減算） | improver 実行 → 除外指定のみ問う → 残集合で自己編集 | 除外指定を反映して CLAUDE.md に適用、レポートが放置でない | 【人間ゲート】 | `.claude/skills/project-setup/tasks/claude-md-curation.md` |
| 10 | テンプレ自己言及の最終検証 | improver 後に残存自己言及を grep 検証 → あれば自動除去して再検証（**improver の後**） | マーカ・セットアップ前提の自己言及文が 1 つも残っていない（grep 相当） | auto | `.claude/skills/project-setup/tasks/self-reference-sweep.md` |
| 11 | プロファイル確定（2 層同時更新） | `project-profile.json` ＋ CLAUDE.md「プロジェクト設定」節を**同時** | 2 層の値が完全一致し、`checks` が空でなければ各 `match` を fnmatch 検証済み | auto | `.claude/skills/project-setup/tasks/profile-finalization.md` |
| 12 | README をプロジェクト向けに再生成 | テンプレ説明 README をプロジェクト固有 README に再生成（既存はテンプレ README のみ）→ 提示 → 承認 | 旧テンプレ自己言及が残らず、名前・説明・profile 由来コマンドを反映し、利用者が承認（既存で独自 README なら据え置き判断を提示） | 【人間ゲート】 | `.claude/skills/project-setup/tasks/readme-regeneration.md` |
| 13 | ドキュメント雛形生成（設計書・用語集・変更履歴） | `docs/design/README.md`（索引＋節テンプレ案内。構造は `.claude/rules/design-doc.md`）・`docs/design/architecture.md`（全体構成。Mermaid 構成図）・`docs/用語集.md`（業務用語→英語識別子の対訳）・`CHANGELOG.md`（`## [Unreleased]` のみ）の雛形を承認のうえ生成（`docs/操作マニュアル.md`・`docs/運用ガイド.md` は要否確認のうえ任意生成）。新規＝要件定義から初期領域候補、既存＝`/init` のアーキ要約から提案 | `docs/design/README.md`・`docs/design/architecture.md`・`docs/用語集.md`・`CHANGELOG.md` が存在し、利用者が承認 | 【人間ゲート】 | `.claude/skills/project-setup/tasks/docs-scaffolding.md` |
| 14 | docs サイト有効化 | docs-site の `npm ci`（有効化時は `npm run check`＝astro check も）＋ Pages デプロイ設定の確認（GitHub=branch 整合／GitLab=ルート `.gitlab-ci.yml` に同梱済み）。docs 不要なら無効化 | `npm ci`＋（有効化時）`npm run check`〔astro check＝設計書 frontmatter 含むビルド検証〕緑のログ（または無効化判断）＋ Pages 設定の確認を提示し、利用者が有効化/無効化を選択 | 【人間ゲート】 | `.claude/skills/project-setup/tasks/docs-site-enablement.md` |
| 15 | バージョン管理・リモート同期（push＝最終承認） | git 管理確認 → `git init`+develop（ローカル可逆）→ remote 確認 → AskUserQuestion → 作成（`--push` 禁止）+ push | `main`・`develop` が remote に push 済み（`git ls-remote` 確認）、または利用者が「push しない」と明示判断 | 【人間ゲート】 | `.claude/skills/project-setup/tasks/version-control-remote-sync.md` |

`blockedBy` 連鎖（完全直列・ヘッド = 環境バリデーション）:
```
ヒアリング充足ループ          ← blockedBy 環境バリデーション
要件ディスカッション          ← blockedBy ヒアリング充足ループ
要件定義書の作成              ← blockedBy 要件ディスカッション
雛形構築・設定ファイル・後始末  ← blockedBy 要件定義書の作成
テンプレ導入ブロックの削除      ← blockedBy 雛形構築・設定ファイル・後始末
/init 実行＋追記確認          ← blockedBy テンプレ導入ブロックの削除
推奨キュレーション＋適用       ← blockedBy /init 実行＋追記確認
CLAUDE.md 改善キュレーション   ← blockedBy 推奨キュレーション＋適用
テンプレ自己言及の最終検証     ← blockedBy CLAUDE.md 改善キュレーション
プロファイル確定             ← blockedBy テンプレ自己言及の最終検証
README 再生成               ← blockedBy プロファイル確定
ドキュメント雛形生成 ← blockedBy README 再生成
docs サイト有効化 ← blockedBy ドキュメント雛形生成
バージョン管理・リモート同期    ← blockedBy docs サイト有効化
```

### タスク生成（既存プロジェクト：14 タスク）

`/init` 以降のテールは新規と共有する。ヘッドは新規フローと同じ「テンプレ導入ブロックの削除」（`/init` 前に出荷時マーカを消す）。「環境バリデーション」タスク（新規のみ）を通らないため、その軽量版「環境チェック（軽量）」タスクをヘッドの直後に足す（これが無いとフック実行の python3 等が欠けたまま後続タスクが無言で失敗・劣化する）。ヒアリング・雛形構築も通らないため、`vcsHost` と `frontendDir` を確定する「プロファイル前提値の確定」タスクを profile の前段に足す（これが無いと既存 Web プロジェクトで frontendDir が空のまま確定し、ビジュアル/UX 検証パイプライン全体が無言で停止する）。また要件定義工程（ヒアリング充足ループ・要件ディスカッション・要件定義書の作成）も通らないため、「要件定義書の有無確認」タスクを `/init` の直後に足す（要件定義書が無い既存プロジェクトは `/init` のコードベース要約を種にした軽量ヒアリング＋論点ディスカッション（軽量版）・理解の照返しで生成するか、opt-out を明示記録する）。

| # | subject | 単一責務（要点） | 種別 | 詳細 |
|---|---------|------------------|------|------|
| 1 | テンプレ導入ブロックの削除 | 新規 #6 と同一（`TEMPLATE-ONBOARDING` ブロックを `/init` 前に削除＋整形） | auto | `.claude/skills/project-setup/tasks/template-onboarding-removal.md` |
| 2 | 環境チェック（軽量） | 「環境バリデーション」タスク（新規のみ）の軽量版。python3・git・Node〔docsSite 利用時のみ〕・gh/glab〔リモートがある場合のみ〕の存在確認表 → 未充足は明示承認で前進 | auto | `.claude/skills/project-setup/tasks/existing-environment-check.md` |
| 3 | `/init` 実行＋追記確認 | 新規 #7 と同一（判明したコマンド・言語・FW を控える） | 【人間ゲート】 | `.claude/skills/project-setup/tasks/init-review.md` |
| 4 | 要件定義書の有無確認 | `docs/要件定義書.md` の存在確認 → 無ければ `/init` 要約を種に軽量ヒアリング（目的・スコープ・非機能・テスト方針・（Web 時）UI/UX 方針）＋論点ディスカッション（軽量版）・理解の照返しで生成、不要なら opt-out を CLAUDE.md 追記欄に記録 | 【人間ゲート】 | `.claude/skills/project-setup/tasks/existing-requirements-doc-check.md` |
| 5 | 推奨キュレーション＋適用 | 新規 #8 と同一 | 【人間ゲート】 | `.claude/skills/project-setup/tasks/recommendation-curation.md` |
| 6 | CLAUDE.md 改善キュレーション（減算） | 新規 #9 と同一 | 【人間ゲート】 | `.claude/skills/project-setup/tasks/claude-md-curation.md` |
| 7 | テンプレ自己言及の最終検証 | 新規 #10 と同一 | auto | `.claude/skills/project-setup/tasks/self-reference-sweep.md` |
| 8 | プロファイル前提値の確定（VCSホスト・フロントエンド構成） | `git remote -v` 等で `vcsHost` を、コードベース解析で `frontendDir`（Web 相当なら UI ディレクトリ／ルート直下は `"."`／UI 無しは `"none"`）を確定（profile 前段） | auto | `.claude/skills/project-setup/tasks/existing-profile-prerequisites.md` |
| 9 | プロファイル確定（2 層同時更新） | 新規 #11 と同一（値はコードベース由来） | auto | `.claude/skills/project-setup/tasks/profile-finalization.md` |
| 10 | デザインの土台の導入（Web のみ） | kind=web なら ブラウザ環境確認 → デザイントークン置き場・コンポーネントカタログ採否・a11y lint（`/setup-js`）・基線スクショを承認のうえ導入（非 Web・`frontendDir: "none"` は対象外提示で完了） | 【人間ゲート】 | `.claude/skills/project-setup/tasks/existing-design-foundation.md` |
| 11 | README をプロジェクト向けに再生成 | 新規 #12 と同一（既存はテンプレ README のときだけ再生成。名前=ディレクトリ/CLAUDE.md、説明=/init 要約） | 【人間ゲート】 | `.claude/skills/project-setup/tasks/readme-regeneration.md` |
| 12 | ドキュメント雛形生成（設計書・用語集・変更履歴） | 新規 #13 と同一（既存は `/init` のアーキ要約から architecture・初期領域候補、用語集はコードベースの主要概念から） | 【人間ゲート】 | `.claude/skills/project-setup/tasks/docs-scaffolding.md` |
| 13 | docs サイト有効化 | 新規 #14 と同一（docs-site の npm ci ＋〔有効化時〕astro check ＋ Pages 設定確認） | 【人間ゲート】 | `.claude/skills/project-setup/tasks/docs-site-enablement.md` |
| 14 | バージョン管理・リモート同期（push＝最終承認） | git 確認（多くは確認のみ）→ remote 確認 → AskUserQuestion → 存在ブランチのみ push | 【人間ゲート】 | `.claude/skills/project-setup/tasks/version-control-remote-sync.md` |

> **参照規約**: 本文の相互参照は**タスク名**で行う。番号は表のみ（詳細ファイルは番号を持たない kebab 名。新規/既存フローで同一タスクの番号がずれるため、本文の無修飾「タスク N」参照は誤誘導になる）。

`blockedBy` 連鎖（完全直列・ヘッド = テンプレ導入ブロックの削除）:
```
環境チェック（軽量）          ← blockedBy テンプレ導入ブロックの削除
/init 実行＋追記確認          ← blockedBy 環境チェック（軽量）
要件定義書の有無確認          ← blockedBy /init 実行＋追記確認
推奨キュレーション＋適用       ← blockedBy 要件定義書の有無確認
CLAUDE.md 改善キュレーション   ← blockedBy 推奨キュレーション＋適用
テンプレ自己言及の最終検証     ← blockedBy CLAUDE.md 改善キュレーション
プロファイル前提値の確定      ← blockedBy テンプレ自己言及の最終検証
プロファイル確定             ← blockedBy プロファイル前提値の確定
デザインの土台の導入          ← blockedBy プロファイル確定
README 再生成               ← blockedBy デザインの土台の導入
ドキュメント雛形生成 ← blockedBy README 再生成
docs サイト有効化 ← blockedBy ドキュメント雛形生成
バージョン管理・リモート同期    ← blockedBy docs サイト有効化
```

> **配線の要点（2分）**: ①「テンプレ導入ブロックの削除」は必ず `/init` の**前**に置く（生成系＝`/init`・recommender・improver が削除対象領域に追記し、後段で巻き添え削除される罠を断つ）。②「テンプレ自己言及の最終検証」は必ず improver（CLAUDE.md 改善）の**後**に置く（improver が CLAUDE.md を再編集して自己言及を復活させうるため）。いずれも分岐を作らず単一線形鎖を保つ。

全タスクの生成と `blockedBy` 連鎖の設定が完了し、線形チェーン照合（上記不変条件）を通過し、最初のタスクを `in_progress` にして初めて Step 1 完了とする（中途で次へ進まない）。

---

## 各タスクの手順

各タスクの実行詳細は `tasks/` 配下に **1 タスク = 1 ファイル**で置いてある（タスク表の「詳細」列がそのファイルを指す）。着手する直前に、そのタスクの詳細ファイル**だけ**を Read してから作業する（進行管理の規律）。各ファイルは先頭にタスク名（表の subject と一字一句一致の見出し）と対象フロー（新規のみ／既存のみ／新規・既存とも）を明記している。

**サブスキルは Skill ツールで呼ぶ**（`plugin:skill` 形式の名前を `skill` 引数に渡す）。**Agent ツール（サブエージェント）では呼べない** — 同名のエージェントは存在せず `Agent type ... not found` になる。

---

## 完了報告

最終タスク完了後に以下を提示して終了する（**タスク化しない**＝検証可能 DoD を持たない出力フェーズ）：

- 生成・更新したファイル（`docs/要件定義書.md`〔既存で opt-out した場合はその記録〕、`docs/design/README.md`・`docs/design/architecture.md`、`docs/用語集.md`、`CHANGELOG.md`、要否に応じて `docs/操作マニュアル.md`・`docs/運用ガイド.md`、`.claude/project-profile.json`、CLAUDE.md「プロジェクト設定」節の同期・テンプレ導入ブロックの除去、`README.md` のプロジェクト向け再生成。既存で独自 README なら据え置き）。docs サイト有効化の結果（`docs-site` の依存導入・Pages 設定、または無効化）。アプリ CI 雛形（app-test／release-deploy）の有効化・削除・後回しの判断
- プロファイルの内容サマリ（言語・各種コマンド・VCS・ブランチ）
- 次のアクション（最初の Issue を `/github-planning` または `/gitlab-planning` で起票 → `/dev-tasks`（開発フローを敷設して着手。worktree 未作成なら作成も含む））
