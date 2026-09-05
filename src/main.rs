mod colors;
mod config;
mod hypr;
mod render;
mod shm;

use std::time::{Duration, Instant};

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, Region};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::calloop as calloop;
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::registry::{
    ProvidesRegistryState, RegistryState, SimpleGlobal,
};
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_registry, delegate_dispatch2, registry_handlers};
use wayland_client::delegate_noop;
use wayland_client::globals::{registry_queue_init, GlobalList};
use wayland_client::protocol::{wl_buffer, wl_output, wl_shm, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter};
use wayland_protocols_wlr::screencopy::v1::client::{zwlr_screencopy_frame_v1 as sc_frame, zwlr_screencopy_manager_v1 as sc_mgr};

use calloop::timer::{TimeoutAction, Timer};

use colors::{Mixer, Rgb, Sampler, Scene, WinRectPx};
use config::Config;
use hypr::Hypr;
use render::{clear_rect, render_glow, render_wallpaper, Rect};
use shm::ShmBuf;

const NAMESPACE: &str = "hyprhalo";

// ---------------------------------------------------------------- surfaces

struct WallpaperSurface {
    layer: LayerSurface,
    output: wl_output::WlOutput,
    scale: u32,
    logical_w: u32,
    logical_h: u32,
    viewport: Option<WpViewport>,
    buffers: Option<ShmBuf>,
    ready: bool,
    needs_initial: bool,
}

struct GlowSurface {
    layer: LayerSurface,
    output: wl_output::WlOutput,
    monitor: String,
    scale: u32,
    logical_w: u32,
    logical_h: u32,
    buffers: Option<ShmBuf>,
    _input_region: Option<Region>,
    ready: bool,
    needs_initial: bool,
    /// Union box of the halo last drawn into *each* buffer slot. Each buffer
    /// holds a halo two frames old, so switching windows must clear the slot's
    /// own band, not the one from the last displayed frame.
    per_slot_band: Vec<Option<Rect>>,
}

#[derive(Clone)]
struct Target {
    /// Window rect in real (fractional-scale-adjusted) monitor pixels,
    /// relative to the monitor origin. Converted into each consumer's own
    /// pixel space (capture buffer / glow buffer) proportionally at use time.
    win: WinRectPx,
    monitor_name: String,
    /// Monitor dimensions in real pixels (from hyprctl).
    mon_w: u32,
    mon_h: u32,
    class: String,
}

struct Capture {
    manager: Option<sc_mgr::ZwlrScreencopyManagerV1>,
    frame: Option<sc_frame::ZwlrScreencopyFrameV1>,
    buffers: Option<ShmBuf>,
    format: wl_shm::Format,
    y_invert: bool,
    failures: u32,
    disabled_until: Option<Instant>,
    /// Set when the target window moved/changed: sample once, then hold until
    /// the next change.
    need_capture: bool,
    /// Current interval between samples (s). Adapts: grows while the content
    /// stays static, snaps back to `1/capture_fps` when it changes.
    interval: f64,
    /// Average color of the previous raw sample, to detect content changes.
    last_avg: Option<Rgb>,
}

// ---------------------------------------------------------------- app state

struct App {
    conn: Connection,
    qh: QueueHandle<App>,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    viewporter: Option<SimpleGlobal<WpViewporter, 1>>,
    output_state: OutputState,
    registry_state: RegistryState,
    hypr: Option<Hypr>,
    config: Config,

    wallpapers: Vec<WallpaperSurface>,
    glow: Option<GlowSurface>,

    target: Option<Target>,
    target_output: Option<wl_output::WlOutput>,
    /// When the last target window was lost; the wallpaper keeps the captured
    /// colors for `idle_timeout_s` after this before drifting to idle.
    target_lost_at: Option<Instant>,
    /// Continuously-eased scene actually painted to the wallpaper (moves
    /// smoothly between 5 fps captures).
    display_scene: Scene,
    display_clock: Option<Instant>,
    captured: Scene,
    sampler: Sampler,
    mixer: Mixer,
    capture: Capture,

    last_wall_pal: u64,
    last_glow_pal: u64,
    last_border: Option<Rgb>,
    border_size_sent: bool,
    exit: bool,
}

impl App {
    fn load_config() -> Config {
        let path = config::default_config_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => match toml::from_str::<Config>(&s) {
                Ok(c) => {
                    log::info!("loaded config from {}", path.display());
                    c
                }
                Err(e) => {
                    log::warn!("invalid config {} ({}); using defaults", path.display(), e);
                    Config::default()
                }
            },
            Err(_) => {
                log::info!("no config at {}, using defaults", path.display());
                Config::default()
            }
        }
    }

    fn idle_rgb(&self) -> Rgb {
        let c = &self.config.idle_color;
        Rgb::from_bytes(c[0], c[1], c[2])
    }

    fn build_scene(idle: Rgb, segs: (u32, u32, u32, u32)) -> Scene {
        let n = (segs.0 + segs.1 + segs.2 + segs.3) as usize;
        Scene { ring: vec![idle; n], top: idle, right: idle, bottom: idle, left: idle, avg: idle }
    }

    fn new(
        conn: Connection,
        qh: QueueHandle<App>,
        compositor: CompositorState,
        layer_shell: LayerShell,
        shm: Shm,
        viewporter: Option<SimpleGlobal<WpViewporter, 1>>,
        globals: &GlobalList,
        config: Config,
    ) -> App {
        let hypr = Hypr::connect();
        let idle = Self::idle_from(&config);

        let g = &config.glow;
        let segs = (g.top_segments, g.right_segments, g.bottom_segments, g.left_segments);
        let captured = Self::build_scene(idle, segs);

        let c = &config.capture;
        let sampler = Sampler {
            top_segments: g.top_segments,
            right_segments: g.right_segments,
            bottom_segments: g.bottom_segments,
            left_segments: g.left_segments,
            band: c.edge_band_px,
            skip: c.border_skip_px,
            stride: c.sample_stride,
        };
        let tau = if config.smoothing > 0.0 { config.smoothing * 0.30 } else { 0.0 };
        let mixer = Mixer::new(tau, config.brightness, config.saturation_boost, config.min_luminance);

        let capture = Capture {
            manager: None,
            frame: None,
            buffers: None,
            format: wl_shm::Format::Argb8888,
            y_invert: false,
            failures: 0,
            disabled_until: None,
            need_capture: false,
            interval: 1.0 / config.capture_fps.max(0.5),
            last_avg: None,
        };

        let output_state = OutputState::new(globals, &qh);
        let registry_state = RegistryState::new(globals);

        App {
            conn,
            qh,
            compositor,
            layer_shell,
            shm,
            viewporter,
            output_state,
            registry_state,
            hypr,
            config,
            wallpapers: Vec::new(),
            glow: None,
            target: None,
            target_output: None,
            target_lost_at: None,
            display_scene: captured.clone(),
            display_clock: None,
            captured,
            sampler,
            mixer,
            capture,
            last_wall_pal: 0,
            last_glow_pal: 0,
            last_border: None,
            border_size_sent: false,
            exit: false,
        }
    }

    fn idle_from(cfg: &Config) -> Rgb {
        Rgb::from_bytes(cfg.idle_color[0], cfg.idle_color[1], cfg.idle_color[2])
    }

    fn output_by_name(&self, name: &str) -> Option<wl_output::WlOutput> {
        self.output_state
            .outputs()
            .find(|o| self.output_state.info(o).and_then(|i| i.name) == Some(name.to_string()))
    }

    // ------------------------------------------------------------ geometry

    fn geometry_tick(&mut self) {
        let Some(hypr) = self.hypr.as_ref() else { return };
        let cfg = &self.config;
        let min = cfg.min_window_size;

        let window = if cfg.window_class.is_empty() {
            hypr.active_window().filter(|w| w.w >= min[0] as i32 && w.h >= min[1] as i32)
        } else {
            hypr.clients()
                .into_iter()
                .filter(|w| w.class == cfg.window_class && w.mapped && !w.minimized)
                .find(|w| w.w >= min[0] as i32 && w.h >= min[1] as i32)
        };

        let Some(w) = window else {
            if self.target.is_some() {
                log::info!("target window lost");
                self.reset_target();
            }
            return;
        };

        let Some(mon) = hypr
            .monitors()
            .into_iter()
            .find(|m| m.name == w.monitor || m.id.to_string() == w.monitor)
        else {
            log::warn!("window monitor {:?} not found in monitors list", w.monitor);
            self.reset_target();
            return;
        };

        // Hyprland window geometry is already in real monitor pixels
        // (fractional-scale-adjusted), relative to the monitor origin.
        // Each consumer (capture buffer, glow buffer) rescales it
        // proportionally into its own pixel space.
        let win = WinRectPx {
            x: w.x - mon.x,
            y: w.y - mon.y,
            w: w.w as u32,
            h: w.h as u32,
        };

        let changed = match &self.target {
            Some(t) => {
                t.monitor_name != mon.name
                    || t.win.x != win.x
                    || t.win.y != win.y
                    || t.win.w != win.w
                    || t.win.h != win.h
            }
            None => true,
        };

        if changed {
            log::debug!(
                "target {} ({}) at {},{}+{}x{} on {}",
                w.class,
                w.title,
                win.x,
                win.y,
                win.w,
                win.h,
                mon.name
            );
            self.target = Some(Target {
                win,
                monitor_name: mon.name.clone(),
                mon_w: mon.width as u32,
                mon_h: mon.height as u32,
                class: w.class.clone(),
            });
            self.target_output = self.output_by_name(&mon.name);
            if self.config.glow.enabled {
                self.ensure_glow_surface(&mon.name);
            }
            self.capture.need_capture = true;
            self.capture.interval = 1.0 / self.config.capture_fps.max(0.5);
            self.capture.failures = 0;
            self.target_lost_at = None;
        }
    }

    fn reset_target(&mut self) {
        self.target = None;
        self.target_output = None;
        self.target_lost_at = Some(Instant::now());
        self.capture.need_capture = false;
        // reset the window border to the idle tint
        if self.config.border.sync && self.last_border.is_some() {
            if let Some(hypr) = self.hypr.as_ref() {
                let idle = Self::idle_from(&self.config);
                let hex = format!(
                    "0xff{:02x}{:02x}{:02x}",
                    (idle.r * 255.0) as u32,
                    (idle.g * 255.0) as u32,
                    (idle.b * 255.0) as u32
                );
                let _ = hypr.dispatch(&format!("setprop class:* border_color {hex}"));
                self.last_border = None;
            }
        }
    }

    // ------------------------------------------------------------ capture

    fn capture_tick(&mut self) {
        let enabled = self.config.glow.enabled || self.config.wallpaper.enabled || self.config.border.sync;
        if !enabled {
            return;
        }
        if self.target.is_none() {
            self.capture.need_capture = false;
            return; // no window to follow; hold colors and drift to idle
        }
        // While a target window is focused we resample periodically so the
        // wallpaper follows changing content (e.g. a video) even when the
        // window does not move.
        self.capture.need_capture = true;
        if let Some(until) = self.capture.disabled_until {
            if Instant::now() < until {
                return;
            }
            self.capture.disabled_until = None;
        }
        let Some(manager) = self.capture.manager.clone() else { return }; // retry next tick
        // The target's wl_output may only be resolvable by name a moment after
        // the window is first seen (output names arrive via xdg_output); retry
        // until it resolves.
        let output = match &self.target_output {
            Some(o) => o.clone(),
            None => {
                let Some(target) = self.target.as_ref() else { return };
                let Some(o) = self.output_by_name(&target.monitor_name) else {
                    self.capture.need_capture = true;
                    return;
                };
                self.target_output = Some(o.clone());
                o
            }
        };
        if self.capture.frame.is_some() {
            return; // a request is already in flight, retry once it completes
        }
        self.capture.need_capture = false;
        self.capture.frame = Some(manager.capture_output(0, &output, &self.qh, ()));
        log::debug!("requested screencopy frame");
    }

    fn on_capture_buffer(&mut self, format: wl_shm::Format, width: u32, height: u32) {
        log::debug!("screencopy buffer {format:?} {width}x{height}");
        let need_resize = match &self.capture.buffers {
            Some(b) => b.width != width || b.height != height,
            None => true,
        };
        if need_resize {
            match ShmBuf::new(&self.shm, &self.qh, width, height, 1, format) {
                Ok(buf) => self.capture.buffers = Some(buf),
                Err(e) => {
                    log::warn!("failed to create capture buffer: {e}");
                    return;
                }
            }
            self.capture.format = format;
        }
        let Some(frame) = self.capture.frame.clone() else { return };
        let Some(buffers) = self.capture.buffers.as_ref() else { return };
        frame.copy(&buffers.buffers[0]);
    }

    fn on_capture_ready(&mut self) {
        if let Some(f) = self.capture.frame.take() {
            f.destroy();
        }
        self.capture.failures = 0;

        let raw: Option<Scene> = {
            let cap = &mut self.capture;
            let target = self.target.clone();
            match (cap.buffers.as_mut(), target) {
                (Some(bufs), Some(t)) => {
                    let mmap = &bufs.pool.mmap()[..];
                    let sx = bufs.width as f64 / t.mon_w as f64;
                    let sy = bufs.height as f64 / t.mon_h as f64;
                    let w = WinRectPx {
                        x: (t.win.x as f64 * sx) as i32,
                        y: (t.win.y as f64 * sy) as i32,
                        w: (t.win.w as f64 * sx) as u32,
                        h: (t.win.h as f64 * sy) as u32,
                    };
                    let frame = colors::Frame {
                        buf: mmap,
                        width: bufs.width,
                        height: bufs.height,
                        stride: bufs.stride.max(0) as usize,
                        y_invert: cap.y_invert,
                    };
                    self.sampler.sample(&frame, &w)
                }
                _ => None,
            }
        };

        if let Some(raw) = raw {
            // Adaptive capture rate: while the scene stays static, lengthen the
            // interval so idle desktops cost almost nothing; snap back to the
            // configured rate the moment the content changes.
            let base = 1.0 / self.config.capture_fps.max(0.5);
            let still = match self.capture.last_avg {
                Some(p) => raw.avg.dist2(&p) < 0.0035,
                None => false,
            };
            self.capture.last_avg = Some(raw.avg);
            self.capture.interval = if still {
                (self.capture.interval * 1.6).min(1.0)
            } else {
                base
            };

            let scene = self.mixer.feed(&raw, Instant::now());
            log::debug!(
                "frame captured: avg=({:.3},{:.3},{:.3}) top=({:.3},{:.3},{:.3}) interval={:.2}s",
                scene.avg.r,
                scene.avg.g,
                scene.avg.b,
                scene.top.r,
                scene.top.g,
                scene.top.b,
                self.capture.interval
            );
            self.captured = scene;
            self.sync_border();
        } else {
            log::debug!("frame ready but no target/buffer to sample");
        }
    }

    fn sync_border(&mut self) {
        let cfg = &self.config;
        if !cfg.border.sync {
            return;
        }
        let Some(target) = self.target.as_ref() else { return };
        if target.class.is_empty() {
            return;
        }
        let Some(hypr) = self.hypr.as_ref() else { return };
        if cfg.border.border_size > 0 && !self.border_size_sent
            && hypr.dispatch(&format!("setprop class:{} border_size {}", target.class, cfg.border.border_size))
        {
            self.border_size_sent = true;
        }
        let color = self.captured.top;
        let delta = self.last_border.map(|p| color.dist2(&p)).unwrap_or(f32::MAX);
        let threshold = (cfg.border.min_delta * cfg.border.min_delta) as f32;
        if delta < threshold {
            return;
        }
        let hex = format!(
            "0xff{:02x}{:02x}{:02x}",
            (color.r * 255.0) as u32,
            (color.g * 255.0) as u32,
            (color.b * 255.0) as u32
        );
        if hypr.dispatch(&format!("setprop class:{} border_color {hex}", target.class)) {
            self.last_border = Some(color);
        }
    }

    // ------------------------------------------------------------ rendering

fn quantize(c: Rgb) -> u32 {
    // 6-bit buckets: sub-threshold color drift doesn't trigger repaints.
    ((((c.r * 255.0) as u32) >> 2) << 16) | ((((c.g * 255.0) as u32) >> 2) << 8) | (((c.b * 255.0) as u32) >> 2)
}

    fn wall_palette(s: &Scene) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for c in [s.top, s.right, s.bottom, s.left, s.avg] {
            h ^= Self::quantize(c) as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    fn glow_palette(s: &Scene) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for c in &s.ring {
            h ^= Self::quantize(*c) as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    // Smoothing time constant for the continuous display easing (s): the
    // `--profile` presets and/or the `display_tau_s` config override it.
    // Current value comes from `self.config.display_tau_s`.

    fn effective_scene(&mut self, now: Instant) -> Scene {
        let idle_in = self.config.idle_timeout_s;
        // Desired target colors at this instant.
        let target = match &self.target {
            // While a target window is present the wallpaper follows its
            // captured colors (updated by `capture_tick`).
            Some(_) => self.captured.clone(),
            None => {
                let idle = self.idle_rgb();
                // After the window is gone, first keep the last captured
                // colors, then drift to idle once `idle_timeout_s` has passed.
                match self.target_lost_at {
                    Some(t0) => {
                        let age = now.duration_since(t0).as_secs_f64();
                        if age < idle_in {
                            self.captured.clone()
                        } else {
                            colors::ease_to(&self.captured, idle, 1.0)
                        }
                    }
                    None => colors::ease_to(&self.captured, idle, 1.0),
                }
            }
        };

        // Continuously ease the displayed scene toward that target so colors
        // flow smoothly between the discrete captures.
        let k = match self.display_clock {
            Some(t) => {
                let dt = now.duration_since(t).as_secs_f64();
                let tau = self.config.display_tau_s.max(0.02);
                (1.0 - (-dt / tau).exp()).clamp(0.0, 1.0) as f32
            }
            None => 1.0,
        };
        self.display_clock = Some(now);
        self.display_scene = colors::ease_scene(&self.display_scene, &target, k);
        self.display_scene.clone()
    }

    fn render_all(&mut self) {
        let now = Instant::now();
        let scene = self.effective_scene(now);

        if self.config.wallpaper.enabled {
            let pal = Self::wall_palette(&scene);
            let mut drew = false;
            for wp in &mut self.wallpapers {
                if !wp.ready || (!wp.needs_initial && pal == self.last_wall_pal) {
                    continue;
                }
                drew |= draw_wallpaper(wp, &scene, &self.config.wallpaper);
            }
            if drew {
                self.last_wall_pal = pal;
            }
        }

        if self.config.glow.enabled {
            let pal = Self::glow_palette(&scene);
            let target = self.target.as_ref();
            if let Some(gs) = self.glow.as_mut() {
                if gs.ready {
                    let has_band = gs.per_slot_band.iter().any(Option::is_some);
                    let should = match target {
                        Some(_) => gs.needs_initial || pal != self.last_glow_pal || has_band,
                        None => has_band || gs.needs_initial,
                    };
                    if should && draw_glow(gs, target, &scene, &self.config.glow) {
                        self.last_glow_pal = pal;
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------ surfaces

    fn ensure_glow_surface(&mut self, monitor: &str) {
        if self.glow.as_ref().is_some_and(|g| g.monitor == monitor) {
            return;
        }
        if let Some(old) = self.glow.take() {
            old.layer.wl_surface().destroy();
            self.glow = None;
        }
        let Some(output) = self.output_by_name(monitor) else { return };
        let info = self.output_state.info(&output);
        let scale = info.map(|i| i.scale_factor.max(1)).unwrap_or(1) as u32;

        let surface = self.compositor.create_surface(&self.qh);
        let layer = self
            .layer_shell
            .create_layer_surface(&self.qh, surface, Layer::Overlay, Some(NAMESPACE), Some(&output));
        layer.set_anchor(Anchor::all());
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        let _ = layer.set_buffer_scale(scale);
        layer.set_size(0, 0);
        layer.commit();

        // click-through: empty input region, kept alive for the surface lifetime
        let input_region = Region::new(&self.compositor).ok();
        if let Some(r) = &input_region {
            layer.wl_surface().set_input_region(Some(r.wl_region()));
        }

        self.glow = Some(GlowSurface {
            layer,
            output,
            monitor: monitor.to_string(),
            scale,
            logical_w: 0,
            logical_h: 0,
            buffers: None,
            _input_region: input_region,
            ready: false,
            needs_initial: true,
            per_slot_band: Vec::new(),
        });
        log::debug!("glow overlay created on {monitor} (scale {scale})");
    }
}

// ---------------------------------------------------------------- handlers

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        if !self.config.wallpaper.enabled {
            return;
        }
        let scale = self.output_state.info(&output).map(|i| i.scale_factor.max(1)).unwrap_or(1) as u32;
        let name = self.output_state.info(&output).and_then(|i| i.name.clone());
        match name {
            Some(n) => log::info!("new output {n} (scale {scale})"),
            _ => log::info!("new output (scale {scale})"),
        }

        let surface = self.compositor.create_surface(&self.qh);
        let layer = self
            .layer_shell
            .create_layer_surface(&self.qh, surface, Layer::Background, Some(NAMESPACE), Some(&output));
        layer.set_anchor(Anchor::all());
        layer.set_size(0, 0);
        let _ = layer.set_buffer_scale(scale);
        layer.commit();

        self.wallpapers.push(WallpaperSurface {
            layer,
            output,
            scale,
            logical_w: 0,
            logical_h: 0,
            viewport: None,
            buffers: None,
            ready: false,
            needs_initial: true,
        });
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.wallpapers.retain(|w| w.output.id() != output.id());
        if let Some(g) = &self.glow {
            if g.output.id() == output.id() {
                self.glow = None;
                self.target_output = None;
            }
        }
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        log::info!("layer surface closed, exiting");
        self.exit = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let (w, h) = configure.new_size;
        let surface = layer.wl_surface().clone();
        log::debug!("layer configure for surface {}: {w}x{h}", surface.id());

        for wp in &mut self.wallpapers {
            if wp.layer.wl_surface().id() == surface.id() {
                wp.logical_w = if w > 0 { w } else { wp.logical_w };
                wp.logical_h = if h > 0 { h } else { wp.logical_h };
                setup_wallpaper_buffers(&self.shm, self.viewporter.as_ref(), wp, self.config.wallpaper.scale_down, qh);
                wp.ready = true;
                wp.needs_initial = true;
                return;
            }
        }

        if let Some(gs) = self.glow.as_mut() {
            if gs.layer.wl_surface().id() == surface.id() {
                gs.logical_w = if w > 0 { w } else { gs.logical_w };
                gs.logical_h = if h > 0 { h } else { gs.logical_h };
                setup_glow_buffers(&self.shm, gs, qh);
                gs.ready = true;
            }
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_registry!(App);

impl Dispatch<wl_buffer::WlBuffer, ()> for App {
    fn event(
        state: &mut Self,
        proxy: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            for wp in &mut state.wallpapers {
                if let Some(b) = &mut wp.buffers {
                    b.mark_released(proxy);
                }
            }
            if let Some(g) = &mut state.glow {
                if let Some(b) = &mut g.buffers {
                    b.mark_released(proxy);
                }
            }
            if let Some(c) = &mut state.capture.buffers {
                c.mark_released(proxy);
            }
        }
    }
}

impl Dispatch<sc_mgr::ZwlrScreencopyManagerV1, ()> for App {
    fn event(
        _state: &mut Self,
        _proxy: &sc_mgr::ZwlrScreencopyManagerV1,
        _event: <sc_mgr::ZwlrScreencopyManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<sc_frame::ZwlrScreencopyFrameV1, ()> for App {
    fn event(
        state: &mut Self,
        _proxy: &sc_frame::ZwlrScreencopyFrameV1,
        event: sc_frame::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            sc_frame::Event::Buffer { format, width, height, .. } => {
                let Ok(format) = format.into_result() else {
                    log::warn!("unsupported screencopy format, skipping");
                    return;
                };
                state.on_capture_buffer(format, width, height);
            }
            sc_frame::Event::Flags { flags } => {
                let flags = flags.into_result().unwrap_or(sc_frame::Flags::empty());
                state.capture.y_invert = flags.contains(sc_frame::Flags::YInvert);
            }
            sc_frame::Event::Ready { .. } => {
                state.on_capture_ready();
            }
            sc_frame::Event::Failed => {
                if let Some(f) = state.capture.frame.take() {
                    f.destroy();
                }
                state.capture.failures += 1;
                if state.capture.failures >= state.config.capture.max_failures {
                    log::warn!("screencopy failed too many times, backing off");
                    state.capture.disabled_until = Some(Instant::now() + Duration::from_secs(5));
                    state.capture.failures = 0;
                }
            }
            _ => {}
        }
    }
}

delegate_noop!(App: WpViewporter);
delegate_noop!(App: WpViewport);

delegate_dispatch2!(App);

// ---------------------------------------------------------------- helpers

fn setup_wallpaper_buffers(
    shm: &Shm,
    viewporter: Option<&SimpleGlobal<WpViewporter, 1>>,
    wp: &mut WallpaperSurface,
    scale_down_cfg: u32,
    qh: &QueueHandle<App>,
) {
    let scale_down = if viewporter.is_some() { scale_down_cfg.max(1) } else { 1 };
    let phys_w = wp.logical_w * wp.scale;
    let phys_h = wp.logical_h * wp.scale;
    let bw = phys_w.div_ceil(scale_down);
    let bh = phys_h.div_ceil(scale_down);
    if bw == 0 || bh == 0 {
        return;
    }
    match ShmBuf::new(shm, qh, bw, bh, 2, wl_shm::Format::Argb8888) {
        Ok(b) => wp.buffers = Some(b),
        Err(e) => {
            log::warn!("wallpaper buffer: {e}");
            return;
        }
    }
    if let Some(vp) = viewporter {
        if let Ok(vp) = vp.get() {
            let viewport = vp.get_viewport(wp.layer.wl_surface(), qh, ());
            viewport.set_source(0.0, 0.0, bw as f64, bh as f64);
            viewport.set_destination(wp.logical_w as i32, wp.logical_h as i32);
            wp.viewport = Some(viewport);
        }
    }
}

fn setup_glow_buffers(shm: &Shm, gs: &mut GlowSurface, qh: &QueueHandle<App>) {
    let bw = gs.logical_w * gs.scale;
    let bh = gs.logical_h * gs.scale;
    if bw == 0 || bh == 0 {
        return;
    }
    match ShmBuf::new(shm, qh, bw, bh, 2, wl_shm::Format::Argb8888) {
        Ok(b) => gs.buffers = Some(b),
        Err(e) => log::warn!("glow buffer: {e}"),
    }
    log::debug!("glow buffers {bw}x{bh}");
    gs.per_slot_band.clear();
}

fn draw_wallpaper(wp: &mut WallpaperSurface, scene: &Scene, cfg: &config::WallpaperConfig) -> bool {
    let Some(bufs) = wp.buffers.as_mut() else { return false };
    let (bw, bh) = (bufs.width, bufs.height);
    let Some((buffer, canvas)) = bufs.next_writeable() else { return false };
    render_wallpaper(
        canvas,
        bw as usize,
        bh as usize,
        scene,
        cfg.direction_blend,
        cfg.vignette,
        cfg.ambient_blend,
    );
    wp.layer.wl_surface().attach(Some(&buffer), 0, 0);
    wp.layer.wl_surface().damage_buffer(0, 0, bw as i32, bh as i32);
    wp.layer.commit();
    wp.needs_initial = false;
    true
}

fn draw_glow(gs: &mut GlowSurface, target: Option<&Target>, scene: &Scene, cfg: &config::GlowConfig) -> bool {
    let Some(bufs) = gs.buffers.as_mut() else { return false };
    let (bw, bh) = (bufs.width, bufs.height);
    let slot = (bufs.current + 1) % bufs.buffers.len();
    let Some((buffer, canvas)) = bufs.next_writeable() else { return false };
    if slot >= gs.per_slot_band.len() {
        gs.per_slot_band.resize(slot + 1, None);
    }
    let prev = gs.per_slot_band[slot];
    let new_band = match target {
        Some(t) => {
            let sx = bw as f64 / t.mon_w as f64;
            let sy = bh as f64 / t.mon_h as f64;
            let w = WinRectPx {
                x: (t.win.x as f64 * sx) as i32,
                y: (t.win.y as f64 * sy) as i32,
                w: (t.win.w as f64 * sx) as u32,
                h: (t.win.h as f64 * sy) as u32,
            };
            render_glow(
                canvas,
                bw as usize,
                bh as usize,
                &w,
                scene,
                cfg.top_segments as usize,
                cfg.right_segments as usize,
                cfg.bottom_segments as usize,
                (cfg.radius_px as i32) * gs.scale as i32,
                (cfg.inner_px as i32) * gs.scale as i32,
                cfg.intensity,
                prev,
            )
        }
        None => {
            if let Some(p) = prev {
                clear_rect(canvas, bw as usize, &p);
            }
            None
        }
    };
    gs.layer.wl_surface().attach(Some(&buffer), 0, 0);
    match new_band {
        Some(b) => {
            let all = Rect::bounding(prev, Some(b)).unwrap_or(b);
            if all.x1 > all.x0 && all.y1 > all.y0 {
                gs.layer.wl_surface().damage_buffer(all.x0.max(0), all.y0.max(0), all.x1 - all.x0, all.y1 - all.y0);
            }
        }
        None => {
            gs.layer.wl_surface().damage_buffer(0, 0, bw as i32, bh as i32);
        }
    }
    gs.layer.commit();
    gs.per_slot_band[slot] = new_band;
    gs.needs_initial = false;
    true
}

// ---------------------------------------------------------------- main

/// Parse `--profile <name>` and `--help`. Profile names come from the
/// `[profiles]` section of the config file. Returns:
/// `None` = exit (bad args or help shown), `Some(None)` = run with no profile,
/// `Some(Some(name))` = run with the given profile.
fn parse_args() -> Option<Option<String>> {
    use std::process::exit;
    let mut profile: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--profile" | "-p" => {
                let Some(v) = args.next() else {
                    eprintln!("error: --profile needs a profile name from the config's [profiles] section");
                    print_usage();
                    exit(1);
                };
                profile = Some(v);
            }
            "--help" | "-h" => {
                print_usage();
                return None;
            }
            other => {
                eprintln!("error: unknown argument {other:?}");
                print_usage();
                exit(1);
            }
        }
    }
    Some(profile)
}

fn print_usage() {
    eprintln!(
        "hyprhalo — ambilight for Hyprland\n
USAGE:\n    hyprhalo [OPTIONS]\n
OPTIONS:\n    -p, --profile <name>  apply one of the profiles from the config's [profiles]
                           section (low, normal, very-smooth by default)\n
        --help, -h         print this help\n
Profiles are defined in the [profiles] section and override the top-level
capture_fps / smoothing / display_tau_s values while selected."
    );
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .init();

    let profile = match parse_args() {
        Some(p) => p,
        None => return,
    };

    let mut config = App::load_config();
    if let Some(name) = &profile {
        match config.apply_profile(name) {
            Ok(()) => log::info!("smoothing profile: {}", name),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }

    let conn = Connection::connect_to_env().expect("failed to connect to Wayland");
    let (globals, queue) = registry_queue_init(&conn).expect("registry init failed");
    let qh = queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor not available");
    let layer_shell = LayerShell::bind(&globals, &qh).ok();
    let Some(layer_shell) = layer_shell else {
        log::error!("compositor does not implement wlr-layer-shell; exiting");
        return;
    };
    let shm = Shm::bind(&globals, &qh).expect("wl_shm not available");
    let viewporter = SimpleGlobal::<WpViewporter, 1>::bind(&globals, &qh).ok();

    let mut app = App::new(conn.clone(), qh, compositor, layer_shell, shm, viewporter, &globals, config);

    match globals.bind(&app.qh, 1..=1, ()) {
        Ok(m) => {
            app.capture.manager = Some(m);
            log::info!("zwlr_screencopy_manager_v1 bound");
        }
        Err(e) => {
            log::warn!("zwlr_screencopy_manager_v1 unavailable ({e}); only static wallpaper");
        }
    }

    let mut event_loop: calloop::EventLoop<App> = calloop::EventLoop::try_new().unwrap();
    let handle = event_loop.handle();

    let _geom_timer = handle
        .insert_source(Timer::immediate(), |_, _, data: &mut App| {
            data.geometry_tick();
            TimeoutAction::ToDuration(Duration::from_millis(data.config.geometry_poll_ms.max(50)))
        })
        .unwrap();

    let _cap_timer = handle
        .insert_source(Timer::immediate(), |_, _, data: &mut App| {
            data.capture_tick();
            TimeoutAction::ToDuration(Duration::from_secs_f64(data.capture.interval.max(0.05)))
        })
        .unwrap();

    let _render_timer = handle
        .insert_source(Timer::immediate(), |_, _, data: &mut App| {
            data.render_all();
            TimeoutAction::ToDuration(Duration::from_millis(50))
        })
        .unwrap();

    WaylandSource::new(conn.clone(), queue).insert(handle).unwrap();

    while !app.exit {
        if let Err(e) = event_loop.dispatch(None, &mut app) {
            log::warn!("event loop error: {e}");
        }
        let _ = app.conn.flush();
        app.render_all();
    }
    log::info!("bye");
}