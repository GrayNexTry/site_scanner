#!/usr/bin/env bash
# Архивы релиза под Linux: переносимые бинарники (glibc ≥ 2.28 — Ubuntu 20.04+, Debian 10+,
# RHEL 8+ и новее) собираются через cargo-zigbuild, интерфейс встраивается в бинарник.
#
#   scripts/dist.sh                  x86_64 и aarch64
#   scripts/dist.sh x86_64           одна архитектура
#
# Нужны: Rust (rustup), Node.js + npm, zig и cargo-zigbuild: pip install ziglang cargo-zigbuild
# Результат: dist/maps-seo-scanner-linux-<arch>.tar.gz и .sha256 (имена без версии, чтобы
# работала постоянная ссылка releases/latest/download/…; версия — maps-seo-scanner --version).
set -euo pipefail
cd "$(dirname "$0")/.."

GLIBC=2.28
ARCHES=("$@")
[ ${#ARCHES[@]} -gt 0 ] || ARCHES=(x86_64 aarch64)

command -v cargo-zigbuild >/dev/null || { echo "нет cargo-zigbuild: pip install ziglang cargo-zigbuild" >&2; exit 1; }

(cd web && { [ -d node_modules ] || npm ci --no-fund --no-audit; } && npm run build)

mkdir -p dist
for arch in "${ARCHES[@]}"; do
    triple="$arch-unknown-linux-gnu"
    rustup target add "$triple" >/dev/null
    cargo zigbuild --release --locked -p scanner-server --target "$triple.$GLIBC"

    name="maps-seo-scanner-linux-$arch"
    stage="dist/$name"
    rm -rf "$stage"
    mkdir -p "$stage/launcher"
    install -m755 "target/$triple/release/maps-seo-scanner" "$stage/"
    install -m755 packaging/install.sh "$stage/install.sh"
    install -m755 packaging/launcher/maps-seo-scanner-launch "$stage/launcher/"
    install -m644 packaging/launcher/*.desktop packaging/launcher/maps-seo-scanner.svg "$stage/launcher/"
    install -m644 README.md LICENSE "$stage/"
    tar -C dist --owner=0 --group=0 -czf "dist/$name.tar.gz" "$name"
    (cd dist && sha256sum "$name.tar.gz" >"$name.tar.gz.sha256")
    rm -rf "$stage"
    echo "Готово: dist/$name.tar.gz"
done
