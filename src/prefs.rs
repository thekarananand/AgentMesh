//! View choices that survive a relaunch, in
//! `~/Library/Application Support/ClaudeControlPlane/prefs.json`. This app's own file: nothing is
//! written under `~/.claude`. Missing or unreadable means defaults.

use std::{fs, path::PathBuf};

use gpui::{App, Global};
use serde_json::{Value, json};

use crate::{logger::LogErr, sessions};

/// Shared by every window (a `Global`); new windows start from it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prefs {
    pub sidebar_open: bool,
    /// Sidebar width in pixels (dragged at its right edge).
    pub sidebar_width: f32,
    /// Terminal font size in pixels.
    pub font_size: f32,
    /// Name tab sessions from their first prompt / AI title (`autoname`).
    pub autoname: bool,
}

impl Global for Prefs {}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            sidebar_open: true,
            sidebar_width: f32::from(crate::theme::SIDEBAR_WIDTH),
            font_size: f32::from(crate::terminal::FONT_SIZE),
            autoname: true,
        }
    }
}

/// Changes the shared prefs and saves them off the UI thread.
pub fn update(cx: &mut App, change: impl FnOnce(&mut Prefs)) {
    let prefs = cx.default_global::<Prefs>();
    change(prefs);
    let prefs = *prefs;
    cx.background_executor()
        .spawn(async move { save(prefs).log_err() })
        .detach();
}

fn path() -> PathBuf {
    sessions::home_dir().join("Library/Application Support/ClaudeControlPlane/prefs.json")
}

pub fn load() -> Prefs {
    fs::read(path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .map(|json| parse(&json))
        .unwrap_or_default()
}

fn parse(json: &Value) -> Prefs {
    let defaults = Prefs::default();
    Prefs {
        sidebar_open: json["sidebarOpen"]
            .as_bool()
            .unwrap_or(defaults.sidebar_open),
        sidebar_width: json["sidebarWidth"]
            .as_f64()
            .map_or(defaults.sidebar_width, |width| width as f32),
        font_size: json["fontSize"]
            .as_f64()
            .map_or(defaults.font_size, |size| size as f32),
        autoname: json["autoname"].as_bool().unwrap_or(defaults.autoname),
    }
}

/// Written whole; last window to change something wins.
fn save(prefs: Prefs) -> std::io::Result<()> {
    let path = path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = json!({
        "sidebarOpen": prefs.sidebar_open,
        "sidebarWidth": prefs.sidebar_width,
        "fontSize": prefs.font_size,
        "autoname": prefs.autoname,
    });
    fs::write(path, format!("{json:#}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_keys_fall_back_to_defaults() {
        assert_eq!(parse(&json!({})), Prefs::default());
        // A key from an older version (`historyOpen`) is ignored.
        let prefs = parse(&json!({ "historyOpen": true, "fontSize": 16 }));
        assert!(prefs.sidebar_open);
        assert_eq!(prefs.sidebar_width, Prefs::default().sidebar_width);
        assert_eq!(parse(&json!({ "sidebarWidth": 320 })).sidebar_width, 320.);
        assert_eq!(prefs.font_size, 16.);
    }
}
