#!/usr/bin/env bash
# 既存の Claude Code 設定・呼び出しとの互換性を維持する。
set -euo pipefail
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
exec bash "$script_dir/../../scripts/run-build.sh" "$@"
