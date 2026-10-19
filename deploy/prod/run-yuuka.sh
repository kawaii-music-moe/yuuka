#!/bin/sh
set -eu
YUUKA_ENCRYPTION_SECRET="$(cat /home/suki/apps/yuuka/deploy/prod/secret.key)"
export YUUKA_ENCRYPTION_SECRET
export YUUKA_RUST_CRON=1
export YUUKA_RUST_DISCORD=1
export CHROME_EXECUTABLE_PATH=/home/suki/.nix-profile/bin/chromium
exec /home/suki/apps/yuuka/target/release/yuuka
