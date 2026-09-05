#!/usr/bin/env sh
# Install hyprhalo: builds the binary and wires it into the user's environment:
#   ~/.local/bin/hyprhalo            (executable / symlink)
#   ~/.local/share/icons/hicolor/    (app icon, several sizes)
#   ~/.local/share/applications/     (desktop entry)
#   ~/.config/hyprhalo/config.toml   (config, only if not already present)
# Also registers ~/.local/bin on the shell PATH when it is missing.
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
BIN_DIR="$HOME/.local/bin"
ICON_DIR="$HOME/.local/share/icons/hicolor"
APP_DIR="$HOME/.local/share/applications"
CONF_DIR="$HOME/.config/hyprhalo"

say() { printf '%s\n' "==> $*"; }

say "building release binary"
cargo build --release --manifest-path "$ROOT/Cargo.toml"

mkdir -p "$BIN_DIR" "$ICON_DIR" "$APP_DIR"

if [ ! -x "$BIN_DIR/hyprhalo" ] || [ "$ROOT/target/release/hyprhalo" -nt "$BIN_DIR/hyprhalo" ]; then
    say "installing binary -> $BIN_DIR/hyprhalo (symlink into the repo build)"
    ln -sf "$ROOT/target/release/hyprhalo" "$BIN_DIR/hyprhalo"
else
    say "binary already installed"
fi

# Icons: prefer pre-rendered PNGs, fall back to generating from the SVG.
if command -v rsvg-convert >/dev/null 2>&1; then
    gen="rsvg-convert"
elif command -v convert >/dev/null 2>&1; then
    gen="convert"
else
    gen=""
fi
install_icon() {
    size="$1"
    dst="$ICON_DIR/${size}x${size}/apps/hyprhalo.png"
    if [ -f "$ROOT/assets/hyprhalo-${size}.png" ]; then
        say "installing icon (${size}x${size})"
        install -Dm644 "$ROOT/assets/hyprhalo-${size}.png" "$dst"
    elif [ -n "$gen" ] && [ -f "$ROOT/assets/hyprhalo.svg" ]; then
        say "rendering and installing icon (${size}x${size})"
        mkdir -p "$(dirname "$dst")"
        if [ "$gen" = rsvg-convert ]; then
            rsvg-convert -w "$size" -h "$size" "$ROOT/assets/hyprhalo.svg" -o "$dst"
        else
            convert -background none "$ROOT/assets/hyprhalo.svg" -resize "${size}x${size}" "$dst"
        fi
    else
        printf '%s\n' "!! icon ${size}x${size} skipped (no asset or renderer)"
    fi
}
install_icon 512
install_icon 256

if [ -f "$ROOT/assets/hyprhalo.svg" ]; then
    say "installing scalable icon"
    install -Dm644 "$ROOT/assets/hyprhalo.svg" "$ICON_DIR/scalable/apps/hyprhalo.svg"
fi

if [ -f "$ROOT/hyprhalo.desktop" ]; then
    say "installing desktop entry"
    install -Dm644 "$ROOT/hyprhalo.desktop" "$APP_DIR/hyprhalo.desktop"
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APP_DIR"
fi

if [ ! -f "$CONF_DIR/config.toml" ]; then
    say "writing default config -> $CONF_DIR/config.toml"
    mkdir -p "$CONF_DIR"
    install -m644 "$ROOT/src/default.toml" "$CONF_DIR/config.toml"
else
    say "keeping existing config at $CONF_DIR/config.toml"
fi

# Make sure ~/.local/bin is on the PATH.
case ":$PATH:" in
    *":$HOME/.local/bin:"*) ;;
    *)
        if command -v fish >/dev/null 2>&1; then
            say "registering ~/.local/bin with fish (fish_add_path)"
            fish -c "fish_add_path \$HOME/.local/bin" 2>/dev/null || true
        fi
        if [ -f "$HOME/.bashrc" ] && ! grep -q '# hyprhalo' "$HOME/.bashrc"; then
            printf '%s\n' '
# hyprhalo: keep release binary reachable
export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.bashrc"
        fi
        if [ -f "$HOME/.zshrc" ] && ! grep -q '# hyprhalo' "$HOME/.zshrc"; then
            printf '%s\n' '
# hyprhalo: keep release binary reachable
export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.zshrc"
        fi
        ;;
esac

say "done. Run:  hyprhalo        (or: hyprhalo -p very-smooth)"
say "hint: shells opened before this install may need 'source' or a new terminal."