#!/usr/bin/env bash
# 本番（systemd ユーザーサービス yuuka.service）へ develop の最新版をデプロイする。main には出さない。
#
#   deploy/prod/deploy-develop.sh                  # origin/develop の最新を本番へ
#   deploy/prod/deploy-develop.sh rollback <dir>   # 退避した状態（バイナリ・画面・DB）へ戻す
#
# 本番は本体のチェックアウト（/home/suki/apps/yuuka・develop）を作業ディレクトリに動いている:
#   バイナリ deploy/prod/bin/yuuka（run-yuuka.sh が起動）・画面 dist/public（管理画面 + pwa/）・
#   DB data/yuuka.db・音声モデル data/tts-models（手で配置・tools/irodori-onnx/README.md）。
#
# 流れ: develop を fast-forward → リリースビルド（Rust・管理画面・PWA）を別の場所に作る →
#   サービスを止めて、今の状態（バイナリ・画面・DB・設定）を deploy/prod/backups/<時刻>-develop-<sha>/
#   へ退避 → 入れ替えて起動 → 確認。確認に失敗したら rollback の手順を表示する（自動では戻さない:
#   マイグレーションを当てた後に古いバイナリへ戻すと DB も戻す必要があり、デプロイ後の書き込みが消えるため）。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PROD="$ROOT/deploy/prod"
SERVICE=yuuka.service
PORT="$(grep -E '^PORT:' "$PROD/config.yaml" | grep -oE '[0-9]+' | head -1)"
BASE="http://127.0.0.1:${PORT:-3402}"
cd "$ROOT"

log() { printf '\033[1m== %s\033[0m\n' "$*"; }
die() { printf '❌ %s\n' "$*" >&2; exit 1; }

# 今の本番の状態（バイナリ・画面・DB・設定・記録）を $1 へ退避する。DB はサービス停止中に呼ぶこと。
snapshot() {
  local dest="$1"
  mkdir -p "$dest/data"
  cp -a "$PROD/bin/yuuka" "$dest/yuuka"
  cp -a "$ROOT/dist/public" "$dest/public"
  cp -a "$PROD/config.yaml" "$PROD/run-yuuka.sh" "$dest/"
  cp -a "$PROD"/deployed-revision "$PROD"/deployed-frontend-revision "$dest/" 2>/dev/null || true
  for f in yuuka.db yuuka.db-wal yuuka.db-shm; do
    [ -e "$ROOT/data/$f" ] && cp -a "$ROOT/data/$f" "$dest/data/"
  done
  chmod 700 "$dest/data"
}

wait_healthy() {
  for _ in $(seq 1 30); do
    if [ "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/")" = 200 ]; then return 0; fi
    sleep 1
  done
  return 1
}

# 起動後の確認。失敗したら非 0。
verify() {
  local since="$1" ok=1 code
  systemctl --user is-active --quiet "$SERVICE" || { echo "❌ $SERVICE が動いていません"; return 1; }
  wait_healthy || { echo "❌ $BASE/ が 200 を返しません"; return 1; }
  check() { # <説明> <期待する HTTP コード> <パス>
    code="$(curl -s -o /dev/null -w '%{http_code}' "$BASE$3")"
    if [ "$code" = "$2" ]; then echo "  ✅ $1（$3 → $code）"; else echo "  ❌ $1（$3 → $code、期待 $2）"; ok=0; fi
  }
  check 'PWA のシェル' 200 /
  check '音声画面' 200 /voice
  check '管理画面' 200 /admin/
  check '未ログインの API は 401' 401 /api/me
  local model="" dir
  for dir in "$ROOT"/data/tts-models/*/; do
    [ -d "$dir" ] && [ "${dir%.partial/}" = "$dir" ] && model="$(basename "$dir")"
  done
  if [ -n "$model" ]; then check "音声モデル（$model）" 200 "/tts-models/$model/manifest.json"
  else echo "  ⚠️  data/tts-models に音声モデルがありません（音声画面は標準の読み上げになります）"; fi
  if curl -sI "$BASE/" | grep -qi "content-security-policy:.*script-src 'self' 'wasm-unsafe-eval'"; then echo "  ✅ CSP（wasm-unsafe-eval）"
  else echo "  ❌ CSP に wasm-unsafe-eval がありません"; ok=0; fi
  local logs
  # Discord のログインは HTTP が応答し始めてから数秒後になる。最大 20 秒待ってから数える。
  for _ in $(seq 1 20); do
    logs="$(journalctl --user -u "$SERVICE" --since "$since" --no-pager -o cat 2>/dev/null || true)"
    echo "$logs" | grep -q 'Discord ログイン成功' && break
    sleep 1
  done
  sleep 3
  logs="$(journalctl --user -u "$SERVICE" --since "$since" --no-pager -o cat 2>/dev/null || true)"
  if echo "$logs" | grep -q 'db migrations applied successfully'; then
    echo "  ✅ マイグレーション: $(echo "$logs" | grep -oE 'applying migration: V[0-9]+__[a-z_]+' | sed 's/applying migration: //' | paste -sd ' ' - | sed 's/^$/なし/')"
  else echo "  ❌ マイグレーションの完了ログがありません"; ok=0; fi
  echo "  ・Discord ログイン: $(echo "$logs" | grep -c 'Discord ログイン成功' || true) 件 / ERROR: $(echo "$logs" | grep -c ' ERROR ' || true) 件"
  [ "$ok" = 1 ]
}

rollback() {
  local dir="${1:-}"
  [ -n "$dir" ] && [ -x "$dir/yuuka" ] && [ -d "$dir/public" ] || die "usage: $0 rollback <deploy/prod/backups/...>（yuuka と public を含む退避フォルダ）"
  [ -f "$dir/data/yuuka.db" ] || die "$dir/data/yuuka.db がありません"
  echo "⚠️  $dir の状態へ戻します。DB も退避時点に戻るため、それ以降の書き込みは消えます。"
  read -r -p '続けるなら yes と入力: ' answer
  [ "$answer" = yes ] || die '中止しました'
  local ts; ts="$(date -u +%Y%m%dT%H%M%SZ)"
  local since; since="$(date '+%Y-%m-%d %H:%M:%S')"
  systemctl --user stop "$SERVICE"
  snapshot "$PROD/backups/${ts}-before-rollback"
  install -m 755 "$dir/yuuka" "$PROD/bin/yuuka.new" && mv "$PROD/bin/yuuka.new" "$PROD/bin/yuuka"
  rm -rf "$ROOT/dist/public.old" && mv "$ROOT/dist/public" "$ROOT/dist/public.old" && cp -a "$dir/public" "$ROOT/dist/public" && rm -rf "$ROOT/dist/public.old"
  rm -f "$ROOT/data/yuuka.db-wal" "$ROOT/data/yuuka.db-shm"
  cp -a "$dir/data/." "$ROOT/data/"
  cp -a "$dir"/deployed-revision "$dir"/deployed-frontend-revision "$PROD/" 2>/dev/null || true
  systemctl --user start "$SERVICE"
  wait_healthy && echo "✅ 戻しました（今の状態は $PROD/backups/${ts}-before-rollback に退避）" || die "起動を確認できません: journalctl --user -u $SERVICE --since '$since'"
}

deploy() {
  [ "$(git branch --show-current)" = develop ] || die '本体のチェックアウトが develop ではありません'
  git diff --quiet HEAD -- || die '追跡中のファイルに未コミットの変更があります'
  log 'develop を最新にする'
  git fetch -q origin develop
  git merge -q --ff-only origin/develop
  local rev prev short ts stage
  rev="$(git rev-parse HEAD)"
  prev="$(cat "$PROD/deployed-revision" 2>/dev/null || echo unknown)"
  short="$(git rev-parse --short HEAD)"
  ts="$(date -u +%Y%m%dT%H%M%SZ)"
  if [ "$rev" = "$prev" ]; then echo "本番は既に $short です（何もしません）"; return 0; fi
  echo "  $prev → $rev"
  git log --oneline "${prev}..${rev}" 2>/dev/null | head -20 || true

  log 'ビルド（Rust・管理画面・PWA）'
  pnpm install --frozen-lockfile --silent
  # dev の Vite も同じ node_modules を使っているので、消さずに lock に合わせる（ci は消してから入れ直す）。
  npm --prefix client/pwa install --no-audit --no-fund --silent
  cargo build --release --bin yuuka
  # 本番が配信中の dist/public には書かず、別の場所に作ってから入れ替える。
  stage="$ROOT/target/prod-stage-$ts"
  rm -rf "$stage" && mkdir -p "$stage"
  pnpm exec vite build --config frontend/vite.config.ts --outDir "$stage/public" --emptyOutDir --logLevel warn
  npm --prefix client/pwa run build --silent
  cp -a "$ROOT/client/pwa/dist" "$stage/public/pwa"
  install -m 755 "${CARGO_TARGET_DIR:-$ROOT/target}/release/yuuka" "$stage/yuuka"
  [ -f "$stage/public/index.html" ] && [ -f "$stage/public/pwa/index.html" ] || die '画面のビルド結果が足りません'

  log '本番を止めて退避・入れ替え'
  local backup="$PROD/backups/${ts}-develop-${short}" since
  since="$(date '+%Y-%m-%d %H:%M:%S')"
  systemctl --user stop "$SERVICE"
  snapshot "$backup"
  mv "$stage/yuuka" "$PROD/bin/yuuka.new" && mv "$PROD/bin/yuuka.new" "$PROD/bin/yuuka"
  mv "$ROOT/dist/public" "$ROOT/dist/public.old" && mv "$stage/public" "$ROOT/dist/public" && rm -rf "$ROOT/dist/public.old"
  rm -rf "$stage"
  echo "$rev" > "$PROD/deployed-revision"
  echo "$rev" > "$PROD/deployed-frontend-revision"
  systemctl --user start "$SERVICE"

  log '確認'
  local result=ok
  verify "$since" || result=failed
  cat > "$backup/deployment.json" <<JSON
{
  "revision": "$rev",
  "previousRevision": "$prev",
  "source": "$ROOT (develop)",
  "deployedAt": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "binarySha256": "$(sha256sum "$PROD/bin/yuuka" | cut -d' ' -f1)",
  "script": "deploy/prod/deploy-develop.sh",
  "verification": "$result"
}
JSON
  if [ "$result" = ok ]; then
    echo "✅ $short を本番へデプロイしました（直前の状態: $backup）"
  else
    echo "❌ 確認に失敗しました。ログ: journalctl --user -u $SERVICE --since '$since'"
    echo "   戻すなら: $0 rollback $backup"
    exit 1
  fi
}

case "${1:-deploy}" in
  deploy) deploy ;;
  rollback) rollback "${2:-}" ;;
  verify) verify "$(date -d '-10 min' '+%Y-%m-%d %H:%M:%S')" ;;
  *) die "usage: $0 [deploy | rollback <backup-dir> | verify]" ;;
esac
