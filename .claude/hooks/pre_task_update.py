#!/usr/bin/env python3
"""PreToolUse (TaskUpdate) フック — タスク completed 時の証拠確認リマインダー（非ブロック）。

TaskUpdate で status を "completed" にしようとした瞬間に、完了条件（AC/DoD）の証拠を
会話で提示済みかを確認するリマインダーを Claude のコンテキストへ注入する
（.claude/rules/alignment.md の大原則＝証拠ベース完了の機械的な歯止め）。

- ブロックはしない（常に exit 0）。リマインダーは hookSpecificOutput.additionalContext
  （PreToolUse の非ブロック警告の公式出力形式）で stdout に単一 JSON として出す。
  permissionDecision は出さない＝通常の許可フローを変えない。
- status が "completed" 以外（in_progress 等）・status 無しでは完全沈黙（exit 0・出力なし）。
- project-profile.json 非依存・常時有効（post_edit のローマ字検知と同格）。
- stdin の JSON 解析失敗・想定外の payload は安全側で沈黙（exit 0）。
- 出力の UTF-8 固定・「tool_input 優先＋top-level フォールバック」の payload 解釈は
  他フック（pre_edit/post_edit）と共通の作法。
"""
import sys
import json

# 出力を UTF-8 に固定（Windows の既定エンコーディングでの文字化け回避）
try:
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
except Exception:
    pass

REMINDER = (
    "TaskUpdate: status を completed に変更しようとしている。"
    "このタスクの【完了条件】/DoD の証拠（コマンド出力・差分・URL 等）を"
    "直前の会話で提示したか確認する。提示していなければ completed にせず、"
    "証拠を先に示す（.claude/rules/alignment.md の大原則）。"
)


def main():
    try:
        payload = json.load(sys.stdin)
    except Exception:
        return 0  # payload が読めない＝想定外の起動は安全側で沈黙

    # 実 Claude Code は status を tool_input にネストする（top-level は後方互換フォールバック）。
    tool_input = payload.get("tool_input") or {}
    status = tool_input.get("status") or payload.get("status")

    if status != "completed":
        return 0  # completed 以外（in_progress 等・status 無し）は完全沈黙

    # 非ブロックのリマインダー: exit 0 のまま additionalContext で Claude のコンテキストへ注入する。
    print(json.dumps({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "additionalContext": REMINDER,
        }
    }, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    # 非ブロックの補助ガード: 想定外の入力・環境でもタスク操作を阻まない（常に exit 0）。
    try:
        sys.exit(main())
    except Exception:
        sys.exit(0)
