# プロファイル確定（2 層同時更新）

> 対象フロー: 新規・既存とも ｜ 種別: auto

> `.claude/project-profile.json` に書き込むため、**auto mode はオフ**にしておく。

これまでに収集した値で **`.claude/project-profile.json`** を書き、**CLAUDE.md「プロジェクト設定」節**を同じ値で同期する（2 層がずれないよう必ず同時更新。**書き込みと同期を別タスクに分けない**）：

- `name` / `kind` / `languages` / `packageManager`
- `commands.build` / `commands.test` /（Web 等で開発サーバがあれば）`commands.dev` / `commands.lint` / `commands.format` /（あれば）`commands.migration` /（結合テストを単体と分離して持つなら）`commands.integrationTest` /（カバレッジを計測するなら）`commands.coverage` /（デプロイ先が確定していれば）`commands.deploy`（手動実行の手順コマンドでも可。未確定なら空のまま＝該当工程はスキップされる）
- `vcsHost`（github/gitlab）/ `defaultBranch` / `protectedBranches` / `frontendDir`（UI コードのあるディレクトリ。**ルート直下なら `"."`、UI を持たないなら `"none"` を明示**。kind=web で空（未確認）のまま確定しない）
- `checks`（言語に応じた編集後チェック。不要なら空のまま）：
  - TypeScript → `{ "match": "**/*.ts", "command": "npx tsc --noEmit", "cwdFromRoot": true }`
  - Python → `{ "match": "**/*.py", "command": "uvx ruff check", "cwdFromRoot": true }`
  - Go → `{ "match": "**/*.go", "command": "go build ./...", "cwdFromRoot": true }`
  - フロント a11y（JSX。要 `eslint-plugin-jsx-a11y`）→ `{ "match": "**/<frontendDir>/**/*.jsx", "command": "npx eslint", "cwdFromRoot": true }`（`.tsx` は別エントリで並べる。**`frontendDir` が `"."` のときは `**/*.jsx` を使う**＝`**/./**` はリテラル `/./` を要求し絶対パスに決して一致しない）
  - **複数言語のプロジェクトは言語ごとにエントリを並べる**（`checks` は配列）。
  - **ビジュアル回帰（スクリーンショット比較）は編集後 `checks` ではなく PR 前チェック**として回す（`playwright` 等。1 ファイル編集ごとに走らせると重いため）。`frontendDir` を `tsc` のスコープだけでなく a11y/視覚チェックにも結線する。
  - **各コマンドが参照する設定ファイルが「雛形構築」タスク（新規のみ）で揃っている**ことを確認してから入れる（無いとチェックが失敗する）。
  - **`match` は編集ファイルの絶対パスに対する Python `fnmatch`**（パス全体に一致する必要がある＝先頭・末尾とも暗黙にアンカーされる）。`fnmatch` は **globstar 非対応**で、`*` がパス区切り `/` も含めて何にでも一致する（`**` は実質 `*` と同じで特別な意味はない）。要点：
    - 先頭は `**/` か `*` で始める（絶対パスは `/home/...` や `C:/...` で始まるため）。`src/**/*.ts` のように具体パスで始めると絶対パスに一致せず**チェックが沈黙する**（無一致）。
    - ブレース展開（`{ts,tsx}`）は非対応。拡張子ごとにエントリを分ける。
    - **動作例**（編集ファイルの絶対パスが `/home/me/proj/frontend/src/app.ts` の場合）：`**/frontend/**/*.ts` は一致／`frontend/**/*.ts` は**不一致**。
    - **パターンの検証**：意図どおり当たるかは次の 1 行で確認できる（`True` なら一致）：
      `python3 -c "import fnmatch; print(fnmatch.fnmatch('/abs/path/to/frontend/src/app.ts', '**/frontend/**/*.ts'))"`
  - **サブディレクトリのツールを入れる場合**：`cwdFromRoot: true` は **リポジトリルート**で実行する。ツールや設定ファイルが `frontendDir` 等のサブディレクトリにあるなら、コマンド側でそのディレクトリを指定し（例: `npx tsc -p <frontendDir> --noEmit`。`timeout` は長めに）、`match` も先頭を `**/` にして `**/<frontendDir>/**/*.ts` のように絞る（`frontendDir` が `"."` の場合は絞り込み不要＝`**/*.ts` をそのまま使う）。
- `protectedGlobs`（自動生成ファイル等があれば。なければ空）
- **`commands.dev` を確定した場合**：frontend-reviewer／「ビジュアル/UX 確認」タスクが自律区間で許可プロンプトに阻まれないよう、`Bash(<devコマンド>*)` を `.claude/settings.json` の `permissions.allow` へ追記する**案を提示し、`AskUserQuestion` で承認を得てから**書き込む（無承認でワイルドカード許可を足さない。`.claude/` 配下＝auto mode オフ）。
- **同梱アプリ CI 雛形の有効化（プロファイル確定後）**：出荷物のアプリ CI 雛形（GitHub: `.github/workflows/app-test.yml`〔テスト/Lint・feat/fix のテスト必須チェック〕・`.github/workflows/release-deploy.yml`〔タグ起動のデプロイ〕／GitLab: `.gitlab/ci/app-test.yml`・`.gitlab/ci/deploy.yml`）は、`.claude/project-profile.json` の `commands` が空のうちは**安全に no-op**（skip して成功終了）で動く。ここで `commands.test`／`lint`／`build`（および `coverage`／`deploy`）が確定したら、各 YAML 先頭コメントの案内に従って言語セットアップ部（Node/Python/Java 等）を整えて有効化する。CI が不要（または当該ホストを使わない）なら該当ファイルを削除する（GitLab はルート `.gitlab-ci.yml` の該当 include 行も外す——snippet だけ消すと include が「local file does not exist」で落ちる）。判断（有効化／削除／後回し）は完了サマリに含める。

`【完了条件】project-profile.json と CLAUDE.md「プロジェクト設定」節の値が完全一致し、kind=web なら frontendDir が非空（"."／"none"／ディレクトリ名のいずれか）であり、checks が空でないなら各 match を絶対パス fnmatch で検証済みであること、およびアプリ CI 雛形の判断（有効化／削除／後回し）を提示。`
