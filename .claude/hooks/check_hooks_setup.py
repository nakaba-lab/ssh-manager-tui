#!/usr/bin/env python3
"""SessionStart フック — セットアップ漏れで無効化されているガードが無いか確認する。

テンプレートを適用した（project-profile.json の name が設定済みの）プロジェクトで、
次の 2 つを独立に検査し、該当するものをまとめて 1 つの警告として注入する:

1. commit-msg 規律: git の core.hooksPath が .githooks を指していない＝出荷の commit-msg
   フックが起動しない場合、Conventional Commits の body 必須・scope 必須が強制されない旨を警告
   （commitMessage.enabled=false の明示無効化プロジェクトはこの警告のみ対象外）。
2. ビジュアル検証パイプライン: kind が "web" なのに frontendDir が空（未確認）の場合、
   視覚 AC 起票・「ビジュアル/UX 確認」タスク・DoD（UI 変更時）・frontend-reviewer の全段が
   対象外として沈黙する旨を警告（"none"＝UI を持たない明示 opt-out は沈黙。
   この警告は core.hooksPath・commitMessage の状態に依存しない＝設定完了済みでも出す）。

他の出荷フック（pre_edit/post_edit/commit-msg）と作法を揃える:
stdout/stderr を UTF-8 に固定／.claude/project-profile.json を上方向探索／取得失敗は安全側（沈黙）。
SessionStart は非ブロッキングのため常に exit 0。警告は additionalContext（Claude のコンテキスト）へ
注入しつつ stderr にも出す（additionalContext が UI へ届かない場合のフォールバック）。
stdout の JSON は必ず 1 個（複数警告は結合して単一 additionalContext にする）。
"""
import sys
import json
import subprocess
from pathlib import Path

# 出力を UTF-8 に固定（Windows の既定エンコーディングでの文字化け回避）
try:
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
except Exception:
    pass


def load_profile(start):
    """start から上方向に .claude/project-profile.json を探す（pre_edit/post_edit と同一）。"""
    base = Path(start).resolve() if start else Path.cwd()
    for directory in [base, *base.parents]:
        candidate = directory / ".claude" / "project-profile.json"
        if candidate.exists():
            try:
                return json.loads(candidate.read_text(encoding="utf-8")), directory
            except Exception:
                return {}, directory
    return {}, None


def is_git_repo(cwd):
    """cwd が git 管理下か。取得不能・例外は False（安全側で沈黙）。"""
    try:
        result = subprocess.run(
            ["git", "-C", cwd, "rev-parse", "--is-inside-work-tree"],
            capture_output=True, text=True, encoding="utf-8", errors="replace",
            timeout=5,
        )
        return result.returncode == 0 and result.stdout.strip() == "true"
    except Exception:
        return False


def get_hooks_path(cwd):
    """git config core.hooksPath を読む。未設定・取得エラー・例外は None。"""
    try:
        result = subprocess.run(
            ["git", "-C", cwd, "config", "--get", "core.hooksPath"],
            capture_output=True, text=True, encoding="utf-8", errors="replace",
            timeout=5,
        )
        value = result.stdout.strip()
        return value if (result.returncode == 0 and value) else None
    except Exception:
        return None


def main():
    try:
        payload = json.load(sys.stdin)
    except Exception:
        return 0  # payload が読めない＝想定外の起動は安全側で沈黙

    cwd = payload.get("cwd") or str(Path.cwd())

    # git 管理外なら沈黙（テンプレ展開直後で git init 前など）
    if not is_git_repo(cwd):
        return 0

    profile, _ = load_profile(cwd)

    # 未セットアップ（project-profile.json が無い／name が空）は沈黙（全警告共通）。
    # /project-setup 完了後に name が入って初めて警告対象にする（展開直後・テンプレ外の誤警告抑制）。
    if not profile.get("name"):
        return 0

    msgs = []

    # --- 警告1: commit-msg 規律が無効 ---
    # commitMessage を明示的に無効化しているプロジェクトはこの警告のみ対象外（commit-msg を使わない設計）。
    if (profile.get("commitMessage") or {}).get("enabled") is not False:
        hooks_path = get_hooks_path(cwd)
        # core.hooksPath の basename が .githooks なら commit-msg が起動する（絶対/相対パス両対応）。
        # .husky 等の別運用も「.githooks でない」が、その場合は出荷の commit-msg が起動しないため警告対象でよい。
        if not (hooks_path is not None and Path(hooks_path).name == ".githooks"):
            if hooks_path is None:
                detail = "core.hooksPath が未設定です"
            else:
                detail = f"core.hooksPath が '{hooks_path}' を指しており .githooks ではありません"
            msgs.append(
                f"git のコミット規律フック（commit-msg）が無効です（{detail}）。"
                "Conventional Commits の body 必須・scope 必須が強制されず、"
                "subject 1 行だけの不完全なコミットが通過します。\n"
                "有効化: git config core.hooksPath .githooks "
                "（zip 展開で実行ビットが落ちた場合は chmod +x .githooks/* も。"
                "/project-setup の「バージョン管理・リモート同期」タスクが行います）"
            )

    # --- 警告2: kind=web なのに frontendDir が未確認（ビジュアル検証パイプライン停止） ---
    # "none"（UI を持たない明示 opt-out）・"."（ルート直下）・ディレクトリ名は正常。空だけが未確認。
    # この警告は core.hooksPath・commitMessage の状態に依存しない（セットアップ完了済みプロジェクトでも出す）。
    # 非文字列（schema 違反 profile）でクラッシュしないよう isinstance で安全側に倒す。
    # 複数フロント（モノレポ）は profile の frontendDirs[]（宣言用・任意）。本フックは単一フロントの
    # 主＝frontendDir を見る（複数フロントの運用は .claude/rules/scale.md）。
    frontend_dir = profile.get("frontendDir")
    if profile.get("kind") == "web" and not (isinstance(frontend_dir, str) and frontend_dir.strip()):
        msgs.append(
            "kind が web なのに frontendDir が空（未確認）です。"
            "ビジュアル/UX 検証パイプライン全体（Issue の視覚 AC 起票・dev-tasks の「ビジュアル/UX 確認」・"
            "DoD の（UI 変更時）項目・frontend-reviewer への委譲）が「対象外」として一度も走らず、"
            "見た目が未検証のままマージされます。\n"
            "設定: .claude/project-profile.json と CLAUDE.md「プロジェクト設定」の frontendDir に "
            "UI コードのあるディレクトリを設定してください（ルート直下にフロントがあるなら \".\"、"
            "UI を持たないプロジェクトなら \"none\" を明示）"
        )

    if not msgs:
        return 0

    msg = "\n\n".join(msgs)

    # Claude のコンテキストへ注入（additionalContext）。SessionStart は非ブロッキング。
    # 複数警告でも stdout の JSON は必ず 1 個（結合して単一 additionalContext にする）。
    print(json.dumps({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": msg,
        }
    }, ensure_ascii=False))
    # additionalContext が UI に届かないケースへのフォールバックとして stderr にも出す。
    print(f"WARNING: {msg}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    # 補助ガードの大原則「常に exit 0」を想定外入力（schema 違反 profile 等）でも守る。
    # docstring の「取得失敗は安全側（沈黙）」と同じ方針＝クラッシュでセッション開始を汚さない。
    try:
        sys.exit(main())
    except Exception:
        sys.exit(0)
