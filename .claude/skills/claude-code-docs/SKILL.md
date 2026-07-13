---
name: claude-code-docs
description: Claude Code（CLI ツール）や Claude Agent SDK の使い方・仕様を、公式ドキュメント（code.claude.com/docs）を参照して回答するとき使用する。「Claude Code の使い方を調べて」「この CLI オプションの仕様は」「Agent SDK の使い方を教えて」が発火ワード。汎用の Claude/Anthropic API（モデル ID・料金・Messages エンドポイント等）の利用や、このリポジトリ内コードの解説では使わない。
---

# Claude Code Docs

This skill answers questions by referencing the official Claude Code documentation.

## How to Fetch Documentation

1. Fetch the index: Use `WebFetch` on https://code.claude.com/docs/llms.txt
2. Identify relevant document URLs from the index
3. Use `WebFetch` to retrieve the relevant documents and answer the question

## Notes

- The index (llms.txt) contains links to all available documentation
- Refer to multiple documents if needed
- Answer in the user's language
