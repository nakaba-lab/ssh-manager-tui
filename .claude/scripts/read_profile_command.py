#!/usr/bin/env python3
"""CI 雛形用の共有スクリプト — .claude/project-profile.json の commands を 1 キー読む。

同梱 CI 4 雛形（.github/workflows/app-test.yml・release-deploy.yml／
.gitlab/ci/app-test.yml・deploy.yml）に同文複製されていた read_cmd ヒアドキュメントの
抽出先。呼び出しは `python3 .claude/scripts/read_profile_command.py <キー名>`
（cwd＝リポジトリルート前提。CI の checkout 直後はルートで実行される）。

契約（抽出前のヒアドキュメントと同一挙動）:
  - commands.<キー名> が設定済み  → その値を stdout に出力して exit 0
  - プロファイル不在・キー未設定 → 空出力で exit 0（未セットアップは安全に no-op
    ＝呼び出し側 CI が skip メッセージを出して成功する）
  - プロファイルが JSON として不正 → stderr に案内を出して exit 1（set -e の
    呼び出し側でジョブが fail し、壊れた profile を見逃さない）

他の出荷スクリプト（.claude/hooks/*.py）と作法を揃え、stdout/stderr を UTF-8 に固定する。
"""
import sys
import json

# 出力を UTF-8 に固定（Windows の既定エンコーディングでの文字化け回避）
try:
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
except Exception:
    pass

PROFILE_PATH = ".claude/project-profile.json"


def main():
    if len(sys.argv) != 2:
        print(
            "使い方: python3 .claude/scripts/read_profile_command.py <commands のキー名>",
            file=sys.stderr,
        )
        return 1
    try:
        profile = json.load(open(PROFILE_PATH, encoding="utf-8"))
    except OSError:
        return 0  # プロファイル不在＝未セットアップ。空出力で no-op（呼び出し側が skip）
    except ValueError:
        print(f"{PROFILE_PATH} が JSON として不正（修復してから再実行）", file=sys.stderr)
        return 1
    print(profile.get("commands", {}).get(sys.argv[1], "") or "")
    return 0


if __name__ == "__main__":
    sys.exit(main())
