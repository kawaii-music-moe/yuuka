#!/usr/bin/env bash
set -euo pipefail

# dev 環境で使うコマンドと構成の早見表。詳細は deploy/README.md の「dev インスタンス」。
cat <<'EOF'
Yuuka dev environment

Commands:
  pnpm dev:start              Start the dev environment (dev API + Redis, PWA Client / admin Vite).
                              If already running, asks for confirmation and restarts everything.
  pnpm dev:start -- --yes     Restart without asking (required when stdin is not a terminal).
  pnpm dev:reset              Stop the dev API, clear deploy/dev/data, and start with a fresh database.
  pnpm dev:reset -- --re-build
                              Same as dev:reset, but rebuild the yuuka:dev image without cache first.
  pnpm dev:client:mock        Run the PWA Client and admin UI against the mock API (no Docker).
  pnpm dev                    Run only the admin UI Vite on the host (set VITE_API_TARGET).
  pnpm deploy:logs:dev        Follow the dev API logs.
  pnpm dev:help               Show this help.

Other operations:
  deploy/instance.sh dev update                         Rebuild the yuuka:dev image and recreate the API.
  deploy/instance.sh dev stop                           Stop the dev API and Redis.
  docker compose -f deploy/dev/vite.compose.yml stop    Stop the Vite dev servers.
  docker compose -f deploy/dev/vite.compose.yml logs -f client vite
                                                        Follow the Vite logs.

Layout (deploy/dev/vite.compose.yml):
  tunnel -> 127.0.0.1:5173  PWA Client Vite (entry point)
              /             PWA Client
              /admin/...    -> 127.0.0.1:7855 admin UI Vite (internal)
              /login        -> redirect to /admin/login
              /api          -> 127.0.0.1:7856 dev API

See deploy/README.md ("dev インスタンス") for details.
EOF
