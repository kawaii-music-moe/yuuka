#!/usr/bin/env bash
# コマンド終了まで共有ロックを保持する。
set -euo pipefail
if (( $# == 0 )); then
  echo 'Usage: scripts/run-build.sh <command> [args...]' >&2
  exit 2
fi
# Cargo の CLI 指定は環境変数より優先されるため、ジョブ指定を禁止する。
for arg in "$@"; do
  case "$arg" in
    -j*|--jobs|--jobs=*|CARGO_BUILD_JOBS=*)
      echo 'yuuka ルール: CARGO_BUILD_JOBS=1 を使用します。ジョブ指定を除いてください。' >&2
      exit 2 ;;
  esac
done
exec 9>/tmp/yuuka-codex-build.lock
if ! flock -n 9; then
  echo 'yuuka ルール: 別のビルドが実行中のため、検証をスキップしました。' >&2
  exit 2
fi
branch=$(git branch --show-current)
if [[ "$branch" != develop ]]; then
  echo "yuuka ルール: 現在のブランチは ${branch:-detached HEAD} のため、検証をスキップしました。コンパイルは develop 限定です。" >&2
  exit 2
fi
export CARGO_BUILD_JOBS=1
"$@"
