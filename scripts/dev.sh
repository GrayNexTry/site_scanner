#!/usr/bin/env bash
# Разработка: API на :8787 (cargo run) и интерфейс на :5173 (vite, прокси /api).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo run -p scanner-server -- --listen 127.0.0.1:8787 &
api=$!
trap 'kill $api 2>/dev/null' EXIT
cd web
[ -d node_modules ] || npm install --no-fund --no-audit
npm run dev
