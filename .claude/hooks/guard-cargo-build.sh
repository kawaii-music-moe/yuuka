#!/usr/bin/env bash
# develop ブランチ以外での cargo build/test/run などを禁止し、cargo check を使わせる
input=$(cat)
cmd=$(jq -r '.tool_input.command // empty' <<<"$input")
cwd=$(jq -r '.cwd // empty' <<<"$input")

# check / clippy / fmt / doc 以外のビルドを伴うサブコマンド
if ! grep -qE '(^|[;&|(` ])cargo( +\+[^ ]+)?( +-[^ ]+)* +(build|b|test|t|run|r|bench|nextest|install|llvm-cov|tarpaulin)( |$)' <<<"$cmd"; then
  exit 0
fi

branch=$(git -C "${cwd:-.}" rev-parse --abbrev-ref HEAD 2>/dev/null)
if [ "$branch" = "develop" ]; then
  exit 0
fi

echo "yuuka ルール: develop ブランチ以外（現在: ${branch:-不明}）では cargo build/test/run 等の実ビルドは禁止です。cargo check（必要なら cargo clippy）で確認してください。実ビルドでの検証は develop ブランチで行います。" >&2
exit 2
