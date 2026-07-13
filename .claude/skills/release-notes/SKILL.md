---
name: release-notes
description: リリース工程の「リリースノート確定」（release ブランチ作成・main 反映の前）でリリースノート生成と CHANGELOG.md の版確定（[Unreleased]→版番号）を行うとき使用する。「リリースノートを作って」「CHANGELOGを生成して」「変更履歴をまとめて」が発火ワード。リリース工程一式（総合テスト〜タグ〜戻しマージ）のタスク敷設は /release-tasks（本スキルはその中の「リリースノート確定」を担う）。リリース前のスタック再同期（自動化・CLAUDE.md・プロファイルの更新）は /project-resync。
disable-model-invocation: true
---

直近リリース以降の Conventional Commits を集計し、`CHANGELOG.md` の `[Unreleased]` を版番号に確定して、SemVer タグ運用に沿ったリリースノートを出力します（タグ付け・push は実行しない）。

> **リリース工程一式のタスク敷設は `/release-tasks`**（リリース判定→総合テスト→受入チェック→リリースノート確定→release ブランチ→`main` マージ＋タグ→`develop` への戻しマージ→リリース後スモーク→マニュアル・ドキュメント最終確認）。本スキルはその中の「リリースノート確定」工程を担う（単体でも実行できる）。

## 前提（git-workflow.md より）

- `release/*` → `main` マージ時に `vMAJOR.MINOR.PATCH` 形式のタグを打つ。
- コミットは Conventional Commits 形式（`feat`/`fix`/`docs`/`refactor`/`perf`/`test`/`chore`/`build`/`ci` など）。
- Breaking Change は `!`（例: `feat(api)!: ...`）。

## 手順

### 1. 直前のリリースタグを特定

```bash
git fetch origin --tags
git tag --list 'v*' --sort=-v:refname | head -5
LAST_TAG=$(git tag --list 'v*' --sort=-v:refname | head -1)
echo "直前タグ: ${LAST_TAG:-（タグなし: 全履歴を対象）}"
```

タグが無い場合は最初のリリースとして全履歴を対象にする。

### 2. 対象範囲のコミットを取得

リリースノート確定は `main` 反映前＝`develop` 上で行う（`/release-tasks` の工程順）ため、集計の基準はデフォルトブランチ（CLAUDE.md「プロジェクト設定」の `defaultBranch`、未設定なら `develop`）とする：

```bash
TARGET=develop   # ← CLAUDE.md の defaultBranch に合わせる
RANGE="${LAST_TAG:+$LAST_TAG..}origin/$TARGET"   # タグがあれば LAST_TAG..origin/$TARGET、なければ全体
git log $RANGE --no-merges --pretty=format:'%h%x09%s'
```

`$ARGUMENTS` に範囲やバージョンの指定（例: `v1.1.0` や `v1.0.0..v1.1.0`）があればそれを優先する。マージ後の事後生成や hotfix リリース（`develop` への戻しマージ前は `develop` 基準では拾えない）では、`LAST_TAG..origin/main` 等を範囲に指定する。

### 3. type 別に分類

各コミットの subject を Conventional Commits の type で分類する：

| 見出し | 対象 type |
|--------|----------|
| ⚠️ Breaking Changes | `!` 付き、または `BREAKING CHANGE:` を含むもの |
| ✨ 新機能 | `feat` |
| 🐛 バグ修正 | `fix` |
| ⚡ パフォーマンス | `perf` |
| ♻️ リファクタリング | `refactor` |
| 📝 ドキュメント | `docs` |
| 🔧 その他 | `chore`/`build`/`ci`/`test`/`style` |

- scope（`feat(api):` の `api`）があれば各項目に併記する。
- `revert` は元コミットと併せて注記する。

### 4. 次バージョンの提案

直前タグからの SemVer 増分を提案する（最終判断はユーザー）：

- Breaking Change あり → **MAJOR** 増（`v2.0.0`）
- `feat` あり（Breaking なし） → **MINOR** 増（`v1.1.0`）
- `fix`/`perf` のみ → **PATCH** 増（`v1.0.1`）

### 5. CHANGELOG.md を確定する（ファイルに書き込む）

`CHANGELOG.md` は Keep a Changelog 形式。開発中は `dev-tasks` の「ドキュメント更新」タスクが `## [Unreleased]` にエントリを貯めている。リリース時に本スキルが **`[Unreleased]` を版番号に確定**する（チャット出力だけでなく、ファイルに実際に書き込む）。

1. **`CHANGELOG.md` の有無を確認**。無ければ（雛形未生成・旧運用）、手順 3 の集計から新規作成する（`# Changelog` ＋ Keep a Changelog 形式・`## [Unreleased]` の枠）。
2. **`[Unreleased]` と集計の差異を補完**：手順 2〜3 で集計したコミット（`feat`/`fix`/`perf` 等）が `[Unreleased]` から漏れていれば追記を提案する（マージ毎追記の取りこぼし救済）。差分をユーザーに提示してから反映する。
3. **版番号に確定**：`## [Unreleased]` の見出しを `## [v{提案バージョン}] - {YYYY-MM-DD}` に書き換え、その**上に新しい空の `## [Unreleased]`** を作る。
   ```markdown
   ## [Unreleased]

   ## [v{提案バージョン}] - YYYY-MM-DD
   ### ⚠️ Breaking Changes
   - (api) ... (`abc1234`)
   ### ✨ 新機能
   - (exp) ... (`def5678`)
   ### 🐛 バグ修正
   - ...
   ### ⚡ パフォーマンス / ♻️ リファクタリング / 📝 ドキュメント / 🔧 その他
   - ...
   ```
4. **書き込み前にユーザー承認**：確定内容（版番号・日付・各セクション）を提示し、承認を得てから `CHANGELOG.md` に書き込む（版の確定はリリースの区切りなので 1 度確認する）。

### 6. リリース判定チェックリスト

タグ作成を案内する前に、以下をユーザーと確認する（未充足の項目があれば指摘し、充足するまでタグ案内を保留する）：

- [ ] CI が緑（対象ブランチの最新コミットで全チェック成功）
- [ ] `CHANGELOG.md` の版確定が承認済み（手順 5）
- [ ] 総合テスト（システムテスト）の実行記録がある（`docs/test/release-vX.Y.Z-system-test.md`。テストレベルの定義は `.claude/rules/testing-strategy.md`）
- [ ] （マイグレーションを含むリリースのみ）リハーサル実施・破壊的変更の有無・適用手順・切戻し（ロールバック）条件を確認した（`.claude/rules/operations.md`「データ移行」の安全ゲート＝リハーサルを飛ばさない・切戻し条件を決めてから切替。`docs/design/migration.md`・`commands.migration`）
- [ ] 操作マニュアル・運用ガイド（`docs/操作マニュアル.md`・`docs/運用ガイド.md`）の更新要否を確認した（`.claude/rules/operations.md`「マニュアル体系」）
- [ ] デプロイ後スモーク（リリース後に疎通確認する主要導線）の項目を確認した（実施は `/release-tasks` の「リリース後スモーク」タスク）

### 7. タグ作成コマンドを案内（実行はしない）

```bash
git tag v{提案バージョン}
git push origin v{提案バージョン}
```

> タグ push（`v*`）を起点にデプロイ雛形 CI（release-deploy）が起動する構成では、push の前に手順 6 のチェックリストが満たされていることを確認する（`.claude/rules/git-workflow.md`「バージョンタグ（SemVer）」）。

## リリース前の再同期案内（必ず提示）

リリースはスタックの節目になりやすい。直近サイクルで言語・フレームワーク・ビルド系・構成が変わっていれば、リリース前に `/project-resync` を実行して、自動化（hooks/サブエージェント/スキル/プラグイン/MCP）・CLAUDE.md・プロジェクトプロファイル（`commands`/`checks`/`languages` 等）を現行スタックへ再同期しておくと、次サイクルが最新の設定で始められる。

> このスキルからは `/project-resync` を自動起動しない（双方とも `disable-model-invocation: true`）。上記を案内し、利用者が `/project-resync` を実行する。

## 注意

- このスキルは `CHANGELOG.md` の確定書き込みまで行うが、**タグ付け・push は実行しない**（手順 7 のコマンド提示に留め、ユーザーがレビューしてから実行）。`disable-model-invocation: true`＝手動発火のみ（自動では起動しない）。
- 該当コミットが0件で、かつ `CHANGELOG.md` の `[Unreleased]` にもエントリが無い場合のみ「直前リリース以降の変更なし」と報告する。0件なのに `[Unreleased]` が埋まっている場合は、集計範囲の取り違え（`main` 基準で見ていないか）を疑う。
