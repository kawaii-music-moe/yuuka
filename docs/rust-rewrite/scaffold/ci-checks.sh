#!/usr/bin/env bash
# CI 必須ゲート。絶対制約1（厳格エラー）の機械強制。
# 検証: ../verification/rpt-clippy-cargodeny.md §4
set -euo pipefail

# clippy: 全ターゲット・全フィーチャで警告をエラー化（manifest の deny と二重化）。
# ※ cargo build ではなく cargo clippy を回すこと（clippy lint はビルド時に評価されない）。
cargo clippy --all-targets --all-features -- -D warnings

# cargo-deny: bans（anyhow/eyre/color-eyre/backoff 禁止）はネットワーク不要で高速。
cargo deny check bans

# フルチェック（advisories/licenses/sources 含む。advisory DB 取得にネットワーク要）。
cargo deny check

# 型生成ドリフト検出（ts-rs/utoipa いずれでも: 生成 → 差分ゼロを検査）。R-12 参照。
# cargo test --workspace export_bindings   # ts-rs の場合
# git diff --exit-code -- frontend/src/lib/api/generated/

# テスト
cargo test --workspace
