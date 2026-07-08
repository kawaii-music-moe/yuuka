# syntax=docker/dockerfile:1
# ==============================================================================
# Yuuka - マルチステージ Docker ビルド
#   stage 1 (rust-builder) : Rust 製クローラ yuuka-crawler をビルド
#   stage 2 (node-builder) : 依存導入 + TypeScript(tsgo) ビルド -> dist/
#   stage 3 (runtime)      : 本番実行イメージ（system chromium 同梱）
# 同一イメージを本番(prod)・開発(dev)など複数インスタンスで使い回す。
# インスタンス固有の差分（ポート/データ/シークレット/設定）は docker-compose 側で注入。
# ==============================================================================

# ---- stage 1: Rust backend (workspace) ----------------------------------
FROM rust:1-bookworm AS rust-builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY src/rust_crawler ./src/rust_crawler
COPY src/rust_synapse ./src/rust_synapse
COPY xtask ./xtask

RUN cargo build --release --bin yuuka --bin yuuka-crawler --bin yuuka-synapse  && cp target/release/yuuka /yuuka  && cp target/release/yuuka-crawler /yuuka-crawler  && cp target/release/yuuka-synapse /yuuka-synapse
# ---- stage 1b: Desktop client (Windows .exe をクロスコンパイル) --------------
# Linux 上から x86_64-pc-windows-gnu ターゲットで Windows GUI バイナリを生成する。
# C/asm 依存（ring 等）と最終リンクに mingw-w64 が必要。成果物はダッシュボードから
# 配布（/api/desktop/download）。clients/desktop 変更時のみ再ビルドされる（層キャッシュ）。
FROM rust:1-bookworm AS desktop-builder
RUN apt-get update \
 && apt-get install -y --no-install-recommends gcc-mingw-w64-x86-64 \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add x86_64-pc-windows-gnu
WORKDIR /build/desktop
COPY clients/desktop/Cargo.toml clients/desktop/Cargo.lock ./
COPY clients/desktop/src ./src
# 配布 exe に焼き込む公開 URL（インスタンス別）。config.rs が option_env!("YUUKA_API_BASE")
# で読む。未指定（空）なら焼き込まずローカル既定 http://127.0.0.1:7854 にフォールバックする。
# 空文字を env に残すと option_env! が Some("") を返し壊れるため、空なら unset する。
ARG YUUKA_API_BASE
# /out は常に作成する。ビルドが失敗してもサーバーイメージのビルドは続行させ
# （デプロイをブロックしない）、その場合ダッシュボードは「配布ビルド無し」を表示する。
RUN mkdir -p /out \
 && if [ -z "${YUUKA_API_BASE:-}" ]; then unset YUUKA_API_BASE; else export YUUKA_API_BASE; fi \
 && if cargo build --release --locked --target x86_64-pc-windows-gnu; then \
      cp target/x86_64-pc-windows-gnu/release/yuuka-desktop.exe /out/yuuka-desktop.exe \
      && sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n1 > /out/version.txt; \
    else \
      echo "WARNING: desktop client cross-build failed; shipping server without the Windows binary." >&2; \
    fi

# ---- stage 2: Node / TypeScript build ---------------------------------------
FROM node:24-bookworm AS node-builder
ENV PUPPETEER_SKIP_DOWNLOAD=true
WORKDIR /app
# ホスト/ロックファイルと同じ pnpm 9 系を使用（pnpm 11 は package.json の pnpm.overrides を
# 読まず lockfile と不一致になるため固定する）
RUN corepack enable && corepack prepare pnpm@9.15.0 --activate
# better-sqlite3 等のネイティブモジュールをビルドするためのツールチェーン
RUN apt-get update \
 && apt-get install -y --no-install-recommends python3 make g++ \
 && rm -rf /var/lib/apt/lists/*
# 依存導入（ロックファイル厳守）
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
# ソース・ドキュメント・フロントエンドを取り込み TypeScript(tsgo) + Vite をビルド
COPY tsconfig.json ./
COPY src ./src
COPY docs ./docs
COPY frontend ./frontend
# ビルド順序は install → tsgo → vite build → prune を厳守（prune=:82 より前で vite build）。
# prune 後は vite/svelte(devDeps) が消え vite build が失敗するため、必ず prune の前に実行する。
# vite build の outDir は frontend/vite.config.ts の outDir=../dist/public に従い /app/dist/public へ出力。
RUN pnpm exec tsgo \
 && pnpm exec vite build --config frontend/vite.config.ts \
 && mkdir -p dist/bin dist/assets \
 && cp -r src/assets/. dist/assets/
# Rust バイナリを dist/bin へ配置（browserService / synapseEngine が参照）
COPY --from=rust-builder /yuuka dist/bin/yuuka
COPY --from=rust-builder /yuuka-crawler dist/bin/yuuka-crawler
COPY --from=rust-builder /yuuka-synapse dist/bin/yuuka-synapse
# デスクトップ版 exe を配布ディレクトリへ配置（desktopClientRoutes が参照）。
# /out は desktop-builder で常に作られる（ビルド失敗時は空＝配布なし扱い）。
RUN mkdir -p dist/downloads
COPY --from=desktop-builder /out/ dist/downloads/
# 本番依存だけに刈り込み（tsx / biome / typescript 等の devDeps を除去）
RUN pnpm prune --prod

# ---- stage 3: runtime --------------------------------------------------------
FROM node:24-bookworm-slim AS runtime
# TZ=Asia/Tokyo: アプリは SQLite の datetime('now','localtime') や new Date()/getHours() など
# プロセスのローカルTZに依存して時刻を扱う（JST前提）。Debian slim は既定 UTC のため、
# 未設定だと全時刻が9時間ずれる。tzdata（下の apt-get）と併せて JST に固定する。
ENV NODE_ENV=production \
    TZ=Asia/Tokyo \
    PUPPETEER_SKIP_DOWNLOAD=true \
    PUPPETEER_EXECUTABLE_PATH=/usr/bin/chromium
WORKDIR /app
# Puppeteer 用 system chromium + 日本語/絵文字フォント + crawler(reqwest) の TLS 依存 + tzdata(JST解決用)
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      chromium \
      fonts-noto-cjk fonts-noto-color-emoji \
      ca-certificates libssl3 \
      tzdata \
 && rm -rf /var/lib/apt/lists/*
# アプリ本体（runtime に必要なものだけ builder から取得）
COPY --from=node-builder /app/node_modules ./node_modules
# dist は tsgo 出力(dist/index.js)・Vite 出力(dist/public=フロント配信元)・サーバー資産(dist/assets) を一括で拾う
COPY --from=node-builder /app/dist ./dist
# src/assets は Vite の /assets/ とは別物のサーバー実行時資産（@napi-rs/canvas 等）。常に残す（不可侵）
# （P5 完了: 旧 vanilla の src/public は撤去。フロントは dist/public 一本化。ロールバックは deploy rollback）
COPY --from=node-builder /app/src/assets ./src/assets
COPY --from=node-builder /app/docs ./docs
COPY package.json ./
# data/ は外部マウント、scratch/ はバックアップ一時作業用。node ユーザ(uid 1000)で書込可能に。
RUN mkdir -p /app/data /app/scratch && chown -R node:node /app
USER node
EXPOSE 7854
ENV YUUKA_RUST_CRON=1
CMD ["./dist/bin/yuuka"]
