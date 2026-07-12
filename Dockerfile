# syntax=docker/dockerfile:1
# ==============================================================================
# Yuuka — 本番スリムイメージ（Rust 直起動・成果物のみ同梱）
#
#   stage rust-builder     : Rust workspace の本番バイナリ `yuuka` をビルド（strip + thin-LTO）
#   stage frontend-builder : Vite で SPA を `dist/public` へビルド
#   stage runtime          : debian-slim に **バイナリ + SPA のみ**を載せた最小実行イメージ
#
# 設計: 出荷物は Rust バイナリ `yuuka` と、それが配信する SPA（`dist/public`）だけ。
#   Node ランタイム / node_modules / dist/index.js（Node サーバ束）/ chromium / フォント /
#   yuuka-crawler / yuuka-synapse / デスクトップ exe は Rust 直起動では未使用のため**同梱しない**。
#   （Rust は外部プロセスを spawn せず、reqwest=rustls で OpenSSL 不要、migration はコンパイル時 embed。
#    実行時の外部依存は config.yaml と dist/public のみ・ライブ検証済み。）
#
# 実行時トポロジ: 本イメージは **Rust 単独**（現行 CMD と同一）。Node strangler（経路A）を併走
#   させる構成は別途フロント段プロキシが必要（docs/rust-rewrite/remaining-work.md B1 参照）。
# インスタンス固有値（ポート/データ/シークレット/設定）は docker-compose 側で注入する。
# ==============================================================================

# ---- stage: Rust backend（workspace の本番バイナリ 1 本のみ） -----------------
FROM rust:1-bookworm AS rust-builder
WORKDIR /build
# workspace のマニフェストと第一者クレートのみ。crawler/synapse（workspace exclude・未使用）は入れない。
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY xtask ./xtask
# registry と target をキャッシュマウントに載せて再ビルドを高速化する。target はレイヤに残らない
# ため、同一 RUN 内で成果物を実パス /yuuka へ取り出す（strip 済み・thin-LTO 済み）。
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked --bin yuuka \
 && cp target/release/yuuka /yuuka

# ---- stage: frontend（Vite → dist/public） -----------------------------------
FROM node:24-bookworm AS frontend-builder
# puppeteer の chromium ダウンロードを抑止（フロントビルドに不要）。
ENV PUPPETEER_SKIP_DOWNLOAD=true
WORKDIR /app
# lockfile と同じ pnpm 9 系（pnpm 11 は overrides を読まず lockfile 不一致になる）。
RUN corepack enable && corepack prepare pnpm@9.15.0 --activate
# pnpm-workspace の allowBuilds に better-sqlite3 があり install 時にネイティブビルドが走るため、
# ビルド段にのみツールチェーンを置く（最終イメージには載らない）。
RUN apt-get update \
 && apt-get install -y --no-install-recommends python3 make g++ \
 && rm -rf /var/lib/apt/lists/*
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile
# フロントのみビルド（Node サーバ束 tsgo は不要）。outDir は frontend/vite.config.ts の
# `../dist/public` に従い /app/dist/public へ出力される。frontend は ../src を import しない。
COPY tsconfig.json ./
COPY frontend ./frontend
RUN pnpm exec vite build --config frontend/vite.config.ts

# ---- stage: runtime（最小実行イメージ） --------------------------------------
FROM debian:bookworm-slim AS runtime
# TZ=Asia/Tokyo: Rust は chrono の Local と SQLite datetime('now','localtime') で JST 前提の
# 壁時計を扱う。slim は既定 UTC のため tzdata と併せ JST に固定する（未設定だと 9 時間ずれる）。
# YUUKA_RUST_CRON=1: 本イメージは Rust が cron を所有する（Node cron と同時起動しないこと）。
ENV TZ=Asia/Tokyo \
    YUUKA_RUST_CRON=1
# ca-certificates: 上流 HTTPS（Gemini/Discord/Google）用のルート証明書。tzdata: JST 解決用。
# OpenSSL は使わない（reqwest=rustls）ため libssl3 は入れない。
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates tzdata \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd -g 1000 yuuka \
 && useradd -u 1000 -g 1000 -m -s /usr/sbin/nologin yuuka
WORKDIR /app
# 成果物のみ: Rust バイナリ + それが配信する SPA。config.yaml と data/ は compose がマウントする。
COPY --from=rust-builder /yuuka ./yuuka
COPY --from=frontend-builder /app/dist/public ./dist/public
# data/ は外部マウント点。所有を非 root（uid 1000・compose の PUID 既定と一致）へ。
RUN mkdir -p /app/data && chown -R yuuka:yuuka /app
USER yuuka
EXPOSE 7854
CMD ["./yuuka"]
