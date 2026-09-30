#!/usr/bin/env bash
# ==============================================================================
# Yuuka インスタンス操作ヘルパー
#   使い方: deploy/instance.sh <インスタンス名> <コマンド...>
#
#   独自コマンド:
#     update    退避(yuuka:prev-<名>) → ビルド → 再作成 → ヘルスチェック（推奨デプロイ手順）
#               既定はレイヤキャッシュ有効で高速。`update --no-cache` でフル再構築。
#     rollback  退避イメージへ戻して再作成
#     verify    ヘルスチェックのみ（HTTP/Botログイン/エラー件数）
#     hot       ホットリロードモード（API=tsx watch + src/ マウント。Rust はイメージ内蔵）。
#               フロントはホストで `pnpm dev:front`(Vite 5173) を別途起動し proxy 経由で
#               このコンテナの /api・/ws/chat を叩く（親書§5.7 案A）。
#               既定はフォアグラウンド起動（Ctrl+C で停止）。`pnpm dev` から呼ばれる。
#               `hot -d` でバックグラウンド起動。`hot --build` で dev-hot イメージを
#               強制再ビルド（Cargo.toml / package.json / Dockerfile.dev 変更時）。
#               同ポート/同DBを掴む通常インスタンスの app は自動停止する（競合回避）。
#     hot-logs  ホットリロードコンテナのログを tail -f
#     hot-down  ホットリロードコンテナを停止
#   それ以外は docker compose のサブコマンドへそのまま委譲:
#     up -d / down / logs -f / ps / build / restart ...
#
#   例:
#     deploy/instance.sh prod update      # 本番をビルドして反映
#     deploy/instance.sh dev  update      # 開発を反映
#     deploy/instance.sh prod rollback    # 直前イメージへ戻す
#     deploy/instance.sh prod logs -f
#
# 動作:
#   - deploy/<名>/instance.env を読み込み（COMPOSE_PROJECT_NAME, ポート, パス等）
#   - deploy/<名>/secret.key を YUUKA_ENCRYPTION_SECRET として literal で export
#     （ファイル内の $ を再展開しない。compose の env_file/変数展開は $ を壊すため使わない）
#   - deploy/<名>/secret.key.new があれば YUUKA_ENCRYPTION_SECRET_NEW として export（鍵ローテーション）
# ==============================================================================
set -euo pipefail

INST="${1:-}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [ -z "$INST" ]; then
  echo "usage: $0 <instance> <update|rollback|verify|docker-compose-subcommand...>" >&2
  echo "  instances: $(cd "$ROOT/deploy" && ls -d */ 2>/dev/null | tr -d / | tr '\n' ' ')" >&2
  exit 1
fi
shift

INST_DIR="$ROOT/deploy/$INST"
ENV_FILE="$INST_DIR/instance.env"
SECRET_FILE="$INST_DIR/secret.key"
[ -f "$ENV_FILE" ]    || { echo "❌ インスタンス設定が見つかりません: $ENV_FILE" >&2; exit 1; }
[ -f "$SECRET_FILE" ] || { echo "❌ シークレットが見つかりません: $SECRET_FILE" >&2; exit 1; }

# instance.env を export（$ を含まない安全な値のみ。compose 変数展開にも使う）
set -a
# shellcheck disable=SC1090
. "$ENV_FILE"
set +a

# 暗号化シークレットは $(cat) で literal 取得（ファイル内の $ は再展開されない）
YUUKA_ENCRYPTION_SECRET="$(cat "$SECRET_FILE")"
export YUUKA_ENCRYPTION_SECRET
if [ -f "$INST_DIR/secret.key.new" ]; then
  YUUKA_ENCRYPTION_SECRET_NEW="$(cat "$INST_DIR/secret.key.new")"
  export YUUKA_ENCRYPTION_SECRET_NEW
fi

# docker compose ラッパ
dc()     { docker compose -p "$COMPOSE_PROJECT_NAME"      --env-file "$ENV_FILE" -f "$ROOT/docker-compose.yml"         "$@"; }
dc_hot() { docker compose -p "${COMPOSE_PROJECT_NAME}-hot" --env-file "$ENV_FILE" -f "$ROOT/docker-compose.dev-hot.yml" "$@"; }

# インスタンスごとのイメージタグ（#56・instance.env の YUUKA_IMAGE_TAG。未設定なら従来通り latest）。
# docker-compose.yml の `image: yuuka:${YUUKA_IMAGE_TAG:-latest}` と同じ既定値にしておくことで、
# `dc build`/`dc up -d` が実際に作る/使うタグと、ここで退避・ロールバックするタグを一致させる。
IMG_TAG="${YUUKA_IMAGE_TAG:-latest}"
IMG="yuuka:$IMG_TAG"

health_check() {
  local port="${HOST_PORT:-7854}" code=""
  echo "🔎 ヘルスチェック: http://127.0.0.1:$port/ （最大40秒待機）"
  for _ in $(seq 1 20); do
    code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 4 "http://127.0.0.1:$port/" || true)"
    [ "$code" = "200" ] && break
    sleep 2
  done
  echo "── 直近ログ ──"
  dc logs --tail 20 app 2>&1 | sed 's/^/   /' || true
  local logins errs
  # Rust 版の実際のログ文言・レベル書式で判定する（#53・Node 版の文言は撤去済みで常に 0 件になっていた）。
  # tracing のログ行は `<timestamp> <LEVEL> <target>: ...` 形式（LEVEL は5桁固定幅で ERROR は左右パディング無し）
  # なので ' ERROR ' で狙い撃つ。telemetry 初期化（crates/yuuka-core/src/telemetry.rs）が
  # 非 TTY では ANSI を無効化するため、docker logs 越しでも色コードで grep が壊れることはない。
  logins="$(dc logs app 2>&1 | grep -c 'Discord ログイン成功' || true)"
  errs="$(dc logs app 2>&1 | grep -c ' ERROR ' || true)"
  echo "── 判定 ──"
  echo "   HTTP: $code / Botログイン: ${logins}件 / エラー疑い: ${errs}件"
  if [ "$code" = "200" ]; then
    # ── 追加検証（フロントは dist/public 一本化。§15.x / P5 完了後は旧 vanilla 配信の分岐なし）──
    local base="http://127.0.0.1:$port"
    # (a) CSP に script-src 'self'（curl -sI=HEAD。server.ts は method 非依存で writeHead:
    #     :198/:222 でヘッダを書くため HEAD でも CSP が返る）
    if ! curl -sI "$base/" | grep -qi "content-security-policy:.*script-src 'self'"; then
      echo "❌ [$INST] CSP: script-src 'self' 不在" >&2
      return 1
    fi
    # index.html 本体を1回だけ取得（curl は Accept-Encoding を送らないため非圧縮 = 生 HTML）
    local html
    html="$(curl -s "$base/")"
    # (b) index.html が hashed module を参照（Vite 既定ハッシュ=8字ちょうど。
    #     server.ts:157 の immutable 正規表現と同一閾値で連動。hash 長を変えるなら両所同時更新）
    if ! echo "$html" | grep -qE 'src="/assets/[^"]+-[A-Za-z0-9_-]{8,}\.js"'; then
      echo "❌ [$INST] hashed module 参照が無い" >&2
      return 1
    fi
    # (d) /assets/*.js が immutable（curl -sI=HEAD。server.ts:158-159 の Cache-Control 分岐）
    local asset
    asset="$(echo "$html" | grep -oE '/assets/[^"]+\.js' | head -1)"
    if ! curl -sI "$base$asset" | grep -qi "cache-control:.*immutable"; then
      echo "❌ [$INST] /assets/*.js が immutable でない" >&2
      return 1
    fi
    # (c) GSV meta（config 設定時のみ・警告に留める）
    echo "$html" | grep -q 'google-site-verification' || echo "⚠️  [$INST] GSV 未注入（config 次第）"
    echo "✅ [$INST] デプロイ成功（CSP/hashed/immutable 検証済）"
  else
    echo "❌ [$INST] ヘルスチェック失敗（HTTP $code）。'deploy/instance.sh $INST rollback' で直前イメージへ復旧できます。" >&2
    return 1
  fi
}

CMD="${1:-}"
case "$CMD" in
  update|deploy)
    echo "🚀 [$INST] デプロイ開始（イメージ: $IMG）..."
    if docker image inspect "$IMG" >/dev/null 2>&1; then
      docker tag "$IMG" "yuuka:prev-$INST" && echo "🏷  現行イメージを退避: yuuka:prev-$INST"
    fi
    # 既定はレイヤキャッシュ有効（変更の無い層＝重い Rust クロスコンパイル等は再利用＝高速）。
    # Dockerfile は Cargo.toml/lock・package.json を src より先に COPY する構造のため、
    # ソース未変更ならその重いステージはキャッシュヒットする。
    # apt のベース更新まで含めてクリーン再構築したい時だけ: `update --no-cache`。
    if [ "${2:-}" = "--no-cache" ]; then
      echo "🔨 ビルド（--no-cache フルリビルド）..."
      dc build --no-cache
    else
      echo "🔨 ビルド（レイヤキャッシュ有効。フル再構築は 'update --no-cache'）..."
      dc build
    fi
    echo "♻️  コンテナ再作成..."
    dc up -d
    health_check
    ;;
  rollback)
    docker image inspect "yuuka:prev-$INST" >/dev/null 2>&1 \
      || { echo "❌ 退避イメージ yuuka:prev-$INST がありません（まだ update していない可能性）" >&2; exit 1; }
    echo "⏪ [$INST] ロールバック: yuuka:prev-$INST → $IMG"
    docker tag "yuuka:prev-$INST" "$IMG"
    dc up -d
    health_check
    ;;
  verify)
    health_check
    ;;
  hot)
    # ホットリロードモード（tsx watch + src/ マウント。Rust バイナリはイメージに焼き込み済み）。
    # 既定はフォアグラウンド起動（`pnpm dev`）。-d でバックグラウンド、--build で強制再ビルド。
    hot_detach="" hot_build=""
    shift  # "hot" を除去し、残りをフラグとして解釈
    for arg in "$@"; do
      case "$arg" in
        -d|--detach) hot_detach="1" ;;
        --build)     hot_build="1" ;;
      esac
    done
    if [ -n "$hot_build" ]; then
      echo "🔨 [$INST] dev-hot イメージを再ビルド..."
      dc_hot build
    elif ! docker image inspect yuuka:dev-hot >/dev/null 2>&1; then
      echo "🔨 [$INST] dev-hot イメージが存在しないためビルドします..."
      dc_hot build
    fi
    # 通常インスタンスの app が起動中なら停止（同じ HOST_PORT / 同じ SQLite を掴み競合するため排他）。
    if [ -n "$(dc ps --status running -q app 2>/dev/null)" ]; then
      echo "⏸  通常 [$INST] app を停止（hot と同ポート/同DBのため排他）..."
      dc stop app
    fi
    if [ -n "$hot_detach" ]; then
      echo "🔥 [$INST] ホットリロード起動（バックグラウンド / tsx watch）..."
      dc_hot up -d
      echo "📋 ログ: deploy/instance.sh $INST hot-logs"
    else
      echo "🔥 [$INST] ホットリロード起動（tsx watch / Ctrl+C で停止）..."
      dc_hot up
    fi
    ;;
  hot-logs)
    dc_hot logs -f app
    ;;
  hot-down)
    dc_hot down
    ;;
  "")
    echo "usage: $0 $INST <update|rollback|verify|hot|hot-logs|hot-down|docker-compose-subcommand...>" >&2
    echo "  hot: API のみ（tsx watch）。フロントはホストで pnpm dev:front(Vite 5173) を併走させる（親書§5.7 案A）" >&2
    exit 1
    ;;
  *)
    dc "$@"
    ;;
esac
