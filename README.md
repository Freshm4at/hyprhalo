# hyprhalo

Ambilight for Hyprland: a software-rendered, low-CPU effect that reads the
edge colors of the active window and syncs them with a dynamic wallpaper, a
smooth halo around the window and the Hyprland window border.

Built on [wlr-screencopy](https://wayland.app/protocols/wlr-screencopy-unstable-v1)
(waiting for the compositor to deliver the buffer — no capture when there is
no target), renders into shared-memory layer surfaces and relies on
`viewporter` scaling so everything stays near-free on CPU.

## Features

- **Dynamic wallpaper** — a full-screen gradient whose colors follow the edges
  of the focused window (top / right / bottom / left + average).
- **Window halo** — a continuous rounded glow around the window. Uses a
  distance-field falloff so the four sides and the corners blend into one
  ring, with cubic interpolation between the sampled edge colors.
- **Border sync** — optionally pushes the current edge color to Hyprland
  (`hyprctl setprop border_color`) for a crisp light frame at near-zero cost.
- **Idle hold** — keeps the last captured colors for a few seconds after the
  window closes/loses focus, then drifts back to the idle color.
- **Adaptive sampling** — monotonously static content backs the capture rate
  off to ~1 s; the moment the content changes it snaps back to the profile
  rate, so videos still track smoothly.
- **Continuous easing** — the displayed colors interpolate between captures
  every render tick, so transitions flow instead of stepping.
- **Cheap** — integer math, no per-pixel divisions in the renderer, quantized
  repaints (a repaint only pays for itself when the color moved enough),
  wallpaper downscaled internally and upscaled by the compositor.

## Requirements

- Hyprland (or any compositor implementing `wlr-layer-shell`,
  `wlr-screencopy-unstable-v1` and `wp_viewporter`)
- Linux, Wayland session, `hyprctl` on `PATH` (for window geometry + border)
- Rust 1.86+ to build

## Build & run

```sh
cargo build --release
./target/release/hyprhalo            # or:
./target/release/hyprhalo -p very-smooth
```

Log verbosity comes from `RUST_LOG` (default `info`):
`RUST_LOG=hyprhalo=debug ./target/release/hyprhalo`.

### CLI

```
-t, --toggle           if an instance is already running, stop it and exit;
                       otherwise launch hyprhalo (use on hotkeys)
-p, --profile <name>   apply one of the profiles from the config's [profiles] section
-h, --help             print help
```

`--profile` applies the named profile's values over the top-level config, e.g.
`hyprhalo -p very-smooth`. `--toggle` keeps a lock file in `$XDG_RUNTIME_DIR`
so a hotkey can start/stop hyprhalo with the same key:

```lua
hl.exec_cmd("/home/you/.local/bin/hyprhalo --toggle -p very-smooth &")
```

## Configuration

The config lives in `~/.config/hyprhalo/config.toml`; the defaults are embedded
and shipped in `src/default.toml`. Copy it over and edit:

```sh
mkdir -p ~/.config/hyprhalo
cp src/default.toml ~/.config/hyprhalo/config.toml
```

Key options:

| Option | Default | Meaning |
| --- | --- | --- |
| `capture_fps` | `5.0` | Sample rate while a target window is focused |
| `smoothing` | `0.35` | Exponential color smoothing applied per capture (0 = instant) |
| `display_tau_s` | `0.25` | Continuous on-screen easing between captures (s); higher = smoother |
| `window_class` | `""` | Only target windows with this class (empty = focused window) |
| `idle_color` | `"0b1020"` | Color drifted toward when no target window is present |
| `idle_timeout_s` | `3.0` | Hold the last colors this long before drifting to idle |
| `[wallpaper]` | | `enabled`, `direction_blend`, `vignette`, `ambient_blend`, `scale_down` |
| `[glow]` | | `enabled`, `radius_px`, `intensity`, per-edge `*_segments` |
| `[border]` | | `sync` (border color), `border_size`, `min_delta` |
| `[capture]` | | `edge_band_px`, `border_skip_px`, `sample_stride`, `max_failures` |

### Profiles

Three named profiles are defined in the `[profiles]` section of the config and
selected at run time with `--profile`. Edit their numbers to tune them:

- `low` — fast response, lowest CPU cost: `capture_fps 4`, `smoothing 0.15`,
  `display_tau_s 0.10`
- `normal` — default behaviour: `5 / 0.35 / 0.25`
- `very-smooth` — slow, buttery transitions, higher sample rate: `8 / 0.60 / 0.60`

## Desktop integration

The repo ships an application icon (`assets/hyprhalo.svg`) and a desktop entry:

```sh
install -Dm644 assets/hyprhalo-512.png   ~/.local/share/icons/hicolor/512x512/apps/hyprhalo.png
install -Dm644 assets/hyprhalo-256.png   ~/.local/share/icons/hicolor/256x256/apps/hyprhalo.png
install -Dm644 assets/hyprhalo.svg       ~/.local/share/icons/hicolor/scalable/apps/hyprhalo.svg
install -Dm644 hyprhalo.desktop          ~/.local/share/applications/hyprhalo.desktop
update-desktop-database ~/.local/share/applications
```

Because the desktop entry and the process share the name `hyprhalo`, launchers,
panels and waybar associate the icon with the process automatically.

## Autostart example

`~/.config/hypr/hyprland.conf`:

```conf
exec-once = hyprhalo -p very-smooth
```

## Development

```sh
cargo build --release
cargo clippy --release
cargo test --release
```

## License

MIT