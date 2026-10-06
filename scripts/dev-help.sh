#!/usr/bin/env bash
set -euo pipefail

# dev 環境で使うコマンドと構成の早見表。詳細は deploy/README.md の「dev インスタンス」。
# `--ja` で日本語表示。
LANG_JA=0
for arg in "$@"; do
  case "$arg" in
    # pnpm 10+ は `pnpm dev:help -- --ja` の `--` もそのまま渡すので読み飛ばす。
    --) ;;
    --ja) LANG_JA=1 ;;
    *) echo "Unknown option: $arg (use --ja for Japanese)" >&2; exit 2 ;;
  esac
done

if (( LANG_JA )); then
  cat <<'EOF'
Yuuka dev 環境

起動・初期化:
  pnpm dev:start              dev 環境（dev API + Redis、PWA Client / 管理画面の Vite）を起動する。
                              起動済みなら確認のうえ全コンテナを作り直して再起動する。
  pnpm dev:start --yes        確認なしで再起動する（端末以外から実行するときは必須）。
  pnpm dev:reset              dev API を止めて deploy/dev/data を空にし、新しい DB で起動し直す。
  pnpm dev:reset --re-build   dev:reset の前に yuuka:dev イメージをキャッシュなしで再ビルドする。

更新・停止・ログ:
  pnpm dev:update             yuuka:dev イメージをビルドして dev API を作り直す。
  pnpm dev:update --no-cache  キャッシュなしでフル再ビルドする。
  pnpm dev:stop               dev API・Redis と Vite dev server を停止する。
  pnpm dev:logs               dev API のログを追う。
  pnpm dev:logs:vite          Vite dev server（PWA Client・管理画面）のログを追う。

その他:
  pnpm dev:client:mock        Docker を使わず、モック API で PWA Client と管理画面を動かす。
  pnpm dev                    ホストで管理画面の Vite だけを動かす（VITE_API_TARGET で API を指定）。
  pnpm dev:help [--ja]        このヘルプを表示する（--ja で日本語）。

構成（deploy/dev/vite.compose.yml）:
  トンネル -> 127.0.0.1:5173  PWA Client の Vite（入口）
                /             PWA Client
                /admin/...    -> 127.0.0.1:7855 管理画面の Vite（内部用）
                /login        -> /admin/login へリダイレクト
                /api          -> 127.0.0.1:7856 dev API

詳細は deploy/README.md の「dev インスタンス」を参照。
EOF
else
  cat <<'EOF'
Yuuka dev environment

Start / reset:
  pnpm dev:start              Start the dev environment (dev API + Redis, PWA Client / admin Vite).
                              If already running, asks for confirmation and restarts everything.
  pnpm dev:start --yes        Restart without asking (required when stdin is not a terminal).
  pnpm dev:reset              Stop the dev API, clear deploy/dev/data, and start with a fresh database.
  pnpm dev:reset --re-build   Same as dev:reset, but rebuild the yuuka:dev image without cache first.

Update / stop / logs:
  pnpm dev:update             Rebuild the yuuka:dev image and recreate the dev API.
  pnpm dev:update --no-cache  Rebuild without the layer cache.
  pnpm dev:stop               Stop the dev API, Redis, and the Vite dev servers.
  pnpm dev:logs               Follow the dev API logs.
  pnpm dev:logs:vite          Follow the Vite dev server logs (PWA Client and admin UI).

Other:
  pnpm dev:client:mock        Run the PWA Client and admin UI against the mock API (no Docker).
  pnpm dev                    Run only the admin UI Vite on the host (set VITE_API_TARGET).
  pnpm dev:help [--ja]        Show this help (--ja for Japanese).

Layout (deploy/dev/vite.compose.yml):
  tunnel -> 127.0.0.1:5173  PWA Client Vite (entry point)
              /             PWA Client
              /admin/...    -> 127.0.0.1:7855 admin UI Vite (internal)
              /login        -> redirect to /admin/login
              /api          -> 127.0.0.1:7856 dev API

See deploy/README.md ("dev インスタンス") for details.
EOF
fi
