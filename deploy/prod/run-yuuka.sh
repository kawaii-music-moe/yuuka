#!/bin/sh
# 本番 yuuka.service の ExecStart（deploy/systemd/yuuka.service.example）。
# 暗号化シークレットを deploy/prod/secret.key から環境変数にして、deploy/prod/bin/yuuka を起動する。
# secret.key.new があれば鍵のローテーション用に YUUKA_ENCRYPTION_SECRET_NEW として渡す（deploy/README.md）。
set -eu
PROD="$(cd "$(dirname "$0")" && pwd)"
YUUKA_ENCRYPTION_SECRET="$(cat "$PROD/secret.key")"
export YUUKA_ENCRYPTION_SECRET
if [ -f "$PROD/secret.key.new" ]; then
  YUUKA_ENCRYPTION_SECRET_NEW="$(cat "$PROD/secret.key.new")"
  export YUUKA_ENCRYPTION_SECRET_NEW
fi
# 本番は定期ジョブと Discord の応答を受け持つ（dev は YUUKA_RUST_CRON=0）。
export YUUKA_RUST_CRON="${YUUKA_RUST_CRON:-1}"
export YUUKA_RUST_DISCORD="${YUUKA_RUST_DISCORD:-1}"
# Nix で入れた Chromium は既知のパスに無いので、あればそれを使う。
if [ -z "${CHROME_EXECUTABLE_PATH:-}" ] && [ -x "$HOME/.nix-profile/bin/chromium" ]; then
  export CHROME_EXECUTABLE_PATH="$HOME/.nix-profile/bin/chromium"
fi
exec "$PROD/bin/yuuka"
