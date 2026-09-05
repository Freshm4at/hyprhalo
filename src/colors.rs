//! Color pipeline: sample the edges of the target window inside a captured
//! frame, average each segment, then smooth + enhance the colors.

use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Rgb {
    pub const fn zero() -> Rgb {
        Rgb { r: 0.0, g: 0.0, b: 0.0 }
    }

    pub fn from_bytes(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0 }
    }

    pub fn luma(&self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }

    /// Squared euclidean distance between two colors (0..3).
    pub fn dist2(&self, o: &Rgb) -> f32 {
        let dr = self.r - o.r;
        let dg = self.g - o.g;
        let db = self.b - o.b;
        dr * dr + dg * dg + db * db
    }
}

pub fn lerp(a: &Rgb, b: &Rgb, t: f32) -> Rgb {
    Rgb {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
    }
}

fn rgb_to_hsl(c: &Rgb) -> (f32, f32, f32) {
    let max = c.r.max(c.g).max(c.b);
    let min = c.r.min(c.g).min(c.b);
    let l = (max + min) * 0.5;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == c.r {
        (c.g - c.b) / d + if c.g < c.b { 6.0 } else { 0.0 }
    } else if max == c.g {
        (c.b - c.r) / d + 2.0
    } else {
        (c.r - c.g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 0.5 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Rgb {
    if s == 0.0 {
        return Rgb { r: l, g: l, b: l };
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    Rgb {
        r: hue_to_rgb(p, q, h + 1.0 / 3.0),
        g: hue_to_rgb(p, q, h),
        b: hue_to_rgb(p, q, h - 1.0 / 3.0),
    }
}

/// Brightness, saturation boost and minimum luminance applied to a color.
pub fn enhance(c: &Rgb, brightness: f64, sat_boost: f64, min_lum: f64) -> Rgb {
    let (h, s, l) = rgb_to_hsl(c);
    let s = (s * sat_boost as f32).clamp(0.0, 1.0);
    let mut out = hsl_to_rgb(h, s, l);
    let b = brightness as f32;
    out.r *= b;
    out.g *= b;
    out.b *= b;
    let lum = out.luma();
    if lum < min_lum as f32 && lum > 1e-4 {
        let k = (min_lum as f32) / lum;
        out.r = (out.r * k).min(1.0);
        out.g = (out.g * k).min(1.0);
        out.b = (out.b * k).min(1.0);
    }
    Rgb {
        r: out.r.clamp(0.0, 1.0),
        g: out.g.clamp(0.0, 1.0),
        b: out.b.clamp(0.0, 1.0),
    }
}

/// Geometry of the target window in *physical* (buffer) pixels.
#[derive(Clone, Copy, Debug)]
pub struct WinRectPx {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl WinRectPx {
    pub fn x1(&self) -> i32 {
        self.x + self.w as i32
    }
    pub fn y1(&self) -> i32 {
        self.y + self.h as i32
    }
}

/// A screen capture frame (RGBA bytes, ARGB8888/XRGB8888 layout).
pub struct Frame<'a> {
    pub buf: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub y_invert: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(dead_code)]
pub enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

/// All sampled colors in one pass.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    /// Colors ordered clockwise starting at the top-left corner:
    /// top (left->right), right (top->bottom), bottom (right->left), left (bottom->top).
    pub ring: Vec<Rgb>,
    pub top: Rgb,
    pub right: Rgb,
    pub bottom: Rgb,
    pub left: Rgb,
    pub avg: Rgb,
}

pub struct Sampler {
    pub top_segments: u32,
    pub right_segments: u32,
    pub bottom_segments: u32,
    pub left_segments: u32,
    /// Band thickness in physical px.
    pub band: u32,
    /// Skip distance from the window border in physical px.
    pub skip: u32,
    /// Sampling stride in physical px.
    pub stride: u32,
}

impl Sampler {
    /// Sample all edges, producing a new raw (unsmoothed) scene.
    pub fn sample(&self, frame: &Frame, win: &WinRectPx) -> Option<Scene> {
        if win.w == 0 || win.h == 0 {
            return None;
        }
        let w = frame.width as i32;
        let h = frame.height as i32;
        let x0 = win.x.clamp(0, w);
        let y0 = win.y.clamp(0, h);
        let x1 = win.x1().clamp(0, w);
        let y1 = win.y1().clamp(0, h);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let win = WinRectPx { x: x0, y: y0, w: (x1 - x0) as u32, h: (y1 - y0) as u32 };

        let band = self.band.min(win.h / 2).max(1);
        let skip = self.skip.min(band.saturating_sub(1));
        let step = self.stride.max(1);

        let mut top = Vec::new();
        let mut right = Vec::new();
        let mut bottom = Vec::new();
        let mut left = Vec::new();

        // Top edge: band of rows right below the window top border.
        let rect_top = |k: u32, n: u32| {
            let sx0 = win.x + ((win.w * k / n) as i32);
            let sx1 = win.x + ((win.w * (k + 1) / n) as i32);
            (
                sx0.max(win.x),
                win.y + skip as i32,
                sx1.max(win.x),
                (win.y + skip as i32 + band as i32).min(win.y1()),
            )
        };
        // Right edge: band of columns just left of the right border.
        let rect_right = |k: u32, n: u32| {
            let sy0 = win.y + ((win.h * k / n) as i32);
            let sy1 = win.y + ((win.h * (k + 1) / n) as i32);
            (
                (win.x1() - skip as i32 - band as i32).max(win.x),
                sy0.max(win.y),
                (win.x1() - skip as i32).max(win.x),
                sy1.max(win.y),
            )
        };
        // Bottom edge: band of rows above the bottom border.
        let rect_bottom = |k: u32, n: u32| {
            let sx0 = win.x + ((win.w * k / n) as i32);
            let sx1 = win.x + ((win.w * (k + 1) / n) as i32);
            (
                sx0.max(win.x),
                (win.y1() - skip as i32 - band as i32).max(win.y),
                sx1.max(win.x),
                (win.y1() - skip as i32).max(win.y),
            )
        };
        // Left edge: band of columns just right of the left border.
        let rect_left = |k: u32, n: u32| {
            let sy0 = win.y + ((win.h * k / n) as i32);
            let sy1 = win.y + ((win.h * (k + 1) / n) as i32);
            (
                win.x + skip as i32,
                sy0.max(win.y),
                (win.x + skip as i32 + band as i32).min(win.x1()),
                sy1.max(win.y),
            )
        };

        let nt = self.top_segments.max(1);
        let nr = self.right_segments.max(1);
        let nb = self.bottom_segments.max(1);
        let nl = self.left_segments.max(1);

        for k in 0..nt {
            let (a, b, c, d) = rect_top(k, nt);
            top.push(avg_rect(frame, a, b, c, d, step as i32));
        }
        for k in 0..nr {
            let (a, b, c, d) = rect_right(k, nr);
            right.push(avg_rect(frame, a, b, c, d, step as i32));
        }
        for k in 0..nb {
            // bottom sampled right-to-left so the ring stays clockwise
            let (a, b, c, d) = rect_bottom(nb - 1 - k, nb);
            bottom.push(avg_rect(frame, a, b, c, d, step as i32));
        }
        for k in 0..nl {
            // left sampled bottom-to-top so the ring stays clockwise
            let (a, b, c, d) = rect_left(nl - 1 - k, nl);
            left.push(avg_rect(frame, a, b, c, d, step as i32));
        }

        let top_c = average_vec(&top);
        let right_c = average_vec(&right);
        let bottom_c = average_vec(&bottom);
        let left_c = average_vec(&left);

        let mut ring = Vec::with_capacity((nt + nr + nb + nl) as usize);
        ring.extend(top_ring(&top, nt));
        ring.extend(right_ring(&right, nr));
        ring.extend(bottom_ring(&bottom, nb));
        ring.extend(left_ring(&left, nl));

        let avg = average_vec(&ring);
        Some(Scene { ring, top: top_c, right: right_c, bottom: bottom_c, left: left_c, avg })
    }
}

fn top_ring(v: &[Rgb], _n: u32) -> Vec<Rgb> {
    v.to_vec()
}
fn right_ring(v: &[Rgb], _n: u32) -> Vec<Rgb> {
    v.to_vec()
}
fn bottom_ring(v: &[Rgb], _n: u32) -> Vec<Rgb> {
    v.to_vec()
}
fn left_ring(v: &[Rgb], _n: u32) -> Vec<Rgb> {
    v.to_vec()
}

fn average_vec(v: &[Rgb]) -> Rgb {
    if v.is_empty() {
        return Rgb::zero();
    }
    let mut r = 0.0f32;
    let mut g = 0.0f32;
    let mut b = 0.0f32;
    for c in v {
        r += c.r;
        g += c.g;
        b += c.b;
    }
    let n = v.len() as f32;
    Rgb { r: r / n, g: g / n, b: b / n }
}

/// Average a rect, sampling every `step` pixel horizontally and vertically.
fn avg_rect(frame: &Frame, x0: i32, y0: i32, x1: i32, y1: i32, step: i32) -> Rgb {
    let (x0, y0, x1, y1) = (
        x0.max(0),
        y0.max(0),
        x1.min(frame.width as i32),
        y1.min(frame.height as i32),
    );
    if x1 <= x0 || y1 <= y0 {
        return Rgb::zero();
    }
    let step = step.max(1);
    let mut r = 0u64;
    let mut g = 0u64;
    let mut b = 0u64;
    let mut n = 0u64;
    // In ARGB8888/XRGB8888 the in-memory byte order is B, G, R, (A/x).
    let rp = 2usize;
    let gp = 1usize;
    let bp = 0usize;
    let mut y = y0;
    while y < y1 {
        let row = if frame.y_invert { (frame.height as i32 - 1 - y) as usize } else { y as usize };
        let base = row * frame.stride;
        let mut x = x0;
        while x < x1 {
            let p = base + x as usize * 4;
            if p + 3 < frame.buf.len() {
                b += frame.buf[p + bp] as u64;
                g += frame.buf[p + gp] as u64;
                r += frame.buf[p + rp] as u64;
                n += 1;
            }
            x += step;
        }
        y += step;
    }
    if n == 0 {
        return Rgb::zero();
    }
    Rgb {
        r: (r as f32 / n as f32) / 255.0,
        g: (g as f32 / n as f32) / 255.0,
        b: (b as f32 / n as f32) / 255.0,
    }
}

/// Time-based exponential smoothing + enhancement.
pub struct Mixer {
    pub tau: f64,
    pub brightness: f64,
    pub sat_boost: f64,
    pub min_lum: f64,
    prev: Option<Scene>,
    prev_clock: Option<Instant>,
    first: bool,
}

impl Mixer {
    pub fn new(tau: f64, brightness: f64, sat_boost: f64, min_lum: f64) -> Mixer {
        Mixer { tau, brightness, sat_boost, min_lum, prev: None, prev_clock: None, first: true }
    }

    pub fn feed(&mut self, raw: &Scene, now: Instant) -> Scene {
        let ease = match self.prev_clock {
            Some(t) if self.tau > 0.0 => {
                let dt = now.duration_since(t).as_secs_f64();
                1.0 - (-dt / self.tau).exp()
            }
            _ => 1.0,
        };
        let ease = ease.clamp(0.02, 1.0);
        self.prev_clock = Some(now);

        let enhance_all = |c: &Rgb| enhance(c, self.brightness, self.sat_boost, self.min_lum);

        let ring: Vec<Rgb> = match &self.prev {
            Some(prev) if !self.first && prev.ring.len() == raw.ring.len() => raw
                .ring
                .iter()
                .zip(prev.ring.iter())
                .map(|(r, p)| lerp(p, &enhance_all(r), ease as f32))
                .collect(),
            _ => raw.ring.iter().map(enhance_all).collect(),
        };
        self.first = false;

        // Summaries want a bit more responsiveness than the ring.
        let sum_ease = ease * 1.4;

        let (top, right, bottom, left) = match &self.prev {
            Some(prev) => (
                lerp(&prev.top, &enhance_all(&raw.top), sum_ease as f32),
                lerp(&prev.right, &enhance_all(&raw.right), sum_ease as f32),
                lerp(&prev.bottom, &enhance_all(&raw.bottom), sum_ease as f32),
                lerp(&prev.left, &enhance_all(&raw.left), sum_ease as f32),
            ),
            None => (
                enhance_all(&raw.top),
                enhance_all(&raw.right),
                enhance_all(&raw.bottom),
                enhance_all(&raw.left),
            ),
        };

        let avg = {
            let raw_avg = enhance_all(&raw.avg);
            match self.prev.as_ref() {
                Some(p) => lerp(&p.avg, &raw_avg, sum_ease as f32),
                None => raw_avg,
            }
        };

        let scene = Scene { ring, top, right, bottom, left, avg };
        self.prev = Some(scene.clone());
        scene
    }
}

/// Ease `scene` toward `idle` with `alpha` in [0,1].
pub fn ease_to(scene: &Scene, idle: Rgb, alpha: f32) -> Scene {
    Scene {
        ring: scene.ring.iter().map(|c| lerp(c, &idle, alpha)).collect(),
        top: lerp(&scene.top, &idle, alpha),
        right: lerp(&scene.right, &idle, alpha),
        bottom: lerp(&scene.bottom, &idle, alpha),
        left: lerp(&scene.left, &idle, alpha),
        avg: lerp(&scene.avg, &idle, alpha),
    }
}

/// Component-wise ease from `a` toward `b` with `alpha` in [0,1].
/// Used to interpolate the displayed colors continuously between captures.
pub fn ease_scene(a: &Scene, b: &Scene, alpha: f32) -> Scene {
    let ring = a
        .ring
        .iter()
        .zip(b.ring.iter())
        .map(|(x, y)| lerp(x, y, alpha))
        .collect();
    Scene {
        ring,
        top: lerp(&a.top, &b.top, alpha),
        right: lerp(&a.right, &b.right, alpha),
        bottom: lerp(&a.bottom, &b.bottom, alpha),
        left: lerp(&a.left, &b.left, alpha),
        avg: lerp(&a.avg, &b.avg, alpha),
    }
}