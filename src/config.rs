use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const DEFAULT_CONFIG: &str = include_str!("default.toml");

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    /// Capture framerate (frames per second).
    #[serde(default = "default_capture_fps")]
    pub capture_fps: f64,
    /// How often (ms) we re-check the target window geometry.
    #[serde(default = "default_geometry_poll_ms")]
    pub geometry_poll_ms: u64,
    /// If set, only windows with this Hyprland class are targeted.
    /// Empty string = follow the focused window.
    #[serde(default)]
    pub window_class: String,
    /// Minimum window size (logical px) to consider a target.
    #[serde(default = "default_min_window_size")]
    pub min_window_size: [u32; 2],
    /// Smoothing factor (0.0 = instant, 1.0 = very slow). Applied exponentially per capture.
    #[serde(default = "default_smoothing")]
    pub smoothing: f64,
    /// Exponential easing time constant (s) for the continuous display
    /// transition between captures. Smaller = snappier, larger = smoother.
    #[serde(default = "default_display_tau")]
    pub display_tau_s: f64,
    /// Global brightness multiplier applied to every derived color.
    #[serde(default = "default_brightness")]
    pub brightness: f64,
    /// Saturation booster (1.0 = unchanged).
    #[serde(default = "default_saturation")]
    pub saturation_boost: f64,
    /// Floor for the luminance of derived colors (avoids pitch-black glows).
    #[serde(default = "default_min_luminance")]
    pub min_luminance: f64,
    /// Color to drift toward when no target window is present.
    #[serde(default, with = "color_serde")]
    pub idle_color: [u8; 3],
    /// Seconds without a target window before drifting to the idle color.
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_s: f64,

    #[serde(default)]
    pub wallpaper: WallpaperConfig,
    #[serde(default)]
    pub glow: GlowConfig,
    #[serde(default)]
    pub border: BorderConfig,
    #[serde(default)]
    pub capture: CaptureConfig,
    /// Named smoothing profiles; `--profile <name>` selects one of these.
    #[serde(default)]
    pub profiles: Profiles,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WallpaperConfig {
    /// Draw the dynamic wallpaper background.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Blend between the vertical (top/bottom) and horizontal (left/right) gradients.
    /// 1.0 = purely vertical top->bottom, 0.0 = purely horizontal.
    #[serde(default = "default_direction_blend")]
    pub direction_blend: f64,
    /// Vignette strength (0.0 = none, 1.0 = strong).
    #[serde(default = "default_vignette")]
    pub vignette: f64,
    /// How much the on-screen average color mixes into the wallpaper.
    #[serde(default = "default_ambient")]
    pub ambient_blend: f64,
    /// Internal downscale factor for the wallpaper buffer (1 = full res, 2 = half).
    /// Larger values are much cheaper on CPU; the compositor upscales (viewporter).
    #[serde(default = "default_scale_down")]
    pub scale_down: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GlowConfig {
    /// Draw the halo around the window.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Radius of the outer fade (logical px).
    #[serde(default = "default_radius")]
    pub radius_px: u32,
    /// Depth of the soft fade drawn just inside the window edges (logical px).
    #[serde(default = "default_inner")]
    pub inner_px: u32,
    /// Opacity/intensity of the halo (0.0..1.0).
    #[serde(default = "default_intensity")]
    pub intensity: f64,
    /// Segments per edge (more = smoother but more costly).
    #[serde(default = "default_top_segments")]
    pub top_segments: u32,
    #[serde(default = "default_bottom_segments")]
    pub bottom_segments: u32,
    #[serde(default = "default_left_segments")]
    pub left_segments: u32,
    #[serde(default = "default_right_segments")]
    pub right_segments: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BorderConfig {
    /// Sync the Hyprland window border color (hyprctl setprop) to the edge colors.
    #[serde(default = "default_true")]
    pub sync: bool,
    /// Hyprland border size to request while a target window is active (0 = leave untouched).
    #[serde(default)]
    pub border_size: u32,
    /// Only send a new border color when the color changed by at least this amount (0..1).
    #[serde(default = "default_min_delta")]
    pub min_delta: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CaptureConfig {
    /// Thickness of the band sampled just inside the window edges (logical px).
    #[serde(default = "default_edge_band")]
    pub edge_band_px: u32,
    /// Skip this many logical px inside the window border before sampling
    /// (avoids sampling the Hyprland border/decorations themselves).
    #[serde(default = "default_border_skip")]
    pub border_skip_px: u32,
    /// Sampling stride inside the band (higher = cheaper, sparser).
    #[serde(default = "default_sample_stride")]
    pub sample_stride: u32,
    /// Recreate the screencopy frame after this many failed/completed cycles attempts.
    #[serde(default = "default_max_failures")]
    pub max_failures: u32,
}

impl Default for Config {
    fn default() -> Self {
        toml::from_str(DEFAULT_CONFIG).expect("default config must parse")
    }
}

macro_rules! section_default {
    ($t:ty, $field:ident) => {
        impl Default for $t {
            fn default() -> Self {
                let cfg: Config = toml::from_str(DEFAULT_CONFIG).expect("default config must parse");
                cfg.$field
            }
        }
    };
}

section_default!(WallpaperConfig, wallpaper);
section_default!(GlowConfig, glow);
section_default!(BorderConfig, border);
section_default!(CaptureConfig, capture);

// ---------------------------------------------------------------------------
// Scalar defaults (mirror `default.toml`).
// ---------------------------------------------------------------------------

fn default_true() -> bool {
    true
}
fn default_capture_fps() -> f64 {
    5.0
}
fn default_geometry_poll_ms() -> u64 {
    250
}
fn default_min_window_size() -> [u32; 2] {
    [480, 320]
}
fn default_smoothing() -> f64 {
    0.35
}
fn default_display_tau() -> f64 {
    0.25
}
fn default_brightness() -> f64 {
    1.0
}
fn default_saturation() -> f64 {
    1.15
}
fn default_min_luminance() -> f64 {
    0.06
}
fn default_idle_timeout() -> f64 {
    3.0
}
fn default_direction_blend() -> f64 {
    0.8
}
fn default_vignette() -> f64 {
    0.35
}
fn default_ambient() -> f64 {
    0.3
}
fn default_scale_down() -> u32 {
    2
}
fn default_radius() -> u32 {
    130
}
fn default_inner() -> u32 {
    14
}
fn default_intensity() -> f64 {
    0.55
}
fn default_top_segments() -> u32 {
    18
}
fn default_bottom_segments() -> u32 {
    10
}
fn default_left_segments() -> u32 {
    12
}
fn default_right_segments() -> u32 {
    12
}
fn default_min_delta() -> f64 {
    0.02
}
fn default_edge_band() -> u32 {
    28
}
fn default_border_skip() -> u32 {
    4
}
fn default_sample_stride() -> u32 {
    2
}
fn default_max_failures() -> u32 {
    5
}

pub fn default_config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".config"));
    base.join("hyprhalo/config.toml")
}

/// A named smoothing profile, as defined in the `[profiles]` config section.
/// `--profile <name>` applies its values over the top-level config.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProfileDef {
    #[serde(default = "default_capture_fps")]
    pub capture_fps: f64,
    #[serde(default = "default_smoothing")]
    pub smoothing: f64,
    #[serde(default = "default_display_tau")]
    pub display_tau_s: f64,
}

impl Default for ProfileDef {
    fn default() -> Self {
        ProfileDef {
            capture_fps: default_capture_fps(),
            smoothing: default_smoothing(),
            display_tau_s: default_display_tau(),
        }
    }
}

/// The three built-in profiles. Their values are editable in the config file
/// (default.toml / ~/.config/hyprhalo/config.toml).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Profiles {
    #[serde(default)]
    pub low: ProfileDef,
    #[serde(default)]
    pub normal: ProfileDef,
    #[serde(default, rename = "very-smooth")]
    pub very_smooth: ProfileDef,
}

impl Default for Profiles {
    fn default() -> Self {
        Profiles {
            low: ProfileDef { capture_fps: 4.0, smoothing: 0.15, display_tau_s: 0.10 },
            normal: ProfileDef::default(),
            very_smooth: ProfileDef {
                capture_fps: 8.0,
                smoothing: 0.60,
                display_tau_s: 0.60,
            },
        }
    }
}

impl Config {
    fn profile(&mut self, name: &str) -> Result<&mut ProfileDef, String> {
        let key = name.to_ascii_lowercase();
        match key.as_str() {
            "low" => Ok(&mut self.profiles.low),
            "normal" => Ok(&mut self.profiles.normal),
            "very-smooth" | "very_smooth" | "smooth" => Ok(&mut self.profiles.very_smooth),
            _ => Err(format!(
                "unknown profile {name:?} (use one defined in [profiles]: low | normal | very-smooth)"
            )),
        }
    }

    /// Apply a named profile's values over the top-level smoothing settings.
    pub fn apply_profile(&mut self, name: &str) -> Result<(), String> {
        let def = self.profile(name)?.clone();
        self.capture_fps = def.capture_fps;
        self.smoothing = def.smoothing;
        self.display_tau_s = def.display_tau_s;
        Ok(())
    }
}

/// Smoothing profiles selectable via `--profile` on the command line. They
/// override the capture rate, per-sample color smoothing and the continuous
/// display easing time constant. `Config` file values are used when no
/// profile is given.
/// Serde module that (de)serializes `[u8;3]` as a hex string like "112233".
mod color_serde {
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(c: &[u8; 3], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2]))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 3], D::Error> {
        let hex = String::deserialize(d)?;
        let hex = hex.trim_start_matches('#');
        if hex.len() != 6 {
            return Err(serde::de::Error::custom("color must be 6 hex digits"));
        }
        let v = u32::from_str_radix(hex, 16).map_err(serde::de::Error::custom)?;
        Ok([(v >> 16) as u8, (v >> 8) as u8, v as u8])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_parses_without_recursion() {
        let cfg: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        let d = Config::default();
        assert_eq!(cfg.idle_color, d.idle_color);
        assert_eq!(cfg.glow.top_segments, 18);
        assert_eq!(cfg.wallpaper.scale_down, 2);
    }
}