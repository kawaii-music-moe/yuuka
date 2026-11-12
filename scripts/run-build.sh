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
# Hosted CI checks the actual PR merge commit in detached HEAD. These
# environment checks distinguish execution contexts, not a security sandbox.
hosted_ci=false
if [[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]]; then
  case "${GITHUB_EVENT_NAME:-}:${GITHUB_REF:-}" in
    pull_request:refs/pull/*/merge|push:refs/heads/develop|push:refs/heads/main)
      if [[ -n "${GITHUB_SHA:-}" && "$(git rev-parse HEAD)" == "$GITHUB_SHA" ]]; then
        hosted_ci=true
      fi ;;
  esac
fi
if [[ "$branch" != develop && "$hosted_ci" != true ]]; then
  echo "yuuka ルール: 現在のブランチは ${branch:-detached HEAD} のため、検証をスキップしました。コンパイルは develop 限定です。" >&2
  exit 2
fi
export CARGO_BUILD_JOBS=1
"$@"
