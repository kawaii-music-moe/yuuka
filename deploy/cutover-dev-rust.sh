#!/usr/bin/env bash
# ==============================================================================
# dev 環境を Rust 版へ置き換える（安全カットオーバー）
# ------------------------------------------------------------------------------
# - prod と共有の yuuka:latest タグを一切触らず、dev 専用イメージ yuuka:dev-rust で起動する
#   （docker-compose.dev-rust.yml オーバーレイ）。
# - Node dev-hot（tsx watch・:7855・同一 SQLite）を停止してから Rust dev を起動する
#   （SQLite 単一ライター遵守・P0-2）。
# - 起動前に dev DB をバックアップし、ヘルスチェック失敗時は自動でロールバック
#   （Rust dev を落とし Node dev-hot を復帰・DB は必要なら復元）。
#
# 使い方:
#   deploy/cutover-dev-rust.sh            # カットオーバー実行
#   deploy/cutover-dev-rust.sh rollback   # 手動ロールバック（Rust→Node dev-hot）
#
# 前提: yuuka:dev-rust を事前ビルド済み（docker build -t yuuka:dev-rust -f Dockerfile .）
# ==============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="$ROOT/deploy/dev/instance.env"
DATA_DIR="$ROOT/deploy/dev/data"
DB="$DATA_DIR/yuuka.db"
PORT="7855"
BASE="http://127.0.0.1:$PORT"

dc_rust() { docker compose -p yuuka-dev      --env-file "$ENV_FILE" -f "$ROOT/docker-compose.yml" -f "$ROOT/docker-compose.dev-rust.yml" "$@"; }
dc_hot()  { docker compose -p yuuka-dev-hot  --env-file "$ENV_FILE" -f "$ROOT/docker-compose.dev-hot.yml" "$@"; }

# 暗号化シークレットを secret.key から literal で export（compose の pass-through が空だと
# Rust も Node も起動時 fail-fast する＝instance.sh と同じ手順・P1-5/N2）。up と rollback の両方で必要。
SECRET_FILE="$ROOT/deploy/dev/secret.key"
[ -f "$SECRET_FILE" ] || { echo "❌ $SECRET_FILE が無い（暗号化シークレット未設定）" >&2; exit 1; }
export YUUKA_ENCRYPTION_SECRET="$(cat "$SECRET_FILE")"
[ -f "$SECRET_FILE.new" ] && export YUUKA_ENCRYPTION_SECRET_NEW="$(cat "$SECRET_FILE.new")" || true

health() {
  # / が 200 かつ /api/setup/status が JSON を返すことを確認（Rust API 稼働の証跡）。
  local code
  code="$(curl -s -o /dev/null -w '%{http_code}' "$BASE/" || true)"
  [ "$code" = "200" ] || { echo "  ✗ GET / → HTTP $code"; return 1; }
  curl -sf "$BASE/api/setup/status" | grep -q '"needSetup"' || { echo "  ✗ /api/setup/status が期待 JSON でない"; return 1; }
  echo "  ✓ / 200 / /api/setup/status JSON（needSetup）"
  return 0
}

rollback() {
  echo "⏪ ロールバック: Rust dev を停止し Node dev-hot を復帰..."
  dc_rust down || true
  dc_hot up -d || true
  echo "   （必要なら DB を復元: cp $DB.bak-<ts> $DB）"
}

if [ "${1:-}" = "rollback" ]; then rollback; exit 0; fi

echo "🔎 前提チェック..."
docker image inspect yuuka:dev-rust >/dev/null 2>&1 || { echo "❌ yuuka:dev-rust が無い。先に: docker build -t yuuka:dev-rust -f Dockerfile ." >&2; exit 1; }
[ -f "$DB" ] || { echo "❌ dev DB が無い: $DB（P0-4 = Rust は DB を作らない）" >&2; exit 1; }

TS="$(date +%Y%m%d-%H%M%S)"
echo "💾 dev DB をバックアップ: $DB.bak-$TS"
cp "$DB" "$DB.bak-$TS"
# WAL があれば checkpoint 済みのメイン DB を確実に取るため -wal/-shm も退避（存在すれば）。
[ -f "$DB-wal" ] && cp "$DB-wal" "$DB-wal.bak-$TS" || true
[ -f "$DB-shm" ] && cp "$DB-shm" "$DB-shm.bak-$TS" || true

echo "⏸  Node dev-hot（tsx watch）を停止（同 :$PORT / 同 SQLite の単一ライター遵守）..."
dc_hot down

echo "🦀 Rust dev（yuuka:dev-rust）を起動..."
dc_rust up -d

echo "⏳ ヘルスチェック（最大 60s）..."
ok=0
for i in $(seq 1 20); do
  sleep 3
  if health; then ok=1; break; fi
  echo "   ...待機 ($i/20)"
done

if [ "$ok" != "1" ]; then
  echo "❌ ヘルスチェック失敗。ログ末尾:"
  dc_rust logs --tail 40 app || true
  rollback
  exit 1
fi

echo "✅ dev を Rust 版へ置き換え完了（:$PORT・yuuka:dev-rust）"
echo "   ロールバック: deploy/cutover-dev-rust.sh rollback"
echo "   Discord live 化: instance.env に YUUKA_RUST_DISCORD=1 + docker-compose.dev-rust.yml の environment を有効化して再 up"
