#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="$ROOT/deploy/dev/instance.env"
INSTANCE_SCRIPT="$ROOT/deploy/instance.sh"
VITE_COMPOSE="$ROOT/deploy/dev/vite.compose.yml"

usage() {
  cat <<'EOF'
Usage: pnpm dev:start [--yes]

Starts the dev environment (dev API + Redis, and the PWA Client / admin Vite dev servers).
If any of it is already running, asks for confirmation and then restarts everything.
Pass --yes (-y) to restart without asking (required when stdin is not a terminal).
EOF
}

ASSUME_YES=0
for arg in "$@"; do
  case "$arg" in
    -y|--yes) ASSUME_YES=1 ;;
    # pnpm 10+ は `pnpm dev:xxx -- --opt` の `--` もそのまま渡すので読み飛ばす。
    --) ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $arg" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ ! -f "$ENV_FILE" ]]; then
  echo "Dev instance configuration not found: $ENV_FILE" >&2
  exit 1
fi

# イメージタグの確認にだけ使う。export しない: COMPOSE_PROJECT_NAME=yuuka-dev が環境に残ると
# vite.compose.yml の `name: yuuka-dev-vite` より優先され、Vite 側の操作が API のプロジェクトに向く。
# shellcheck disable=SC1090
. "$ENV_FILE"

vite_compose() { env -u COMPOSE_PROJECT_NAME docker compose -f "$VITE_COMPOSE" "$@"; }

if ! docker image inspect "yuuka:${YUUKA_IMAGE_TAG:-latest}" >/dev/null 2>&1; then
  echo "Dev image yuuka:${YUUKA_IMAGE_TAG:-latest} is missing; build it with: deploy/instance.sh dev update" >&2
  exit 1
fi

# compose ファイルに定義されたサービスだけを見る（同じプロジェクトに残った孤児コンテナは数えない）。
# shellcheck disable=SC2046
api_running="$("$INSTANCE_SCRIPT" dev ps --status running -q $("$INSTANCE_SCRIPT" dev config --services))"
# shellcheck disable=SC2046
vite_running="$(vite_compose ps --status running -q $(vite_compose config --services))"

if [[ -n "$api_running" || -n "$vite_running" ]]; then
  echo "The dev environment is already running:"
  "$INSTANCE_SCRIPT" dev ps --format 'table {{.Name}}\t{{.Status}}'
  vite_compose ps --format 'table {{.Name}}\t{{.Status}}'
  if (( ! ASSUME_YES )); then
    if [[ ! -t 0 ]]; then
      echo "Refusing to restart without confirmation; rerun with --yes." >&2
      exit 1
    fi
    read -r -p "Restart the dev environment? [y/N] " answer
    if [[ ! "$answer" =~ ^[Yy]([Ee][Ss])?$ ]]; then
      echo "Aborted; nothing was changed."
      exit 0
    fi
  fi
  # 一部だけ止まっている場合も含め、全コンテナを作り直して揃える。
  echo "Restarting the dev API and Redis..."
  "$INSTANCE_SCRIPT" dev up -d --no-build --force-recreate
  "$INSTANCE_SCRIPT" dev verify
  echo "Restarting the Vite dev servers..."
  vite_compose up -d --force-recreate
else
  echo "Starting the dev API and Redis..."
  "$INSTANCE_SCRIPT" dev up -d --no-build
  "$INSTANCE_SCRIPT" dev verify
  echo "Starting the Vite dev servers..."
  vite_compose up -d
fi

# 外向きの入口（PWA Client の Vite）が /・/admin/・/api を返すまで待つ。
# 起動時の依存 install があるため、最大 120 秒待機する。
entry="http://127.0.0.1:${VITE_CLIENT_PORT:-5173}"
if [[ -f "$ROOT/deploy/dev/.env" ]]; then
  port="$(sed -n 's/^VITE_CLIENT_PORT=//p' "$ROOT/deploy/dev/.env" | tail -1)"
  [[ -n "$port" ]] && entry="http://127.0.0.1:$port"
fi
echo "Waiting for $entry ..."
for _ in $(seq 1 60); do
  ok=1
  for path in / /admin/ /api/setup/status; do
    code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 4 "$entry$path" || true)"
    [[ "$code" == "200" ]] || { ok=0; break; }
  done
  (( ok )) && break
  sleep 2
done
if (( ! ok )); then
  echo "The dev entry point did not become ready ($entry$path -> HTTP $code)." >&2
  echo "Check the logs: docker compose -f deploy/dev/vite.compose.yml logs client vite" >&2
  exit 1
fi
echo "Dev environment is ready: $entry (PWA Client /, admin /admin/, API /api)"
