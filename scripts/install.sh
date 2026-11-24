#!/usr/bin/env bash
# Установка без root.
#   scripts/install.sh            — собрать и положить бинарник в ~/.local/bin
#   scripts/install.sh --desktop  — ярлыки в меню приложений (Walker): «Сканер карт» запускает
#                                    сервер временной службой systemd --user и открывает интерфейс,
#                                    «Сканер карт: остановить сервер» — останавливает. Без сборки.
#   scripts/install.sh --remove-desktop — убрать ярлыки и значок
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$(pwd)"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
APPS="$DATA/applications"
ICON="$DATA/icons/hicolor/scalable/apps/maps-seo-scanner.svg"
LAUNCHER="$ROOT/packaging/launcher/maps-seo-scanner-launch"
ENTRIES=(maps-seo-scanner.desktop maps-seo-scanner-stop.desktop)

case "${1:-}" in
--desktop)
    chmod +x "$LAUNCHER"
    install -Dm644 packaging/launcher/maps-seo-scanner.svg "$ICON"
    mkdir -p "$APPS"
    for f in "${ENTRIES[@]}"; do
        sed "s|@LAUNCHER@|$LAUNCHER|g" "packaging/launcher/$f" >"$APPS/$f"
    done
    update-desktop-database -q "$APPS" 2>/dev/null || true
    [ -x target/release/maps-seo-scanner ] || echo "Бинарника ещё нет — соберите: scripts/build.sh"
    echo "Ярлыки «Сканер карт» и «Сканер карт: остановить сервер» добавлены в меню приложений"
    ;;
--remove-desktop)
    for f in "${ENTRIES[@]}"; do rm -f "$APPS/$f"; done
    rm -f "$ICON"
    update-desktop-database -q "$APPS" 2>/dev/null || true
    echo "Ярлыки убраны"
    ;;
"")
    scripts/build.sh
    install -Dm755 target/release/maps-seo-scanner "$HOME/.local/bin/maps-seo-scanner"
    echo "Установлено: ~/.local/bin/maps-seo-scanner"
    ;;
*)
    echo "использование: scripts/install.sh [--desktop|--remove-desktop]" >&2
    exit 2
    ;;
esac
