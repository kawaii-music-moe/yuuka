#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTANCE_SCRIPT="$ROOT/deploy/instance.sh"
VITE_COMPOSE="$ROOT/deploy/dev/vite.compose.yml"

usage() {
  cat <<'EOF'
Usage: pnpm dev:update [--no-cache] | pnpm dev:stop | pnpm dev:logs | pnpm dev:logs:vite

  dev:update      Rebuild the yuuka:dev image and recreate the dev API (--no-cache: full rebuild).
  dev:stop        Stop the dev API, Redis, and the Vite dev servers.
  dev:logs        Follow the dev API logs.
  dev:logs:vite   Follow the Vite dev server logs (PWA Client and admin UI).
EOF
}

# COMPOSE_PROJECT_NAME が環境に残っていると vite.compose.yml の `name: yuuka-dev-vite` より
# 優先されてしまうため外す（scripts/dev-start.sh と同じ）。
vite_compose() { env -u COMPOSE_PROJECT_NAME docker compose -f "$VITE_COMPOSE" "$@"; }

command="${1:-}"
[[ $# -gt 0 ]] && shift

NO_CACHE=0
for arg in "$@"; do
  case "$arg" in
    # pnpm 10+ は `pnpm dev:xxx -- --opt` の `--` もそのまま渡すので読み飛ばす。
    --) ;;
    --no-cache) [[ "$command" == "update" ]] || { echo "Unknown option: $arg" >&2; usage >&2; exit 2; }; NO_CACHE=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $arg" >&2; usage >&2; exit 2 ;;
  esac
done

case "$command" in
  update)
    if (( NO_CACHE )); then
      "$INSTANCE_SCRIPT" dev update --no-cache
    else
      "$INSTANCE_SCRIPT" dev update
    fi
    ;;
  stop)
    echo "Stopping the Vite dev servers..."
    vite_compose stop
    echo "Stopping the dev API and Redis..."
    "$INSTANCE_SCRIPT" dev stop
    ;;
  logs)
    "$INSTANCE_SCRIPT" dev logs -f
    ;;
  logs-vite)
    vite_compose logs -f client vite
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
