#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="$ROOT/deploy/dev/instance.env"
INSTANCE_SCRIPT="$ROOT/deploy/instance.sh"

usage() {
  cat <<'EOF'
Usage: pnpm dev:reset [--re-build]

Stops the dev API, clears deploy/dev/data, and starts it with a fresh database.
Pass --re-build to rebuild the dev Docker image from scratch before starting.
EOF
}

REBUILD=0
for arg in "$@"; do
  case "$arg" in
    --re-build) REBUILD=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $arg" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ ! -f "$ENV_FILE" ]]; then
  echo "Dev instance configuration not found: $ENV_FILE" >&2
  exit 1
fi

# Load only the dev instance's local parameters, then guard every destructive path.
set -a
# shellcheck disable=SC1090
. "$ENV_FILE"
set +a

EXPECTED_DATA_DIR="$(realpath -m "$ROOT/deploy/dev/data")"
ACTUAL_DATA_DIR="$(realpath -m "${DATA_DIR:-}")"
if [[ "${COMPOSE_PROJECT_NAME:-}" != "yuuka-dev" || "${YUUKA_IMAGE_TAG:-}" != "dev" || "$ACTUAL_DATA_DIR" != "$EXPECTED_DATA_DIR" ]]; then
  echo "Refusing reset: deploy/dev/instance.env must target yuuka-dev, yuuka:dev, and deploy/dev/data." >&2
  exit 1
fi
if [[ -L "$ROOT/deploy/dev/data" ]]; then
  echo "Refusing reset: deploy/dev/data must not be a symbolic link." >&2
  exit 1
fi
if (( ! REBUILD )) && ! docker image inspect "yuuka:${YUUKA_IMAGE_TAG}" >/dev/null 2>&1; then
  echo "Dev image yuuka:${YUUKA_IMAGE_TAG} is missing; rerun with --re-build." >&2
  exit 1
fi

echo "Stopping the dev API and Redis..."
"$INSTANCE_SCRIPT" dev down

mkdir -p "$EXPECTED_DATA_DIR"
find "$EXPECTED_DATA_DIR" -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +
echo "Cleared dev data: $EXPECTED_DATA_DIR"

if (( REBUILD )); then
  echo "Rebuilding the dev image without cache..."
  YUUKA_INIT_DB=1 "$INSTANCE_SCRIPT" dev update --no-cache
else
  echo "Starting the existing dev image without rebuilding..."
  YUUKA_INIT_DB=1 "$INSTANCE_SCRIPT" dev up -d --no-build
  "$INSTANCE_SCRIPT" dev verify
fi
