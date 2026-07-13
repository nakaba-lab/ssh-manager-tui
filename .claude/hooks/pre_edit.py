#!/usr/bin/env python3
"""PreToolUse (Edit|MultiEdit|Write) フック — project-profile.json 駆動・言語非依存。

- protectedGlobs に一致するファイルの編集をブロック（自動生成物・適用済みマイグレーション等）
- protectedBranches への直接編集を警告（worktree 運用の逸脱検知）

protectedGlobs が空なら何もブロックしない。ただし出荷時の project-profile.json は
protectedBranches=["develop","main"] を持つため、それらのブランチ上での直接編集
（.claude/ 配下と CLAUDE.md は除外）は既定で警告する点に注意（完全な無音ではない）。
編集対象ファイルのパスから上方向に .claude/project-profile.json を探すため、
worktree やモノレポでも各サブツリーのプロファイルを正しく解決する。
"""
import sys
import json
import subprocess
import fnmatch
from pathlib import Path

# 出力を UTF-8 に固定（Windows の既定エンコーディングでの文字化け回避）
try:
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
except Exception:
    pass


def norm(path):
    """OS 差を吸収してパス区切りを / に正規化する。"""
    return path.replace(chr(92), "/") if path else path


def load_profile(start):
    """編集対象から上方向に project-profile.json を探して読み込む。"""
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

    # 実 Claude Code は file_path を tool_input にネストする。スモーク等の top-level 形式にも後方互換。
    tool_input = payload.get("tool_input") or {}
    path = tool_input.get("file_path") or payload.get("file_path") or payload.get("path") or ""
    normalized = norm(path)
    profile, root = load_profile(path)

    # 1) 保護グロブ（自動生成物など）→ ブロック（exit 2）
    for pattern in profile.get("protectedGlobs", []):
        if normalized and fnmatch.fnmatch(normalized, pattern):
            print(
                f"BLOCKED: {path} は保護対象です（project-profile.json の protectedGlobs）。"
                "直接編集できません。生成元を更新してください。"
                "（調整: .claude/project-profile.json の protectedGlobs。"
                "意図的な編集なら一時的にエントリを外す）",
                file=sys.stderr,
            )
            return 2

    # 2) 保護ブランチへの直接編集 → 警告（.claude/ 配下と CLAUDE.md は除外）
    #    CLAUDE.md は basename 一致で判定する（MYCLAUDE.md 等の部分一致誤除外を防ぐ）。
    protected = profile.get("protectedBranches", [])
    basename = normalized.rsplit("/", 1)[-1] if normalized else ""
    is_config_file = (
        normalized.startswith(".claude/") or "/.claude/" in normalized
        or basename == "CLAUDE.md"
    )
    if path and protected and not is_config_file:
        git_cmd = ["git", "branch", "--show-current"]
        if root is not None:
            # 編集ファイルが属する worktree のブランチを見る（プロセス cwd ではなく）
            git_cmd = ["git", "-C", str(root), "branch", "--show-current"]
        try:
            result = subprocess.run(git_cmd, capture_output=True, text=True)
            branch = result.stdout.strip()
        except Exception:
            branch = ""
        if branch in protected:
            print(
                f"WARNING: {branch} ブランチを直接編集しています。"
                "feature/fix の worktree で作業してください（/worktree-new）。"
                "（無効化/調整: .claude/project-profile.json の protectedBranches）",
                file=sys.stderr,
            )

    return 0


if __name__ == "__main__":
    sys.exit(main())
