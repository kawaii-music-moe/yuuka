#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
command="${1:-}"
[[ $# -gt 0 ]] && shift
if [[ $# -gt 0 ]]; then echo 'This command takes no options.' >&2; exit 2; fi
case "$command" in
  update)
    runtime="$(systemctl --user show yuuka-dev.service --property=WorkingDirectory --value)"
    if [[ ! -d "$runtime/bin" || ! -f "$runtime/bin/yuuka" ]]; then
      echo 'yuuka-dev.service must use a runtime directory containing bin/yuuka.' >&2
      exit 1
    fi
    cd "$ROOT"
    cargo build --release --bin yuuka
    # Replace atomically; a build failure leaves the running binary intact.
    install -m 755 "${CARGO_TARGET_DIR:-$ROOT/target}/release/yuuka" "$runtime/bin/yuuka.new"
    mv "$runtime/bin/yuuka.new" "$runtime/bin/yuuka"
    systemctl --user restart yuuka-dev.service
    ;;
  stop)
    systemctl --user stop yuuka-dev-vite.service 2>/dev/null || {
      if systemctl --user is-active --quiet yuuka-dev-vite.service; then exit 1; fi
    }
    systemctl --user stop yuuka-dev.service
    # Redis is shared with production; leave it running.
    ;;
  logs) exec journalctl --user -u yuuka-dev.service -n 100 -f ;;
  logs-vite) exec journalctl --user -u yuuka-dev-vite.service -n 100 -f ;;
  *) echo 'Usage: pnpm dev:update | pnpm dev:stop | pnpm dev:logs | pnpm dev:logs:vite' >&2; exit 2 ;;
esac
