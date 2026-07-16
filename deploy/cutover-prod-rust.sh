#!/usr/bin/env bash
# ==============================================================================
# 本番(prod) を Node → Rust へ置き換える（安全カットオーバー / update 時の一発移行）
# ------------------------------------------------------------------------------
# prod は base docker-compose.yml を `image: yuuka:latest` で使う（dev のような隔離タグ問題は
# 生じない＝タグの付け替えで切替）。本スクリプトはダウンタイムを最小化しつつ、失敗時は現行 Node
# イメージへ自動ロールバックする。
#
# 手順（順序が重要）:
#   1. 前提チェック（DB / secret / 現行イメージが Node であること）
#   2. Node 稼働中に Rust を **staging タグ**でビルド（yuuka:rust-staging）＝ここまで無停止
#   3. staging が Rust バイナリ（CMD=./yuuka）であることを検証
#   4. 現行 yuuka:latest（Node）を yuuka:prev-prod へ退避（ロールバック用イメージ）
#   5. Node app を停止（クリーン終了＝WAL チェックポイント / SQLite 単一ライター解放・P0-2）
#   6. prod DB をバックアップ（停止後＝一貫性のあるスナップショット。main + wal + shm）
#   7. staging を yuuka:latest へ昇格し、Rust app を起動（YUUKA_RUST_DISCORD=1 で Discord も Rust）
#   8. ヘルスチェック（HTTP 200 + Rust API JSON + Bot ログイン痕跡）
#   9. 失敗時は yuuka:prev-prod（Node）へ自動ロールバック
#
# 使い方:
#   deploy/cutover-prod-rust.sh              # カットオーバー実行（既定=レイヤキャッシュ有効）
#   deploy/cutover-prod-rust.sh --no-cache   # Rust を --no-cache でフル再ビルドしてから切替
#   deploy/cutover-prod-rust.sh rollback     # 手動ロールバック（yuuka:prev-prod へ戻す）
#
# 前提: このブランチの Dockerfile は Rust バイナリ `yuuka` をビルドする（CMD ./yuuka）。
#       instance.env / secret.key は deploy/prod/ に既存であること。
# ==============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INST="prod"
INST_DIR="$ROOT/deploy/$INST"
ENV_FILE="$INST_DIR/instance.env"
SECRET_FILE="$INST_DIR/secret.key"
COMPOSE="$ROOT/docker-compose.yml"
STAGING_TAG="yuuka:rust-staging"
PREV_TAG="yuuka:prev-$INST"

[ -f "$ENV_FILE" ]    || { echo "❌ $ENV_FILE が無い" >&2; exit 1; }
[ -f "$SECRET_FILE" ] || { echo "❌ $SECRET_FILE が無い（暗号化シークレット未設定）" >&2; exit 1; }

# instance.env を export（HOST_PORT / DATA_DIR / COMPOSE_PROJECT_NAME / YUUKA_RUST_DISCORD 等）。
set -a
# shellcheck disable=SC1090
. "$ENV_FILE"
set +a

# 暗号化シークレットを literal で export（compose の変数展開を通さない・N2 fail-fast 回避）。
export YUUKA_ENCRYPTION_SECRET="$(cat "$SECRET_FILE")"
[ -f "$SECRET_FILE.new" ] && export YUUKA_ENCRYPTION_SECRET_NEW="$(cat "$SECRET_FILE.new")" || true
# Discord を Rust 所有に（Node イメージは無視）。instance.env に無くてもカットオーバー時は 1 を強制。
export YUUKA_RUST_DISCORD="${YUUKA_RUST_DISCORD:-1}"

PORT="${HOST_PORT:?instance.env に HOST_PORT が無い}"
DATA_DIR="${DATA_DIR:?instance.env に DATA_DIR が無い}"
DB="$DATA_DIR/yuuka.db"
BASE="http://127.0.0.1:$PORT"

dc() { docker compose -p "$COMPOSE_PROJECT_NAME" --env-file "$ENV_FILE" -f "$COMPOSE" "$@"; }

# ── ヘルスチェック（HTTP 200 + Rust API JSON。Bot ログインは痕跡があれば加点表示） ──────────
health() {
  local code
  code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "$BASE/" || true)"
  [ "$code" = "200" ] || { echo "  ✗ GET / → HTTP $code"; return 1; }
  curl -sf --max-time 5 "$BASE/api/setup/status" | grep -q '"needSetup"' \
    || { echo "  ✗ /api/setup/status が期待 JSON でない（Rust API 未応答）"; return 1; }
  echo "  ✓ / 200 / /api/setup/status JSON（needSetup）"
  return 0
}

# ── ロールバック: prev-prod（Node）を latest へ戻して再作成 ──────────────────────────────
rollback() {
  echo "⏪ ロールバック: $PREV_TAG（Node）→ yuuka:latest へ復帰..."
  if docker image inspect "$PREV_TAG" >/dev/null 2>&1; then
    docker tag "$PREV_TAG" yuuka:latest
    dc up -d app || true
    echo "   Node へ復帰しました（DB を戻す必要があれば: cp $DB.bak-<ts> $DB）"
  else
    echo "   ⚠ $PREV_TAG が無いためイメージ復帰不可。手動確認が必要です。" >&2
  fi
}

if [ "${1:-}" = "rollback" ]; then rollback; exit 0; fi

NO_CACHE=""
[ "${1:-}" = "--no-cache" ] && NO_CACHE="--no-cache"

echo "🔎 [1/9] 前提チェック..."
[ -f "$DB" ] || { echo "❌ prod DB が無い: $DB（P0-4 = Rust は DB を作らない）" >&2; exit 1; }
if docker image inspect yuuka:latest >/dev/null 2>&1; then
  cur_cmd="$(docker image inspect yuuka:latest --format '{{.Config.Cmd}}' 2>/dev/null || true)"
  echo "   現行 yuuka:latest CMD = $cur_cmd"
  case "$cur_cmd" in
    *yuuka*) echo "   ⚠ 既に Rust バイナリのようです（CMD に yuuka）。再切替でも安全に続行します。" ;;
  esac
else
  echo "   ⚠ yuuka:latest が未作成（初回デプロイ）。ロールバック用退避はスキップされます。"
fi

echo "🔨 [2/9] Rust を staging タグでビルド（$STAGING_TAG）＝Node 稼働のまま無停止 ${NO_CACHE:+(--no-cache)}..."
docker build $NO_CACHE -t "$STAGING_TAG" -f "$ROOT/Dockerfile" "$ROOT"

echo "🧪 [3/9] staging が Rust バイナリか検証..."
staging_cmd="$(docker image inspect "$STAGING_TAG" --format '{{.Config.Cmd}}')"
case "$staging_cmd" in
  *yuuka*) echo "   ✓ CMD = $staging_cmd（Rust 直起動）" ;;
  *) echo "❌ staging の CMD が Rust でない: $staging_cmd（Node をビルドした可能性＝ブランチ確認）" >&2
     docker image rm "$STAGING_TAG" >/dev/null 2>&1 || true
     exit 1 ;;
esac

echo "🏷  [4/9] 現行イメージを退避: $PREV_TAG（ロールバック用）..."
if docker image inspect yuuka:latest >/dev/null 2>&1; then
  docker tag yuuka:latest "$PREV_TAG"
else
  echo "   （yuuka:latest 未作成のため退避なし）"
fi

echo "⏸  [5/9] Node app を停止（クリーン終了＝WAL チェックポイント / 単一ライター解放）..."
dc stop app

TS="$(date +%Y%m%d-%H%M%S)"
echo "💾 [6/9] prod DB をバックアップ: $DB.bak-$TS（停止後の一貫スナップショット）..."
cp "$DB" "$DB.bak-$TS"
[ -f "$DB-wal" ] && cp "$DB-wal" "$DB-wal.bak-$TS" || true
[ -f "$DB-shm" ] && cp "$DB-shm" "$DB-shm.bak-$TS" || true

echo "🦀 [7/9] staging を yuuka:latest へ昇格し Rust app を起動（YUUKA_RUST_DISCORD=$YUUKA_RUST_DISCORD）..."
docker tag "$STAGING_TAG" yuuka:latest
dc up -d

echo "⏳ [8/9] ヘルスチェック（最大 60s）..."
ok=0
for i in $(seq 1 20); do
  sleep 3
  if health; then ok=1; break; fi
  echo "   ...待機 ($i/20)"
done

if [ "$ok" != "1" ]; then
  echo "❌ [9/9] ヘルスチェック失敗。Rust app ログ末尾:" >&2
  dc logs --tail 60 app 2>&1 | sed 's/^/   /' || true
  rollback
  exit 1
fi

# Bot ログイン痕跡（best-effort・失敗判定はしない）。
logins="$(dc logs app 2>&1 | grep -c 'としてログインしました' || true)"
echo "✅ [9/9] prod カットオーバー完了（:$PORT・yuuka:latest = Rust・Discord Bot ログイン ${logins} 件）"
echo "   退避イメージ: $PREV_TAG（Node）  /  DB バックアップ: $DB.bak-$TS"
echo "   ロールバック: deploy/cutover-prod-rust.sh rollback"
echo "   staging タグは不要になれば削除可: docker image rm $STAGING_TAG"
