use serde_json::Value;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

/// Minimal Hyprland IPC client. Talks directly to the Hyprland unix sockets,
/// no subprocess spawn, no extra dependencies.
#[derive(Debug, Clone)]
pub struct Hypr {
    request_sock: PathBuf,
}

#[allow(dead_code)]
pub struct Monitor {
    pub name: String,
    pub id: i32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f64,
}

#[allow(dead_code)]
pub struct WinGeom {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Monitor name hosting the window.
    pub monitor: String,
    pub class: String,
    pub title: String,
    pub mapped: bool,
    pub minimized: bool,
}

impl Hypr {
    pub fn connect() -> Option<Hypr> {
        let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
        let base = std::env::var("XDG_RUNTIME_DIR")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/run/user/1000"));
        let dir = base.join("hypr").join(sig);
        if !dir.join(".socket.sock").exists() {
            return None;
        }
        Some(Hypr { request_sock: dir.join(".socket.sock") })
    }

    /// Send a raw hyprctl command. Returns the raw text reply.
    fn request(&self, cmd: &str) -> Option<String> {
        let mut sock = UnixStream::connect(&self.request_sock).ok()?;
        sock.set_write_timeout(Some(Duration::from_millis(300))).ok()?;
        sock.set_read_timeout(Some(Duration::from_millis(800))).ok()?;
        sock.write_all(cmd.as_bytes()).ok()?;
        sock.shutdown(Shutdown::Write).ok()?;
        let mut buf = String::new();
        sock.read_to_string(&mut buf).ok()?;
        Some(buf)
    }

    /// Send a request returning JSON.
    fn request_json(&self, cmd: &str) -> Option<Value> {
        let raw = self.request(&format!("j/{cmd}"))?;
        serde_json::from_str(raw.trim()).ok()
    }

    pub fn monitors(&self) -> Vec<Monitor> {
        let Some(v) = self.request_json("monitors") else { return Vec::new() };
        let Some(arr) = v.as_array() else { return Vec::new() };
        arr.iter()
            .filter_map(|m| {
                Some(Monitor {
                    name: m.get("name")?.as_str()?.to_string(),
                    id: m.get("id").and_then(|v| v.as_i64()).unwrap_or(-1) as i32,
                    x: m.get("x")?.as_i64()? as i32,
                    y: m.get("y")?.as_i64()? as i32,
                    width: m.get("width")?.as_i64()? as i32,
                    height: m.get("height")?.as_i64()? as i32,
                    scale: m.get("scale")?.as_f64().unwrap_or(1.0),
                })
            })
            .collect()
    }

    /// List of all windows (from `clients`), used when a specific class is targeted.
    pub fn clients(&self) -> Vec<WinGeom> {
        let Some(v) = self.request_json("clients") else { return Vec::new() };
        let Some(arr) = v.as_array() else { return Vec::new() };
        arr.iter().filter_map(parse_client).collect()
    }

    /// The currently focused window (from `activewindow`).
    pub fn active_window(&self) -> Option<WinGeom> {
        let v = self.request_json("activewindow")?;
        if v.is_null() {
            return None;
        }
        parse_client(&v)
    }

    /// Fill `geom` from a JSON object using either modern (at/size) or legacy (x,y,w,h) keys.
    fn get_geom(obj: &Value) -> Option<(i32, i32, i32, i32)> {
        if let (Some(at), Some(size)) = (obj.get("at"), obj.get("size")) {
            if let (Some(a), Some(s)) = (at.as_array(), size.as_array()) {
                if let (Some(x), Some(y)) = (a.first()?.as_i64(), a.get(1)?.as_i64()) {
                    if let (Some(w), Some(h)) = (s.first()?.as_i64(), s.get(1)?.as_i64()) {
                        return Some((x as i32, y as i32, w as i32, h as i32));
                    }
                }
            }
        }
        let x = obj.get("x")?.as_i64()? as i32;
        let y = obj.get("y")?.as_i64()? as i32;
        let w = obj.get("w")?.as_i64()? as i32;
        let h = obj.get("h")?.as_i64()? as i32;
        Some((x, y, w, h))
    }

    /// Dispatch a `hyprctl dispatch ...` command. Returns whether it was accepted.
    pub fn dispatch(&self, cmd: &str) -> bool {
        let reply = self.request(&format!("dispatch {cmd}"));
        matches!(reply.as_deref(), Some("ok"))
    }
}

fn parse_client(o: &Value) -> Option<WinGeom> {
    let (x, y, w, h) = Hypr::get_geom(o)?;
    // mwHyprland returns the monitor as either a name (string) or an ID (integer).
    let monitor = match o.get("monitor") {
        Some(m) => m.as_str().map(str::to_string).or_else(|| m.as_i64().map(|i| i.to_string())).unwrap_or_default(),
        None => String::new(),
    };
    let class = o
        .get("class")
        .and_then(|c| c.as_str())
        .map(str::to_string)
        .unwrap_or_default();
    let title = o
        .get("title")
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .unwrap_or_default();
    let mapped = o.get("mapped").and_then(|m| m.as_bool()).unwrap_or(true);
    // Hyprland exposes the icon/float etc, use legacy minimized heuristic:
    let minimized = !mapped;
    Some(WinGeom { x, y, w, h, monitor, class, title, mapped, minimized })
}