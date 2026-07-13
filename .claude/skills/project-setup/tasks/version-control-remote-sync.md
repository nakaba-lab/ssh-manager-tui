# バージョン管理・リモート同期（push＝最終承認）

> 対象フロー: 新規・既存とも ｜ 種別: 【人間ゲート】

> プロファイル（「プロファイル確定」タスク）・再生成 README（「README 再生成」タスク）を先に確定してからここに来るため、初回コミット／push にプロファイルと再生成 README が含まれる。

1. **Git 管理の確認**：
   ```bash
   git rev-parse --is-inside-work-tree 2>/dev/null
   ```
   - **管理外（No）**（新規で多い）→ 初期化する（`defaultBranch` が develop の Git Flow 運用）：
     ```bash
     git init -b main          # 本番ブランチ main を初期ブランチにする（release/hotfix の合流先）
     git add -A
     git commit -m "chore: initial commit"   # または空コミット: git commit --allow-empty -m "initial commit"
     git switch -c develop                    # main から develop を派生（以後の既定の開発ブランチ）
     ```
     > `git init -b main` は git 2.28+ が必要。古い git では `git init && git add -A && git commit -m ... && git branch -m main` で代替する。
   - **管理済み（Yes）**（既存で多い）→ そのまま次へ（入れ子 `.git` の片付けは「雛形構築」タスク〔新規のみ〕で実施済み）。
   - **（新規・既存とも）git 管理が確立したら、初回コミットの後に git フックを有効化する**（コミットメッセージ形式の強制）：
     ```bash
     git config core.hooksPath .githooks
     chmod +x .githooks/commit-msg .githooks/pre-commit 2>/dev/null || true   # zip 展開で実行ビットが落ちるため付与（Windows は不要・失敗時も継続）
     ```
     > 以後のコミットは `.githooks/commit-msg` が形式（`<type>(<scope>): …`・scope 必須・body 必須）を検証し、ブランチ名のチケット番号から `Refs #N` を自動補完する（`.claude/rules/git-workflow.md` 参照）。設定を**初回コミットの後**に行うのは、初回 `chore: initial commit`（scope/body 無し）を自フックで弾かないため。**`core.hooksPath` は git config でありクローンには継承されない**ため、チームで共有する場合は各自が clone 後に `git config core.hooksPath .githooks` を再実行する（「README 再生成」タスクの再生成 README に明記する）。設定すると `commit-msg` に加えて **`.githooks/pre-commit`** も有効になり、`docs/design/` を編集したコミットで `docs-site` の `astro check`（設計書スキーマ検証）が走る（`docsSite` 駆動。`docs-site/node_modules` 未導入の worktree はフックが `npm ci` を案内する）。なお、この `core.hooksPath` 設定が完了すると SessionStart フック `check_hooks_setup.py` は次回セッションから沈黙する（未設定の間は「commit-msg フックが無効＝body・scope が強制されない」と警告する補助ガード）。
2. **リモートの確認**：
   ```bash
   git remote -v
   ```
   - **既に remote が設定されている場合は URL を照返して確認する**（`alignment.md` の照返し）: その remote が**テンプレートの配布元リポジトリを指していないか**を利用者に提示して確認し、指していれば push 前に `git remote set-url`／`git remote remove` で付け替える（zip 配布では remote は無いのが正常。clone 経路や流用で残った配布元 remote への誤 push＝テンプレ汚染を防ぐ二重防御。README の配布モデル注記と対）。
3. **リモート作成・push ゲート（`AskUserQuestion`）**：リモート作成・push は外部影響＝取り消せない操作なので、ここで利用者の最終承認を得る（dev-tasks の `/ship` と同型の人間ゲート）。
   - header: `リモート push`
   - question:「プロファイルを確定しました。リモートへ push しますか？（push は外部影響＝取り消せない操作です）新規は main・develop の両方、既存は存在するブランチを push します。」
   - options:「push する（main・develop／既存は存在ブランチを remote へ）／リモート作成のみ（push せず remote だけ作る）／何もしない」
4. 回答に応じて実行：
   - **push する**：リモートが無ければ先に作成（**`--push` を付けない**）→ push。
     - GitHub: `gh repo create <name> --private --source=. --remote=origin`
     - GitLab: `glab repo create <name>`（リモート名は `origin` に揃える）
     - ⚠️ `gh repo create --push` は **現在のブランチ（HEAD）だけ** を push するため使わない。`main`・`develop` を明示 push する：
       ```bash
       git push -u origin main      # 本番ブランチ（release/hotfix の合流先）
       git push -u origin develop   # 既定の開発ブランチ
       ```
     - **既存リポジトリで `develop` 等が無い（トランクベース運用）場合は、`git branch` で確認して存在するブランチだけ** push する。
   - **リモート作成のみ**：上記の作成だけ行い push しない。
   - **何もしない**：push もリモート作成もしない（明示判断として記録）。
5. **ブランチ保護の案内（push した場合）**：保護ブランチ（`develop`・`main`）に **required reviews（レビュー必須）と required status checks（CI の成功必須。`design-doc-check`・`app-test` 等を指定）**を設定するよう案内する。
   - GitHub: Settings → Branches → Branch protection rules（CLI なら `gh api -X PUT repos/<owner>/<repo>/branches/<branch>/protection` に必要な JSON を渡す）。
   - GitLab: Settings → Repository → Protected branches（レビュー必須は Settings → Merge requests の承認設定。`glab` に専用コマンドが無いため設定画面か API を案内する）。
   - 適用可否はチーム規模・ホストのプランに依存するため**案内に留める**（設定しない判断も可。push しなかった場合は対象外）。
   - **あわせて Secret scanning（秘密情報の push 保護）を案内する**：同梱 CI の `secret-scan`（gitleaks による PR/MR の差分スキャン。多段構成の正は `.claude/rules/operations.md`「秘密情報の管理」）に加え、ホスト側の検知を有効化する（プランによっては push の瞬間に止められる — 下記の制約参照）。これもプラン依存のため**案内に留める**。
     - GitHub: Settings → Code security の **Secret scanning ＋ Push protection**（public リポジトリは無料。private は有償アドオン〔GitHub Secret Protection〕）。
     - GitLab: パイプライン版 Secret Detection（`.gitlab-ci.yml` に `Security/Secret-Detection.gitlab-ci.yml` テンプレートを include・全ティア無料。同梱 `secret-scan` と併用可）。push の瞬間に止める Secret push protection は **Ultimate のみ**。
   - **あわせて CI 逃がしラベルを作成する（リモート作成・push 後。「リモート作成のみ」を選んだ場合も実施する）**：同梱 CI の逃がしラベル（`design:none`＝`design-doc-check`／`test:none`＝`app-test` の `test-required`）は、**リポジトリに事前作成されていないと PR/MR に付与できない**（本文行の `Design: none`／`Test: none` はラベル無しでも使える）。
     - GitHub: `gh label create design:none`・`gh label create test:none`（`--description` で用途〔設計書更新不要／テスト追加不要の逃がし〕の説明文を付ける推奨）。
     - GitLab: ラベル作成（`glab label create` または UI の Settings → Labels）を案内する。

`【完了条件】`core.hooksPath=.githooks` を設定済み（`git config --get core.hooksPath` で確認）、かつ main・develop（既存は存在ブランチ）が remote に push 済み（git ls-remote で確認。push した場合はブランチ保護の案内結果と CI 逃がしラベル〔design:none・test:none〕の作成結果も提示）、または利用者が「push しない／作成のみ」と明示判断した旨を提示（リモートを作成した場合は逃がしラベルの作成結果も含める）。【証拠なしに completed 禁止】push/不 push の判断と確認結果を提示する前に completed にしない。`
