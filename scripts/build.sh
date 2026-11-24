#!/usr/bin/env bash
# Сборка: интерфейс (web/dist) встраивается в релизный бинарник.
set -euo pipefail
cd "$(dirname "$0")/.."
(cd web && { [ -d node_modules ] || npm install --no-fund --no-audit; } && npm run build)
cargo build --release -p scanner-server
echo "Готово: target/release/maps-seo-scanner"
