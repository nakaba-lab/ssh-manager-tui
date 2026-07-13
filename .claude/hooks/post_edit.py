#!/usr/bin/env python3
"""PostToolUse (Edit|MultiEdit|Write) フック — project-profile.json 駆動・言語非依存。

- ローマ字識別子の検知（言語非依存・常時有効）
- profile.checks に定義された言語別チェック（型チェック・コンパイル等）を実行

checks が空なら（ローマ字検知以外は）完全 no-op。
チェックコマンドの実行ファイルが PATH に無ければ沈黙（未インストール環境でも安全）。
失敗・タイムアウトしてもフック自体は exit 0 に倒す（型/コンパイルエラーのみ stderr 警告）。
"""
import sys
import json
import subprocess
import re
import shutil
import shlex
import fnmatch
from pathlib import Path

# 出力を UTF-8 に固定（Windows の既定エンコーディングでの文字化け回避）
try:
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
except Exception:
    pass

# ローマ字識別子のブラックリスト（.claude/rules/naming-*.md と対応）
ROMAJI = {
    "syouhin", "torihiki", "kanri", "shori", "koumoku", "nyuuryoku",
    "shutsuryoku", "sakusei", "sakujyo", "henko", "kakunin", "ichiran",
    "meisai", "jouhou", "settei", "buhin", "chumon", "uriage",
}

# ローマ字検知の対象拡張子（言語非依存）
CODE_EXT = (
    ".java", ".ts", ".mts", ".tsx", ".js", ".jsx", ".py",
    ".go", ".rb", ".kt", ".cs", ".php", ".rs", ".scala", ".swift",
)


def norm(path):
    return path.replace(chr(92), "/") if path else path


def load_profile(start):
    """編集対象から上方向に project-profile.json を探す。戻り値は (profile, root)。"""
    base = Path(start).resolve() if start else Path.cwd()
    for directory in [base, *base.parents]:
        candidate = directory / ".claude" / "project-profile.json"
        if candidate.exists():
            try:
                return json.loads(candidate.read_text(encoding="utf-8")), directory
            except Exception:
                return {}, directory
    return {}, None


def main():
    try:
        payload = json.load(sys.stdin)
    except Exception:
        return 0

    # 実 Claude Code は file_path / new_string を tool_input にネストする。top-level 形式にも後方互換。
    tool_input = payload.get("tool_input") or {}
    path = tool_input.get("file_path") or payload.get("file_path") or payload.get("path") or ""
    normalized = norm(path)
    new_text = (
        tool_input.get("new_string") or tool_input.get("content")
        or payload.get("new_string") or payload.get("content") or ""
    )
    # MultiEdit は new_string/content を持たず、edits[] に複数の new_string を入れる。
    # ローマ字検知を MultiEdit でも効かせるため、edits[] の new_string も連結対象にする。
    edits = tool_input.get("edits") or payload.get("edits") or []
    if isinstance(edits, list):
        new_text = "\n".join(
            [new_text] + [e.get("new_string", "") for e in edits
                          if isinstance(e, dict) and e.get("new_string")]
        )
    profile, root = load_profile(path)

    # 1) ローマ字識別子の検知（言語非依存・常時有効）
    #    識別子を camelCase / snake_case で分割して語幹照合する（syouhinCount 等の複合語も検知）
    if normalized and normalized.endswith(CODE_EXT) and "node_modules" not in normalized:
        subwords = set()
        for token in re.findall("[A-Za-z_][A-Za-z0-9_]*", new_text):
            for part in re.findall("[A-Z]?[a-z]+|[A-Z]+(?![a-z])|[0-9]+", token):
                subwords.add(part.lower())
        bad = sorted(subwords & ROMAJI)
        if bad:
            print(
                f"WARNING: ローマ字識別子を検出: {bad}。"
                "英語名を使ってください（.claude/rules/naming-*.md 参照）"
                "（英語名は docs/用語集.md の対訳に従う。無ければ追記してから使う）",
                file=sys.stderr,
            )

    # 2) プロファイル駆動チェック（checks が空なら何もしない）
    for check in profile.get("checks", []):
        match = check.get("match", "")
        command = check.get("command", "")
        if not (normalized and match and command):
            continue
        if not fnmatch.fnmatch(normalized, match):
            continue
        try:
            argv = shlex.split(command)
        except Exception:
            continue
        if not argv:
            continue
        resolved = shutil.which(argv[0])
        if resolved is None:
            continue  # 実行ファイルが無い環境では沈黙（安全側）
        argv[0] = resolved  # Windows の .CMD/.BAT シム（npx/tsc 等）も直接起動できるよう解決済みフルパスに
        cwd = str(root) if check.get("cwdFromRoot") and root else None
        try:
            result = subprocess.run(
                argv, capture_output=True, text=True, cwd=cwd,
                timeout=check.get("timeout", 120),
            )
            if result.returncode != 0:
                print(
                    f"チェック失敗 [{match}]:"
                    "（checks の調整: .claude/project-profile.json の checks。"
                    "コマンド・timeout を見直す）",
                    file=sys.stderr,
                )
                print((result.stdout + result.stderr)[-2000:], file=sys.stderr)
        except Exception:
            pass  # タイムアウト等は無視

    return 0


if __name__ == "__main__":
    sys.exit(main())
