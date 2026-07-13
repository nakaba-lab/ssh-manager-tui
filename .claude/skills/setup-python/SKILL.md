---
name: setup-python
description: Python プロジェクトに Ruff（Lint＋Format）を新規セットアップするとき使用する。「Ruff を設定して」「Python の Lint を入れて」「Python のフォーマットを設定して」が発火ワード。JS/TS の ESLint・Prettier 導入は /setup-js。既存設定の修正や特定の Lint エラー修正では使わない。
---

Python プロジェクトに Ruff（Lint + Format）を新規セットアップします。

## 手順

### 1. 対象ディレクトリを確認

`$ARGUMENTS` が指定された場合はそのディレクトリ、なければカレントディレクトリを対象にする。

### 2. Ruff の導入

uv を使う場合（推奨）:

```bash
uv add --dev ruff
```

pip の場合:

```bash
pip install ruff
```

### 3. 設定ファイルを生成する

`ruff.toml`（または `pyproject.toml` の `[tool.ruff]`）をプロジェクトルートに作成する：

```toml
line-length = 100
target-version = "py311"

[lint]
select = ["E", "F", "I", "UP", "B", "SIM"]  # pycodestyle, pyflakes, isort, pyupgrade, bugbear, simplify
ignore = []

[format]
quote-style = "double"
```

### 4. 動作確認

```bash
uvx ruff check .        # Lint（uv 未使用なら ruff check .）
uvx ruff format .       # Format
uvx ruff check . --fix  # 自動修正
```

### 5. CI 統合（オプション）

GitHub Actions 用の例：

```yaml
- name: Lint with Ruff
  run: uvx ruff check . && uvx ruff format --check .
```

### 6. プロファイルへ反映

CLAUDE.md「プロジェクト設定」と `.claude/project-profile.json` の `commands.lint`（`uvx ruff check .`）・`commands.format`（`uvx ruff format .`）を更新する。
編集後に自動 Lint したい場合は `checks` に追加できる：

```json
{ "match": "**/*.py", "command": "uvx ruff check", "cwdFromRoot": true, "timeout": 60 }
```

設定完了後、適用した内容と次のステップを報告する。
