#!/usr/bin/env bash
# 一般的なコンパイルコマンドは共有ロックを保持するランナーに限定する。
set -euo pipefail
input=$(cat)
cmd=$(jq -r '.tool_input.command // empty' <<<"$input")
# 複合コマンドにランナーが含まれるだけでは許可しない。
if [[ "$cmd" =~ ^(\./)?\.claude/hooks/run-build\.sh[[:space:]] ]] &&
   [[ ! "$cmd" =~ [\;\&\|\`\$\(\)\<\>] ]] && [[ "$cmd" != *$'\n'* ]]; then
  exit 0
fi
# パッケージやスクリプト経由の間接ビルドも保守的に検出する。
if ! grep -qE '(^|[;&|(` /])(cargo|rustc|rustup|xtask|npm|npx|pnpm|yarn|bun|vite|vitest|tsc|vue-tsc|svelte-check|make|cmake|ninja|just)([[:space:]]|$)|(^|[[:space:]/])[^[:space:]]*build[^[:space:]]*|(^|[;&|(` ])(bash|sh|node) +[^;&|]*(scripts/|deploy/)' <<<"$cmd"; then
  exit 0
fi
if [[ "$cmd" =~ ^cargo[[:space:]]+(fmt|--version|-V)[[:space:]]*$ ]]; then
  exit 0
fi
echo 'yuuka ルール: コンパイルの可能性があるコマンドは .claude/hooks/run-build.sh <コマンド> [引数...] で実行してください。develop 限定・共有ロック・Rust 1ジョブを適用します。バックグラウンド起動や並列ビルドは禁止です。詳細は AGENTS.md を参照してください。' >&2
exit 2
