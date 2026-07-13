# Git ワークフロー規則

## ブランチ戦略（Git Flow）

| ブランチ     | 役割                   | 派生元    | マージ先           |
| ------------ | ---------------------- | --------- | ------------------ |
| `main`       | 本番リリース済みコード | —         | —                  |
| `develop`    | 開発統合ブランチ       | `main`    | —                  |
| `release/*`  | リリース準備           | `develop` | `main` + `develop` |
| `hotfix/*`   | 本番緊急修正           | `main`    | `main` + `develop` |
| `feature/*`  | 機能開発               | `develop` | `develop`          |
| `fix/*`      | バグ修正               | `develop` | `develop`          |
| `docs/*`     | ドキュメント更新       | `develop` | `develop`          |
| `refactor/*` | リファクタリング       | `develop` | `develop`          |
| `perf/*`     | パフォーマンス改善     | `develop` | `develop`          |
| `test/*`     | テスト追加・修正       | `develop` | `develop`          |
| `chore/*`    | ビルド・設定・雑務     | `develop` | `develop`          |

> **hotfix の起点（本番障害）**: 稼働中の本番に影響する障害は bug ではなく `incident` テンプレート（`.github/ISSUE_TEMPLATE/incident.md`／`.gitlab/issue_templates/incident.md`）で起票し、暫定対応（応急処置）を記録したうえで、恒久対応を fix Issue として `hotfix/*` で実装する。事後はポストモーテム（根本原因・再発防止の仕組み化）まで行う。対応フロー（重大度 SEV1-3・トリアージ・ポストモーテム様式）の正は `.claude/rules/operations.md`。

## ブランチ命名

```
<type>/<issue-number>-<short-description>
```

例：
- `feature/42-add-login-page`
- `fix/123-fix-null-pointer`
- `docs/56-update-api-docs`
- `refactor/78-extract-service-layer`
- `perf/34-optimize-database-queries`
- `test/67-add-user-service-tests`
- `chore/90-upgrade-dependencies`
- `release/1.2.0`（`release/*` のみ例外。issue 番号の代わりにバージョン番号を使う）
- `hotfix/99-fix-auth-bypass`

## コミットメッセージ

コミットメッセージは必ず Conventional Commits 形式にすること：

```
<type>(<scope>): <description>

<body: WHY と影響範囲を記述（原則必須）>

<footer: Refs #XXX など（任意）>
```

**body は原則必須**（WHY と影響範囲。自明な変更でも 1 文は書く）。**footer はコミットでは `Refs #<issue>`（追跡用の参照）**を使い、**Issue を閉じる `Closes #<issue>` は PR/MR 本文に書く**（feature→develop は squash マージのため、各コミットに `Closes` を撒くと PR/MR 本文の `Closes` と重複する）。

> **設計書フッタ（`Design: none`）**: `feat`/`fix` コミットは `docs/design/`（生きた設計書）配下の更新を**同じコミットに含める**こと（`commit-msg` フックが未更新を `exit 1` で拒否する）。設計変更を伴わない `feat`/`fix`（typo・内部リファクタ等）は body に `Design: none` の行を書けば通る。`--amend` で設計書を再ステージしない場合も「未更新」扱いになるため、`Design: none` か再ステージが要る。詳細は `.claude/rules/design-doc.md`。

### コミットの作り方（多行を落とさない）

1 行だけの `git commit -m "<subject>"` は body が落ちるため使わない。全文を一時ファイルに書いて `-F` で渡す：

```bash
MSG="$(git rev-parse --git-dir)/COMMIT_MSG"
cat > "$MSG" <<'EOF'
feat(auth): ログイン API を追加

未認証ユーザーの導線が無かったため追加。/login 経路に影響する。

Refs #42
EOF
git commit -F "$MSG"
```

> **強制（`commit-msg` フック）**: `git config core.hooksPath .githooks` を設定すると、出荷の `.githooks/commit-msg` が subject 形式（`<type>(<scope>): …`・scope 必須）と body 必須を検証し、不適合なら**コミットを拒否**する。さらにブランチ名のチケット番号から `Refs #N` を自動補完する。挙動は `.claude/project-profile.json` の `commitMessage`（`enabled`/`requireScope`/`requireBody`/`bodyExemptTypes`/`footer`/`ticketBranchPattern`）で調整できる。`/project-setup` が初回コミット後にこの設定を行う。

> **注意（`Closes #N` の自動クローズはデフォルトブランチ限定）**: コミット／PR・MR の `Closes #N` による Issue 自動クローズは、**リポジトリのデフォルトブランチへのマージ時のみ**発火し、閉じるのは当該 L2 Issue 1 件だけ（Milestone・子 Sub-issue / Task はカスケードしない）。Git Flow では `feature/*`・`fix/*` を `develop` にマージするため、デフォルトブランチが `main` のままだと自動クローズが効かない。`Closes #N` は linked-PR/MR 表示と default=develop 時の自動クローズのために残しつつ、マージ後は `/github-finalize`・`/gitlab-finalize`（`vcsHost` に応じて）で Issue＋子＋（該当時）Milestone を明示クローズする（`/dev-tasks` の「クローズ＆ファイナライズ」タスクが参照）。

| タイプ     | 説明                                                   |
| ---------- | ------------------------------------------------------ |
| `feat`     | 新機能                                                 |
| `fix`      | バグ修正                                               |
| `docs`     | ドキュメントのみの変更                                 |
| `style`    | コードスタイル変更（フォーマット等、動作に影響しない） |
| `refactor` | バグ修正でも機能追加でもないコード変更                 |
| `perf`     | パフォーマンス改善                                     |
| `test`     | テストの追加・修正                                     |
| `chore`    | ビルド・設定・雑務                                     |
| `build`    | ビルドシステムや依存関係の変更                         |
| `ci`       | CI 設定の変更                                          |
| `revert`   | 以前のコミットの取り消し                               |

### Breaking Change

後方互換性のない変更は `!` を付ける：

```
feat(api)!: change authentication endpoint response format
```

### ブランチタイプとコミットタイプの対応

| ブランチタイプ | 主なコミットタイプ       | 備考                                           |
| -------------- | ------------------------ | ---------------------------------------------- |
| `feature/*`    | `feat`                   |                                                |
| `fix/*`        | `fix`                    |                                                |
| `docs/*`       | `docs`                   |                                                |
| `refactor/*`   | `refactor`               |                                                |
| `perf/*`       | `perf`                   |                                                |
| `test/*`       | `test`                   |                                                |
| `chore/*`      | `chore` / `build` / `ci` |                                                |
| `hotfix/*`     | `fix`                    | ブランチ名は `hotfix`、コミットは `fix`        |
| `release/*`    | `chore`                  | バージョン更新・CHANGELOG 等                   |
| —              | `style` / `revert`       | 独立ブランチは作らず、作業中のブランチに含める |

## バージョンタグ（SemVer）

`release/*` → `main` マージ時に `vMAJOR.MINOR.PATCH` 形式のタグを打つ：

| バージョン | 増やすタイミング                        | 例              |
| ---------- | --------------------------------------- | --------------- |
| MAJOR      | 後方互換性のない変更（Breaking Change） | `v2.0.0`        |
| MINOR      | 後方互換な新機能追加                    | `v1.1.0`        |
| PATCH      | 後方互換なバグ修正                      | `v1.0.1`        |
| PRERELEASE | プレリリース版                          | `v1.0.0-beta.1` |

```bash
git tag v1.0.0
git push origin v1.0.0
```

> **タグ→デプロイ接続**: タグ push（`v*`）を起点に、同梱 CI のデプロイ雛形（`.github/workflows/release-deploy.yml`／`.gitlab/ci/deploy.yml`）が起動する。デプロイ先固有の手順への接続はプロジェクトの CI に委ねる（`/project-setup` が `commands.deploy` の確定時に有効化する。デプロイしないプロジェクトでは削除してよい）。
>
> **リリース工程のタスク敷設は `/release-tasks`**: リリース判定→総合テスト→受入チェック→リリースノート確定（`/release-notes`）→release ブランチ→`main` マージ＋タグ→`develop` への戻しマージ→リリース後スモーク→マニュアル・ドキュメント最終確認、の一連の工程は `/release-tasks` がタスクとして敷設・進行する（本ファイルはブランチ・タグ・マージの規則のみを定め、工程の手順は重複定義しない）。

## マージ戦略

- `feature/` `fix/` `docs/` `refactor/` `perf/` `test/` `chore/` → `develop`：**Squash Merge**
- `release/*` → `main`：**Merge Commit**（タグを打つ）
- `release/*` → `develop`：**Merge Commit**
- `hotfix/*` → `main`：**Merge Commit**（タグを打つ）
- `hotfix/*` → `develop`：**Merge Commit**

> **戻しマージを忘れない**: `release/*`・`hotfix/*` は `main` だけでなく **`develop` にも**マージする（忘れると次リリースから修正が落ちる・コンフリクトの温床になる）。`/ship` が `main` へのマージ完了後に戻しマージの実行を `AskUserQuestion` で確認し、リリース工程全体では `/release-tasks` の「develop への戻しマージ」タスクが担う。

## 依存ロックファイル

**ロックファイルはコミットする**（`.gitignore` に入れない）。再現可能ビルドの前提であり、`npm ci` などの再現コマンド・CI・依存監査（`security-reviewer`・OSS ライセンス一覧生成）がコミット済みのロックファイルを前提にする。**ロックファイルはビルド成果物ではない**（成果物は `dist/`・`build/` 等＝`.gitignore` 対象。混同して lockfile を ignore しない）。

| パッケージマネージャ | ロックファイル |
| ---------------------- | -------------- |
| npm / pnpm / yarn      | `package-lock.json` / `pnpm-lock.yaml` / `yarn.lock` |
| uv / poetry            | `uv.lock` / `poetry.lock` |
| Cargo / Bundler / Go   | `Cargo.lock` / `Gemfile.lock` / `go.sum` |
| Gradle                 | `gradle.lockfile`（依存ロックを有効化している場合） |

- **uv の注意**: ランタイム依存は `pyproject.toml` の `[project].dependencies` に置き（`uv add <pkg>`）、`requirements.txt` と二重管理しない（`uv sync` は `requirements.txt` を読まない＝乖離の温床。配布が要るなら `uv export` の生成物として扱う）。`uv.lock` はコミットする（uv 公式推奨）。
- **Go の注意**: `go.mod`（依存・バージョン宣言）と `go.sum`（チェックサム DB）は**両方コミットする**（`go.sum` だけでなく `go.mod` も成果物ではない）。
- **例外（配布ライブラリ）**: 公開ライブラリの `Cargo.lock` はコミットしない（Rust 公式: バイナリ＝コミット／ライブラリ＝除外）。同種の判断が要る他スタックはその言語の定石に従う（種別が `library` のときに確認する）。
- **ロックファイルのコンフリクト**: rebase でロックファイルがコンフリクトしたら手で再生成する（`npm install`／`uv lock` 等で作り直してステージ）。`.gitattributes` の `merge=union` は行追記型テキスト（`CHANGELOG.md`）向けで、構造を持つロックファイルには使わない。

## ロールバック（リリース・マージの切り戻し）

`git push --force` は deny 済み（`.claude/settings.json` の permissions）。履歴の書き換え（reset + force push）では戻さず、**revert コミットを前進的に積んで戻す**のが原則。リリース後の障害対応フロー（incident 起票→暫定対応→hotfix→ポストモーテム）は `.claude/rules/operations.md` を参照。

### 切り戻し判断基準（前進修正 vs リリース取り消し）

| 状況 | 手段 | 理由 |
|------|------|------|
| 原因が特定済みで修正が小さい（本番影響が限定的） | **前進修正（`hotfix/*`）** | 修正して進む方が早く、履歴も自然に前進する |
| 原因不明・修正が大きい・本番影響が継続している | **リリース取り消し（`git revert`）→ 修正版タグ** | まず止血して直前の健全な状態に戻し、恒久対応は改めて `hotfix/*`・`feature/*` で行う |
| 特定機能だけが問題（リリース全体は健全） | 当該機能の **squash コミットのみ `git revert`** | リリース全体を巻き戻さず最小限で戻す |

### squash merge コミットの revert 手順

`feature/*`・`fix/*` → `develop` は Squash Merge のため、マージ結果は**通常の単一コミット**になっている。マージコミットではないので **`-m` の親指定なし**でそのまま revert できる：

```bash
git log --oneline develop   # 取り消したい squash コミットの SHA を特定
git revert <SHA>            # 通常コミットとして revert（コミット type は revert）
```

- revert も保護ブランチへ直接 push せず、**通常のブランチ → PR/MR → マージのフロー**に乗せる（ブランチ例: `fix/<issue>-revert-<対象>`）。
- **develop / main 両系への波及に注意**: `main` に入った変更（release/hotfix 経由）を revert する場合、同じ変更が `develop` にも存在する。`main` だけ revert すると次リリースで変更が復活するため、release/hotfix と同じ要領で **`main` + `develop` の両方に revert を反映する**。
- `release/*`・`hotfix/*` → `main` は Merge Commit のため、マージコミット自体を revert する場合のみ `git revert -m 1 <マージコミット SHA>`（第一親＝`main` 側を残す）と親指定が要る。

### リリースタグの扱い

- **打ったタグは消さない・付け替えない**のが原則。問題のあるリリースは revert・修正を積んで**修正版タグ（PATCH 増）を前に進める**（利用者・CI が既にタグを参照している可能性があり、同名タグの差し替えは参照先の食い違い事故を生む）。
- タグ直後に気づいた**誤タグ**（タイポ・対象コミット違い）で、まだ誰も参照していないと確認できる場合に限り取り下げてよい：

```bash
git tag -d v1.2.0                    # ローカルのタグを削除
git push origin :refs/tags/v1.2.0    # リモートのタグを削除（誤タグの取り下げに限る）
```

> タグ push でデプロイ雛形 CI が起動する構成（上記「バージョンタグ（SemVer）」）では、誤タグの取り下げ前に当該パイプラインの停止・影響も確認する。

## git worktree 運用

### worktree の作成

Issue のタスクごとに worktree を作成する。ディレクトリはリポジトリの兄弟ディレクトリに配置し、ブランチ名のスラッシュをハイフンに変換した名前にすること：

```
ブランチ: feature/42-add-login-page
ディレクトリ: ../feature-42-add-login-page
```

> **派生元ブランチ**: 以下の例では `develop` を使う。プロジェクトのデフォルトブランチは CLAUDE.md の「プロジェクト設定」節（`defaultBranch`）に従う（トランクベース運用なら `main` などに読み替える）。

事前に `git fetch origin` を実行してから：

```bash
git worktree add ../feature-42-<説明> -b feature/42-<説明> origin/develop
```

### 直列タスクの rebase

あるタスクが前のタスクの成果物に依存している場合、前タスクが `develop` にマージされてから以下を実行する：

```bash
git fetch origin
git rebase origin/develop
```

Squash Merge 運用なので `merge` より `rebase` を推奨（マージコミットの積み重ねを避けるため）。

### 並行開発のコンフリクト規律

全 Issue が追記する共有ファイル — `CHANGELOG.md`・`docs/design/README.md`（索引）・`docs/用語集.md` — は、rebase でコンフリクトしたら「**両方を残す**」が原則（各 Issue の追記はいずれも残すべき変更であり、どちらか一方を選ぶ性質のコンフリクトではない）。

- `CHANGELOG.md` は `.gitattributes` の `merge=union` が行単位の追記を自動統合する（コンフリクトマーカー自体が出ない）。ただし**同一行の編集・隣接行の並び替えは自動統合の対象外**のため手動で解消する（このときも両方の追記を残す）。
- 索引（`docs/design/README.md`）・用語集（`docs/用語集.md`）には `merge=union` を適用していない（表・リンク構造のため機械統合が崩れを生みうる）。コンフリクトマーカーが出たら両側の行を手で残して解消する。

### worktree のクリーンアップ

フェーズ完了後は `/worktree-cleanup` スキルを使うこと。
Squash Merge のためローカルブランチが「未マージ」と判定される場合は `git branch -D` で強制削除する（リモートへのマージを確認済みであること）。
