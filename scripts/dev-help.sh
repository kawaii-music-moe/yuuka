#!/usr/bin/env bash
set -euo pipefail
cat <<'HELP'
pnpm dev                 PWA + 管理画面の Vite を前景で起動（Ctrl-C で両方停止）
pnpm dev:admin           管理画面の Vite のみ起動
pnpm dev:start           systemd の dev API + Vite をバックグラウンド起動
pnpm dev:stop            dev API + バックグラウンド Vite を停止（共有 Redis は継続）
pnpm dev:update          この作業ツリーの Rust をビルドし dev API を更新・再起動
pnpm dev:logs            dev API の journal を追跡
pnpm dev:logs:vite       バックグラウンド Vite の journal を追跡
pnpm dev:client:mock     モック API でフロントを起動

入口: http://127.0.0.1:5173/ （PWA）、/admin/ （管理画面）
API: http://127.0.0.1:3403 （VITE_API_TARGET で変更）
初回: pnpm install --frozen-lockfile && npm --prefix client/pwa ci
Docker 運用: pnpm dev:docker:start / dev:docker:update / dev:docker:stop
詳細: deploy/README.md
HELP
