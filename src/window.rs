//! Windows: one per project.

use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
};

use gpui::{
    App, Bounds, Focusable, Global, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowHandle, WindowId, WindowOptions, point, prelude::*, px, size,
};

use crate::{sessions, theme, workspace::Workspace};

/// The project for the first window: first argument, else the launch dir, else home.
/// A Finder or Dock launch has cwd `/`, which is never what anyone wants.
pub fn initial_folder() -> PathBuf {
    let folder = env::args()
        .nth(1)
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .or_else(|| env::current_dir().ok().filter(|dir| dir != Path::new("/")))
        .unwrap_or_else(sessions::home_dir);
    canonical(folder)
}

/// The one spelling of a project folder: absolute, symlinks resolved, no trailing slash. Claude
/// Code records the real path, and one-window-per-project compares folders with `==`.
pub fn canonical(folder: PathBuf) -> PathBuf {
    fs::canonicalize(&folder).unwrap_or(folder)
}

/// Open project to its window. Lookups never read a workspace, so they work from inside a
/// workspace update (reading a leased entity panics).
#[derive(Default)]
pub struct OpenProjects(HashMap<PathBuf, WindowHandle<Workspace>>);

impl Global for OpenProjects {}

impl OpenProjects {
    /// An empty window switched projects.
    pub fn retarget(from: &Path, to: PathBuf, cx: &mut App) {
        let projects = &mut cx.default_global::<Self>().0;
        if let Some(handle) = projects.remove(from) {
            projects.insert(to, handle);
        }
    }

    pub fn closed(window: WindowId, cx: &mut App) {
        cx.default_global::<Self>()
            .0
            .retain(|_, handle| handle.window_id() != window);
    }
}

/// Brings the project's window forward if one is open.
pub fn activate_existing(folder: &Path, cx: &mut App) -> bool {
    let Some(handle) = cx.default_global::<OpenProjects>().0.get(folder).copied() else {
        return false;
    };
    let activated = handle
        .update(cx, |_, window, _| window.activate_window())
        .is_ok();
    if !activated {
        // Closed without the close hook running; forget it.
        cx.default_global::<OpenProjects>().0.remove(folder);
    }
    activated
}

/// One window per project: an already-open project is brought forward instead.
pub fn open_window(folder: PathBuf, cx: &mut App) {
    if activate_existing(&folder, cx) {
        return;
    }
    let bounds = Bounds::centered(None, size(px(1100.), px(650.)), cx);
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // Room for the sidebar plus a usable grid (~40 columns, ~10 rows).
            window_min_size: Some(size(px(640.), px(240.))),
            titlebar: Some(TitlebarOptions {
                title: None,
                appears_transparent: true,
                traffic_light_position: Some(point(px(12.), px(10.))),
            }),
            // The sidebar's title bar drags the window itself; the terminal's top rows stay
            // clickable instead of being an AppKit move region.
            app_owns_titlebar_drag: true,
            window_background: if theme::reduce_transparency() {
                WindowBackgroundAppearance::Opaque
            } else {
                WindowBackgroundAppearance::Blurred
            },
            ..Default::default()
        },
        |window, cx| {
            let view = cx.new(|cx| Workspace::new(folder.clone(), window, cx));
            window.focus(&view.focus_handle(cx), cx);
            view
        },
    );
    match opened {
        Ok(handle) => {
            cx.default_global::<OpenProjects>().0.insert(folder, handle);
        }
        Err(error) => {
            log::error!("open window: {error}");
            // `on_window_closed` never fires without a window; don't linger invisibly.
            if cx.windows().is_empty() {
                cx.quit();
            }
        }
    }
}
