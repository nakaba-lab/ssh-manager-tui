# CLAUDE.md 改善キュレーション（減算）

> 対象フロー: 新規・既存とも ｜ 種別: 【人間ゲート】

1. `Skill("claude-md-management:claude-md-improver")` を実行する。品質レポートを出した後、**承認を得て CLAUDE.md を自身で編集する**スキル（recommender と違い自分で書き込む。自前の承認機構を持つ）。
2. **減算ゲート（`AskUserQuestion`）**：improver 自身の承認と二重化させないため、テンプレ側ゲートは**「除外したい個別提案の指定」だけ**を問う（全採否を問い直さない）。
   - header: `CLAUDE.md 改善`
   - question:「improver が CLAUDE.md の改善案 M 件を提示しました（追加◯ / 修正◯ / 削除◯）。除外したい改善案を選んでください（複数選択可・未選択＝全件を適用）。残った案を improver が CLAUDE.md に反映します。」
   - options:**M ≤ 4** は各提案を multiSelect（除外する項目）／**M ≥ 5** は除外する**種別**（追加案 / 修正案 / 削除案）→種別内 multiSelect。選択肢は improver の実出力から動的生成。
   - **保持対象（プロジェクト設定節・標準ワークフロー/各規則の説明・Hooks/プラグイン説明・コーディング規約）に触れる削除案は、除外候補としてハイライト**して過剰除去を防ぐ。
3. 除外集合を確定 → improver に「除外を除いた集合で CLAUDE.md を編集」させる（improver が自分で書く。Claude が代筆しない。improver の承認はこの残集合に対して 1 回だけ消費）。編集対象は **CLAUDE.md 系のみ**（`settings.json`・agents・hooks・skills は触らない＝「推奨キュレーション＋適用」タスクの担当）。CLAUDE.md は `.claude/` 外なので auto mode の制約は受けない。

`【完了条件】improver の提案が利用者の除外指定を反映して CLAUDE.md に適用され、レポートが放置になっていないことを提示。【証拠なしに completed 禁止】除外指定の回答を得る前に CLAUDE.md を確定編集しない・completed にしない。`
