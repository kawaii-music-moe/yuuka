#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
for arg in "$@"; do
  case "$arg" in
    --yes|-y|--) ;; # Compatibility with the former Docker command; start is idempotent.
    -h|--help) echo 'Usage: pnpm dev:start (start systemd dev API and Vite services)'; exit 0 ;;
    *) echo "Unknown option: $arg" >&2; exit 2 ;;
  esac
done
# Resolve dependencies before starting services, so missing installs fail immediately.
cd "$ROOT"
node -e 'for (const dir of [process.cwd(), process.cwd()+"/client/pwa"]) require.resolve("vite/package.json", {paths:[dir]})' \
  || { echo 'Run pnpm install --frozen-lockfile && npm --prefix client/pwa ci first.' >&2; exit 1; }
systemctl --user start yuuka-dev.service
if systemctl --user is-active --quiet yuuka-dev-vite.service; then
  echo 'yuuka-dev-vite is already running. Stop it with pnpm dev:stop before changing worktrees.'
  exit 0
fi
systemctl --user reset-failed yuuka-dev-vite.service 2>/dev/null || true
systemd-run --user --unit=yuuka-dev-vite --collect --property=KillMode=control-group \
  --working-directory="$ROOT" \
  --setenv="VITE_API_TARGET=${VITE_API_TARGET:-http://127.0.0.1:3403}" \
  --setenv="VITE_PORT=${VITE_PORT:-7855}" \
  --setenv="VITE_CLIENT_PORT=${VITE_CLIENT_PORT:-5173}" \
  --setenv="VITE_BIND_ADDR=${VITE_BIND_ADDR:-127.0.0.1}" \
  "$(command -v node)" "$ROOT/scripts/dev.mjs"
entry="http://127.0.0.1:${VITE_CLIENT_PORT:-5173}"
for _ in {1..30}; do
  systemctl --user is-active --quiet yuuka-dev-vite.service || break
  ok=1
  for probe in /:200 /admin/:200 /api/me:401; do
    code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 "$entry${probe%:*}" || true)"
    [[ "$code" == "${probe##*:}" ]] || ok=0
  done
  if (( ok )); then echo "Dev environment ready: $entry (admin: /admin/)"; exit 0; fi
  sleep 1
done
echo 'Dev startup failed. See pnpm dev:logs:vite and pnpm dev:logs.' >&2
systemctl --user stop yuuka-dev-vite.service
exit 1
