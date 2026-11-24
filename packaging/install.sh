#!/usr/bin/env bash
# Установка сканера карт из архива релиза — без root, всё в домашнем каталоге.
#
#   ./install.sh               бинарник в ~/.local/bin и ярлыки «Сканер карт» в меню приложений
#   ./install.sh --no-desktop  только бинарник
#   ./install.sh --uninstall   убрать бинарник, лаунчер, ярлыки и значок (базу сканов не трогает)
#
# Каталог для бинарника можно сменить: MAPS_SEO_SCANNER_BIN_DIR=/путь ./install.sh
set -euo pipefail
cd "$(dirname "$(readlink -f "$0")")"

BIN_DIR="${MAPS_SEO_SCANNER_BIN_DIR:-$HOME/.local/bin}"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
APPS="$DATA/applications"
ICON="$DATA/icons/hicolor/scalable/apps/maps-seo-scanner.svg"
LAUNCHER="$BIN_DIR/maps-seo-scanner-launch"
ENTRIES=(maps-seo-scanner.desktop maps-seo-scanner-stop.desktop)

install_bin() {
    install -Dm755 maps-seo-scanner "$BIN_DIR/maps-seo-scanner"
    echo "Бинарник: $BIN_DIR/maps-seo-scanner ($("$BIN_DIR/maps-seo-scanner" --version))"
}

install_desktop() {
    # Лаунчер берёт бинарник, лежащий рядом с ним, поэтому кладём их в один каталог.
    install -Dm755 launcher/maps-seo-scanner-launch "$LAUNCHER"
    install -Dm644 launcher/maps-seo-scanner.svg "$ICON"
    mkdir -p "$APPS"
    for f in "${ENTRIES[@]}"; do
        sed "s|@LAUNCHER@|$LAUNCHER|g" "launcher/$f" >"$APPS/$f"
    done
    update-desktop-database -q "$APPS" 2>/dev/null || true
    echo "Ярлыки «Сканер карт» и «Сканер карт: остановить сервер» добавлены в меню приложений"
    local missing=()
    for c in systemd-run curl xdg-open; do command -v "$c" >/dev/null || missing+=("$c"); done
    if [ ${#missing[@]} -gt 0 ]; then
        echo "Внимание: ярлыкам нужны ${missing[*]} — без них запускайте из консоли: maps-seo-scanner --open" >&2
    fi
}

uninstall() {
    rm -f "$BIN_DIR/maps-seo-scanner" "$LAUNCHER" "$ICON"
    for f in "${ENTRIES[@]}"; do rm -f "$APPS/$f"; done
    update-desktop-database -q "$APPS" 2>/dev/null || true
    echo "Удалено. База сканов осталась в $DATA/maps-seo-scanner — удалите вручную, если не нужна."
}

case "${1:-}" in
"")
    install_bin
    install_desktop
    ;;
--no-desktop)
    install_bin
    ;;
--uninstall)
    uninstall
    exit 0
    ;;
*)
    echo "использование: ./install.sh [--no-desktop|--uninstall]" >&2
    exit 2
    ;;
esac

case ":$PATH:" in
*":$BIN_DIR:"*) echo "Запуск: maps-seo-scanner --open" ;;
*)
    echo "Каталога $BIN_DIR нет в PATH. Добавьте в ~/.bashrc (или ~/.zshrc):"
    echo "  export PATH=\"$BIN_DIR:\$PATH\""
    echo "Пока можно так: $BIN_DIR/maps-seo-scanner --open"
    ;;
esac
