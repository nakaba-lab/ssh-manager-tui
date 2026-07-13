# 環境バリデーション

> 対象フロー: 新規のみ ｜ 種別: auto

以下を確認し、結果を表（OK / 未設定）で提示する。各コマンドは存在確認のみ。

| 項目 | 確認コマンド |
|------|-------------|
| Git ユーザー設定 | `git config user.name` / `git config user.email` |
| VCS CLI（GitHub） | `gh --version` / `gh auth status` |
| VCS CLI（GitLab） | `glab --version` / `glab auth status` |
| JS ランタイム | `npx --version` |
| Node.js / npm（docs サイト） | `node --version` / `npm --version`（docs サイト＝Astro のビルド・設計書スキーマ検証に必須。`docs-site/package.json` の `engines.node` = `>=22.0.0`。推奨は Active LTS の Node 24。docs サイトを使わないなら不要＝`docsSite.enabled` を false に） |
| Python ランタイム | `uvx --version` |
| Google Chrome 本体 | OS に応じて実行ファイルの存在（Web の場合に必要） |
| LSP / プラグイン | （**言語確定後に確認**）使用言語の LSP プラグインが `.claude/settings.json` で有効か。使用言語は「ヒアリング充足ループ」タスクで決まるため、ここで未定ならこの行は「保留」と明示してよい |
| hook 実行環境 | `python3 --version`（フックの実行に必要） |

- **不足あり** → ユーザーに導入を促す。プラグインを追加・有効化した場合は **`/plugin` での再読込（または Claude Code の再起動）を依頼**し、完了後にこのチェックへ戻る。
- VCS CLI は github / gitlab の **両方の存在を確認しておけば良い**（どちらを使うかはヒアリングで確定）。
- **LSP はプラグイン有効化だけでは動かない**: 対応する言語サーバ実体が PATH 上に無いと、有効化済みでも無言で機能しない。各 LSP プラグインの README が案内する導入物を入れる（例: typescript-lsp は `typescript-language-server`、pyright-lsp は `pyright`〔同梱の `pyright-langserver` が PATH に入る〕、jdtls-lsp は PATH 上の `jdtls` ラッパー）。使用言語の LSP を使うなら、この実体の導入有無も合わせて確認する（未導入なら導入を促すか「後回し」と明示判断させる）。

`【完了条件】LSP 行を除く全項目が OK、または未設定項目に対し利用者が「このまま進む」と明示承認した（確認表を提示済み）。`
