#!/usr/bin/env sh
# Uninstall hyprhalo: removes what install.sh put in place.
# The user config (~/.config/hyprhalo) is kept unless --remove-config is passed.
# Usage: ./uninstall.sh [--remove-config]
set -eu

REMOVE_CONFIG=0
for arg in "$@"; do
    case "$arg" in
        --remove-config|-c) REMOVE_CONFIG=1 ;;
        -h|--help)
            printf '%s\n' "usage: uninstall.sh [--remove-config]" ;
            exit 0 ;;
        *) printf '%s\n' "unknown argument: $arg" ; exit 1 ;;
    esac
done

BIN_DIR="$HOME/.local/bin"
ICON_DIR="$HOME/.local/share/icons/hicolor"
APP_DIR="$HOME/.local/share/applications"
CONF_DIR="$HOME/.config/hyprhalo"

say() { printf '%s\n' "==> $*"; }

say "removing executable"
rm -f "$BIN_DIR/hyprhalo"

say "removing icons"
rm -f "$ICON_DIR/512x512/apps/hyprhalo.png"
rm -f "$ICON_DIR/256x256/apps/hyprhalo.png"
rm -f "$ICON_DIR/scalable/apps/hyprhalo.png"
rm -f "$ICON_DIR/scalable/apps/hyprhalo.svg"
# Remove only dirs that are now empty (shared theme dirs are left alone).
rmdir "$ICON_DIR/512x512/apps" "$ICON_DIR/512x512"  2>/dev/null || true
rmdir "$ICON_DIR/256x256/apps" "$ICON_DIR/256x256"  2>/dev/null || true
rmdir "$ICON_DIR/scalable/apps" "$ICON_DIR/scalable" 2>/dev/null || true

say "removing desktop entry"
rm -f "$APP_DIR/hyprhalo.desktop"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APP_DIR" 2>/dev/null || true

if [ "$REMOVE_CONFIG" -eq 1 ]; then
    say "removing configuration ($CONF_DIR)"
    rm -rf "$CONF_DIR"
else
    printf '%s\n' "   keeping $CONF_DIR (pass --remove-config to delete it)"
fi

printf '%s\n' "==> done. hyprhalo has been removed."
printf '%s\n' "    (the source tree in the repo, fish_add_path entry and the"
printf '%s\n' "     ~/.local/bin PATH line in your shell rc files were left untouched)"
printf '%s\n' "    to drop the PATH entry from fish: fish_add_path --remove ~/.local/bin"