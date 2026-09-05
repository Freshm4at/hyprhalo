//! Software rendering of the dynamic wallpaper and the window glow into the
//! shm canvases of the layer-shell surfaces. Integer/lightweight math so that
//! ~10 fps stays nearly free next to a heavy game.

use crate::colors::{lerp, Rgb, Scene, WinRectPx};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Rect {
    pub fn bounding(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
        match (a, b) {
            (Some(a), Some(b)) => Some(Rect {
                x0: a.x0.min(b.x0),
                y0: a.y0.min(b.y0),
                x1: a.x1.max(b.x1),
                y1: a.y1.max(b.y1),
            }),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }
}

/// Mutable pixel row in an ARGB8888 buffer.
fn row_mut(canvas: &mut [u8], w: usize, y: i32) -> &mut [u8] {
    if w == 0 {
        return &mut canvas[..0];
    }
    let rows = canvas.len() / (w * 4);
    let y = (y.max(0) as usize).min(rows.saturating_sub(1));
    let start = y * w * 4;
    let end = ((y + 1) * w * 4).min(canvas.len());
    &mut canvas[start..end]
}

// ---------- Wallpaper ----------

/// Fill `canvas` (ARGB8888, width `w` x height `h`) with the synced gradient.
pub fn render_wallpaper(
    canvas: &mut [u8],
    w: usize,
    h: usize,
    scene: &Scene,
    direction_blend: f64,
    vignette: f64,
    ambient_blend: f64,
) {
    if w == 0 || h == 0 {
        return;
    }
    let (t, r, b, l, avg) = (scene.top, scene.right, scene.bottom, scene.left, scene.avg);
    let hw = (255.0 * (1.0 - direction_blend.clamp(0.0, 1.0))) as u32;
    let bw = 255 - hw;
    let vig_par = vignette.clamp(0.0, 1.0);

    // Per-column horizontal color + horizontal vignette, precomputed once.
    let mut hcol = vec![(0u8, 0u8, 0u8, 0u16); w];
    let last = (w - 1).max(1) as f32;
    for (x, cell) in hcol.iter_mut().enumerate() {
        let u = x as f32 / last;
        let edge_dist = ((2.0 * u - 1.0).abs()) as f64;
        let vig = 255.0 * (1.0 - vig_par * edge_dist * edge_dist);
        let col = lerp(&l, &r, u);
        *cell = ((col.r * 255.0) as u8, (col.g * 255.0) as u8, (col.b * 255.0) as u8, vig as u16);
    }

    let vmax = (h - 1).max(1) as f32;
    let amb = ambient_blend.clamp(0.0, 1.0) as f32;
    let row_bytes = w * 4;

    for y in 0..h {
        let t_y = y as f32 / vmax;
        let mut base = lerp(&t, &b, t_y);
        base = lerp(&base, &avg, amb);
        let edge_dist = ((2.0 * t_y - 1.0).abs()) as f64;
        let vigy = (255.0 * (1.0 - vig_par * edge_dist * edge_dist)) as u32;

        let br = (base.r * 255.0) as u32;
        let bg = (base.g * 255.0) as u32;
        let bb = (base.b * 255.0) as u32;

        let row = &mut canvas[y * row_bytes..(y + 1) * row_bytes];
        for (x, px) in row.chunks_exact_mut(4).enumerate() {
            let (hr, hg, hb, vigx) = hcol[x];
            let r = (br * bw + hr as u32 * hw) >> 8;
            let g = (bg * bw + hg as u32 * hw) >> 8;
            let b = (bb * bw + hb as u32 * hw) >> 8;
            let vig = ((vigx as u32 * vigy) >> 8) + 1;
            px[0] = ((b * vig) >> 8) as u8; // B
            px[1] = ((g * vig) >> 8) as u8; // G
            px[2] = ((r * vig) >> 8) as u8; // R
            px[3] = 0xFF;                   // A
        }
    }
}

// ---------- Glow ----------

/// Boxes around the window occupied by the halo, per side.
/// `radius` fades outward (away from the window). The glow surface is an
/// overlay ON TOP of the target window, so boxes must never extend into the
/// window interior (that would paint over the game) — the inner fade stops
/// exactly at the window edge.
pub fn glow_boxes(win: &WinRectPx, radius: i32, _inner: i32) -> [Rect; 4] {
    [
        // above the window, from radius up to its top edge
        Rect { x0: win.x, y0: win.y - radius, x1: win.x + win.w as i32, y1: win.y },
        // right of the window, from its right edge out to radius
        Rect { x0: win.x + win.w as i32, y0: win.y, x1: win.x + win.w as i32 + radius, y1: win.y + win.h as i32 },
        // below the window, from its bottom edge down to radius
        Rect { x0: win.x, y0: win.y + win.h as i32, x1: win.x + win.w as i32, y1: win.y + win.h as i32 + radius },
        // left of the window, from radius out to its left edge
        Rect { x0: win.x - radius, y0: win.y, x1: win.x, y1: win.y + win.h as i32 },
    ]
}

pub fn union_of_boxes(boxes: &[Rect; 4]) -> Option<Rect> {
    boxes.iter().fold(None, |acc, b| Rect::bounding(acc, Some(*b)))
}

/// Zero a rectangle in the canvas.
pub fn clear_rect(canvas: &mut [u8], w: usize, r: &Rect) {
    let x0 = r.x0.max(0) as usize;
    let y0 = r.y0.max(0) as usize;
    let x1 = r.x1.max(0) as usize;
    let y1 = r.y1.max(0) as usize;
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    for y in y0..y1 {
        let row_slice = row_mut(canvas, w, y as i32);
        let start = (x0 * 4).min(row_slice.len());
        let end = (x1 * 4).min(row_slice.len());
        if end > start {
            row_slice[start..end].fill(0);
        }
    }
}

/// Smooth 1D color ring around the perimeter, interpolating segment colors so
/// there are no hard seams at segment boundaries.
struct RingSampler<'a> {
    top: &'a [Rgb],
    right: &'a [Rgb],
    bottom: &'a [Rgb],
    left: &'a [Rgb],
    w: f64,
    h: f64,
}

impl<'a> RingSampler<'a> {
    /// Color at perimeter position `p` (px, clockwise from top-left corner).
    fn at(&self, p: f64) -> Rgb {
        let total = 2.0 * self.w + 2.0 * self.h;
        let p = ((p % total) + total) % total;
        if p < self.w {
            seg_color(self.top, p / self.w * self.top.len() as f64)
        } else if p < self.w + self.h {
            seg_color(self.right, (p - self.w) / self.h * self.right.len() as f64)
        } else if p < 2.0 * self.w + self.h {
            seg_color(self.bottom, (p - self.w - self.h) / self.w * self.bottom.len() as f64)
        } else {
            seg_color(self.left, (p - 2.0 * self.w - self.h) / self.h * self.left.len() as f64)
        }
    }
}

fn seg_color(colors: &[Rgb], pos: f64) -> Rgb {
    if colors.is_empty() {
        return Rgb::zero();
    }
    let n = colors.len() as f64;
    let pos = pos.clamp(0.0, n - 1e-9);
    let i = pos.floor() as usize;
    let f = (pos - i as f64) as f32;
    let a = colors[i];
    let b = colors[(i + 1) % colors.len()];
    // Smoothstep between the discrete ring samples: a cubic ease so the color
    // gradient has no visible knots/bands at segment boundaries.
    let s = f * f * (3.0 - 2.0 * f);
    lerp(&a, &b, s)
}

/// Write a glow pixel (premultiplied, max-composited).
#[inline]
fn put_glow(row: &mut [u8], x: i32, c: Rgb, alpha: f64) {
    if alpha <= 0.0 || alpha >= 0.98 {
        return;
    }
    let a = (alpha * 255.0) as u8;
    if a < 6 {
        return;
    }
    let idx = x as usize * 4;
    if idx + 3 >= row.len() {
        return;
    }
    // premultiplied: light with alpha over whatever is below, max-composite
    if row[idx + 3] >= a {
        return;
    }
    let inv = alpha as f32;
    let pr = (c.r * inv * 255.0) as u8;
    let pg = (c.g * inv * 255.0) as u8;
    let pb = (c.b * inv * 255.0) as u8;
    row[idx] = pb;
    row[idx + 1] = pg;
    row[idx + 2] = pr;
    row[idx + 3] = a;
}

/// Draw a continuous rounded halo around `win`, split by the per-edge segment
/// counts (`order: top -> right -> bottom -> left`, bottom/left already
/// reversed to keep the ring clockwise).
///
/// Unlike a per-side boxed glow this renders the region *outside* the window
/// as a single smooth falloff `1 - (d/radius)^2` where `d` is the Euclidean
/// distance to the window edge, so the four sides and the corners blend into
/// one continuous ring (no separate "circles"/bands).
///
/// `prev` is the union box left over from the previous frame; it is cleared
/// first so reused buffers stay clean. Returns the newly drawn union box.
#[allow(clippy::too_many_arguments)]
pub fn render_glow(
    canvas: &mut [u8],
    w: usize,
    h: usize,
    win: &WinRectPx,
    scene: &Scene,
    nt: usize,
    nr: usize,
    nb: usize,
    radius: i32,
    inner: i32,
    intensity: f64,
    prev: Option<Rect>,
) -> Option<Rect> {
    let top = &scene.ring[0..nt];
    let right = &scene.ring[nt..nt + nr];
    let bottom = &scene.ring[nt + nr..nt + nr + nb];
    let left = &scene.ring[nt + nr + nb..];

    if let Some(p) = prev {
        clear_rect(canvas, w, &p);
    }

    let ring = RingSampler { top, right, bottom, left, w: win.w as f64, h: win.h as f64 };
    let boxes = glow_boxes(win, radius, inner);
    let intensity = intensity.clamp(0.0, 1.0);
    let r = radius.max(1) as f64;
    let r2 = r * r;
    let _ = inner; // the halo stays outside the window (clamped at its edge)

    let x0 = win.x;
    let y0 = win.y;
    let x1 = win.x + win.w as i32;
    let y1 = win.y + win.h as i32;
    let x0f = x0 as f64;
    let y0f = y0 as f64;
    let x1f = x1 as f64;
    let y1f = y1 as f64;
    let (fw, fh) = (win.w as f64, win.h as f64);

    let xmin = (x0 - radius).max(0);
    let xmax = (x1 + radius).min(w as i32);
    let ymin = (y0 - radius).max(0);
    let ymax = (y1 + radius).min(h as i32);
    if xmax <= xmin || ymax <= ymin {
        return union_of_boxes(&boxes);
    }

    // Per-column squared outer distance (cheap; removes sqrt and per-pixel math).
    let cols: Vec<f64> = (xmin..xmax)
        .map(|x| {
            let dx = if x < x0 { (x0 - x) as f64 } else if x >= x1 { (x - x1) as f64 } else { 0.0 };
            dx * dx
        })
        .collect();

    let row_bytes = w * 4;
    for y in ymin..ymax {
        let dy = if y < y0 { (y0 - y) as f64 } else if y >= y1 { (y - y1) as f64 } else { 0.0 };
        let dy2 = dy * dy;
        let row = &mut canvas[y as usize * row_bytes..(y + 1) as usize * row_bytes];
        let yc = (y as f64).clamp(y0f, y1f);
        for (i, &dx2) in cols.iter().enumerate() {
            let x = xmin + i as i32;
            if dx2 == 0.0 && dy2 == 0.0 {
                continue; // inside the window: nothing to draw
            }
            let dd = dx2 + dy2;
            if dd >= r2 {
                continue;
            }
            let xc = (x as f64).clamp(x0f, x1f);
            // Map the pixel onto the window perimeter to pick the ring color,
            // choosing the nearest edge so corners blend continuously.
            let p = if dx2 > dy2 {
                if x < x0 {
                    2.0 * fw + fh + (y1f - yc) // left
                } else {
                    fw + (yc - y0f) // right
                }
            } else if y < y0 {
                xc - x0f // top
            } else {
                fw + fh + (x1f - xc) // bottom
            };
            let alpha = (1.0 - dd / r2) * intensity;
            put_glow(row, x, ring.at(p), alpha);
        }
    }

    union_of_boxes(&boxes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> Scene {
        let c = Rgb::from_bytes(255, 0, 0);
        let ring = vec![c; 8];
        Scene { ring, top: c, right: c, bottom: c, left: c, avg: c }
    }

    fn alpha_at(c: &[u8], w: usize, x: usize, y: usize) -> u8 {
        c[y * w * 4 + x * 4 + 3]
    }

    /// The halo must be one continuous blob: a pixel in the corner region
    /// (diagonal, distance < radius from the window corner) has to be lit, and
    /// a pixel beyond the radius must stay clear.
    #[test]
    fn glow_fills_corners_continuously() {
        let w = 60usize;
        let h = 40usize;
        let mut c = vec![0u8; w * h * 4];
        let win = WinRectPx { x: 20, y: 12, w: 20, h: 16 };
        let s = scene();
        render_glow(&mut c, w, h, &win, &s, 2, 2, 2, 8, 2, 0.8, None);

        // Diagonal corner pixel: 4px out horizontally and vertically.
        let corner_x = (win.x + win.w as i32 + 4) as usize;
        let corner_y = (win.y - 4) as usize;
        assert!(alpha_at(&c, w, corner_x, corner_y) > 0, "corner pixel must be lit");

        // Far away from the window: nothing drawn.
        assert_eq!(alpha_at(&c, w, 2, 2), 0, "far pixel must stay clear");
        // Just outside the radius: nothing drawn.
        let far_x = (win.x + win.w as i32 + 9) as usize;
        assert_eq!(alpha_at(&c, w, far_x, (win.y + win.h as i32 / 2) as usize), 0);
    }
}