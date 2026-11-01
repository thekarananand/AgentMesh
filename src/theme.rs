//! Colors and sizes for the chrome outside the terminal. One neutral gray ladder (Linear-style),
//! one accent, amber and green only as status colors.

use gpui::{Pixels, px};

use crate::terminal::ANSI;

/// UI text outside the terminal (sidebar, title bar, welcome). Icons stay in the Nerd Font.
pub const UI_FONT_FAMILY: &str = "Inter";

pub const TITLE_BAR: Pixels = px(36.);
/// Room left of the title-bar content for the traffic lights (Zed uses 71-78).
pub const TRAFFIC_LIGHT_PADDING: Pixels = px(76.);
/// Default sidebar width; dragging its right edge changes it within these bounds.
pub const SIDEBAR_WIDTH: Pixels = px(280.);
pub const MIN_SIDEBAR_WIDTH: Pixels = px(200.);
pub const MAX_SIDEBAR_WIDTH: Pixels = px(480.);
/// Room the terminal always keeps when the sidebar is dragged wider.
pub const MIN_TERMINAL_WIDTH: Pixels = px(320.);

/// Window fill, translucent over the blurred desktop.
pub const GLASS: u32 = 0x080909dd;
/// Sidebar material: Linear's charcoal surface (`#161718`) at ~70%, a lighter, thinner glass
/// than the terminal's ~87%, so more of the blurred desktop shows through. Painted instead of
/// `GLASS`, not over it.
pub const SIDEBAR: u32 = 0x161718b3;

/// Active title, project name.
pub const INK: u32 = 0xf7f8f8;
/// Row titles.
pub const BODY: u32 = 0xd0d6e0;
/// Subtitles, times, hints.
pub const SUBTLE: u32 = 0x8a8f98;
/// Section labels, history titles at rest. 4.5:1 on the sidebar's `#161718` (WCAG AA); was
/// `#62666d` (3.1:1). The glass is translucent, so a bright desktop lowers it somewhat.
pub const TERTIARY: u32 = 0x7c8088;
/// Sidebar edge, title-bar rule.
pub const HAIRLINE: u32 = 0xffffff14;
pub const HOVER: u32 = 0xffffff0a;
pub const ACTIVE: u32 = 0xffffff1c;
/// The selected row while the window is key (macOS: accent selection in a key window, gray
/// `ACTIVE` otherwise).
pub const ACCENT_SELECTION: u32 = 0x74ade84d;
pub const TOOLTIP_BG: u32 = 0x1c1d20f2;

pub const ACCENT: u32 = 0x74ade8;
pub const AMBER: u32 = ANSI[3];
pub const AMBER_PILL: u32 = 0xd18f5226;
pub const GREEN: u32 = ANSI[2];
pub const PRESSED: u32 = 0xffffff26;

/// Apple's spring parameters: `response` is roughly the settle time in seconds, `damping_ratio`
/// 1.0 means no overshoot.
pub fn spring(response: f32, damping_ratio: f32) -> gpui::SpringConfig {
    let omega = std::f32::consts::TAU / response;
    gpui::SpringConfig::new(omega * omega, 2. * damping_ratio * omega, 1.)
}

/// Row fills and hover swaps: quick, critically damped, interruptible mid-flight.
pub fn hover_spring() -> gpui::SpringConfig {
    spring(0.18, 1.)
}

/// The macOS accessibility switches this app follows. Read once, at launch, with one `defaults`
/// call; changing them in System Settings takes effect on the next launch.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Accessibility {
    /// Spinner, pulse and fades render static (GPUI checks `cx.reduce_motion()`).
    pub reduce_motion: bool,
    /// Solid window and sidebar, no blur.
    pub reduce_transparency: bool,
}

static ACCESSIBILITY: std::sync::OnceLock<Accessibility> = std::sync::OnceLock::new();

pub fn accessibility() -> Accessibility {
    *ACCESSIBILITY.get_or_init(|| {
        std::process::Command::new("defaults")
            .args(["read", "com.apple.universalaccess"])
            .output()
            .map(|out| parse_accessibility(&String::from_utf8_lossy(&out.stdout)))
            .unwrap_or_default()
    })
}

/// `defaults read` prints the domain as `{ reduceMotion = 1; ... }`; unset keys are absent.
fn parse_accessibility(domain: &str) -> Accessibility {
    let on = |key: &str| {
        domain
            .lines()
            .any(|line| line.trim() == format!("{key} = 1;"))
    };
    Accessibility {
        reduce_motion: on("reduceMotion"),
        reduce_transparency: on("reduceTransparency"),
    }
}

pub fn reduce_transparency() -> bool {
    accessibility().reduce_transparency
}

/// Solid when transparency is reduced.
fn material(color: u32) -> u32 {
    if reduce_transparency() {
        color | 0xff
    } else {
        color
    }
}

/// The terminal side's fill.
pub fn glass() -> u32 {
    material(GLASS)
}

/// The sidebar's fill.
pub fn sidebar() -> u32 {
    material(SIDEBAR)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessibility_reads_the_defaults_domain() {
        let domain =
            "{\n    reduceMotion = 1;\n    reduceTransparency = 0;\n    mouseDriver = 0;\n}\n";
        assert_eq!(
            parse_accessibility(domain),
            Accessibility {
                reduce_motion: true,
                reduce_transparency: false,
            }
        );
        assert_eq!(parse_accessibility("{\n}\n"), Accessibility::default());
    }
}
