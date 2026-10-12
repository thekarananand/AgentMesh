//! Claude Control Plane: one window per project. Sidebar lists the window's open tabs (one `claude`
//! each) and the project's session history.

mod autoname;
mod glyphs;
mod input;
mod launch;
mod logger;
mod prefs;
mod sessions;
mod sidebar;
mod terminal;
mod theme;
mod watch;
mod window;
mod workspace;

use std::{borrow::Cow, env};

use gpui::{App, KeyBinding, Menu, MenuItem, WindowHandle, actions};
use logger::LogErr;
use workspace::{
    ActivateTab, CloseTab, DecreaseFontSize, IncreaseFontSize, Minimize, NewSession, NextTab,
    OpenProject, PreviousTab, RenameAgent, ResetFontSize, SHORTCUT_TABS, ToggleSidebar, Workspace,
    Zoom,
};

// Embedded in the binary so they render the same on machines without the fonts installed.
const FONT: &[u8] = include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Regular.ttf");
const UI_FONT: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
/// Weight 500: project name, active tab.
const UI_FONT_MEDIUM: &[u8] = include_bytes!("../assets/fonts/Inter-Medium.ttf");
/// Weight 600: the `Current Agents` / `History` section labels.
const UI_FONT_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.otf");

/// Per-session state of the Claude Code that launched this app, if any. Inherited, it makes every
/// inner `claude` think it is a child session (no transcripts), leaks the parent's control-socket
/// token to every tab and its tools, and forces the parent's effort level. Exact names, not the
/// `CLAUDE_CODE_` prefix: user settings like `CLAUDE_CODE_USE_BEDROCK` must pass through.
const SCRUBBED_ENV: [&str; 10] = [
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
];

/// Terminal identity inherited from whatever terminal launched this app (e.g. Ghostty). Left in
/// place, inner `claude` misdetects its host terminal (`/terminal-setup` reports Ghostty), or
/// thinks it runs inside tmux/screen and wraps its escapes for them. Prefixes.
const SCRUBBED_ENV_PREFIXES: [&str; 18] = [
    "TERM_PROGRAM",
    "TERM_SESSION_ID",
    "TERMINFO",
    "LC_TERMINAL",
    "COLORFGBG",
    "GHOSTTY_",
    "ITERM_",
    "KITTY_",
    "WEZTERM_",
    "ALACRITTY_",
    "VSCODE_",
    "KONSOLE_",
    "VTE_VERSION",
    "WT_SESSION",
    "ZED_TERM",
    "TMUX",
    "STY",
    "__CFBundleIdentifier",
];

actions!(
    control_plane,
    [Quit, Hide, HideOthers, ShowAll, ToggleAutoname]
);

fn bind_keys(cx: &mut App) {
    let workspace = Some("Workspace");
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-t", NewSession, workspace),
        // Each window is a project, so a new window starts from the project picker. Bound
        // before Cmd-O so labels show Cmd-O (the last binding wins).
        KeyBinding::new("cmd-n", OpenProject, workspace),
        KeyBinding::new("cmd-o", OpenProject, workspace),
        KeyBinding::new("cmd-w", CloseTab, workspace),
        KeyBinding::new("cmd-shift-[", PreviousTab, workspace),
        KeyBinding::new("cmd-shift-]", NextTab, workspace),
        KeyBinding::new("cmd-m", Minimize, workspace),
        KeyBinding::new("cmd-b", ToggleSidebar, workspace),
        KeyBinding::new("cmd-=", IncreaseFontSize, workspace),
        KeyBinding::new("cmd-+", IncreaseFontSize, workspace),
        KeyBinding::new("cmd--", DecreaseFontSize, workspace),
        KeyBinding::new("cmd-0", ResetFontSize, workspace),
        KeyBinding::new("cmd-a", terminal::SelectAll, Some(terminal::KEY_CONTEXT)),
        KeyBinding::new("cmd-k", terminal::Clear, Some(terminal::KEY_CONTEXT)),
        KeyBinding::new("cmd-c", terminal::Copy, Some(terminal::KEY_CONTEXT)),
        KeyBinding::new("cmd-v", terminal::Paste, Some(terminal::KEY_CONTEXT)),
    ]);
    cx.bind_keys((1..=SHORTCUT_TABS).map(|number| {
        KeyBinding::new(&format!("cmd-{number}"), ActivateTab(number - 1), workspace)
    }));
    cx.on_action(|_: &Quit, cx| quit(cx));
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &ToggleAutoname, cx| {
        prefs::update(cx, |prefs| prefs.autoname = !prefs.autoname);
        // The checkmark is part of the menu; rebuild it.
        let menus = menus(cx);
        cx.set_menus(menus);
    });
    let menus = menus(cx);
    cx.set_menus(menus);
}

fn menus(cx: &mut App) -> Vec<Menu> {
    let autoname = cx.default_global::<prefs::Prefs>().autoname;
    vec![
        Menu::new("Claude Control Plane").items([
            MenuItem::action("Hide Claude Control Plane", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Claude Control Plane", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Session", NewSession),
            MenuItem::action("Open Project\u{2026}", OpenProject),
            MenuItem::separator(),
            MenuItem::action("Close Tab", CloseTab),
        ]),
        Menu::new("Edit").items([
            MenuItem::action("Copy", terminal::Copy),
            MenuItem::action("Paste", terminal::Paste),
            MenuItem::action("Select All", terminal::SelectAll),
            MenuItem::separator(),
            MenuItem::action("Clear Scrollback", terminal::Clear),
        ]),
        Menu::new("View").items([
            MenuItem::action("Toggle Sidebar", ToggleSidebar),
            MenuItem::action("Name Sessions Automatically", ToggleAutoname).checked(autoname),
            MenuItem::separator(),
            MenuItem::action("Bigger", IncreaseFontSize),
            MenuItem::action("Smaller", DecreaseFontSize),
            MenuItem::action("Actual Size", ResetFontSize),
        ]),
        Menu::new("Agent Management").items([MenuItem::action(
            "Rename Current Agent\u{2026}",
            RenameAgent,
        )]),
        // Named "Window": macOS lists the open windows in it.
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
            MenuItem::separator(),
            MenuItem::action("Show Previous Tab", PreviousTab),
            MenuItem::action("Show Next Tab", NextTab),
        ]),
    ]
}

/// Quitting hangs up on every agent: asks first, in one window, if any is mid-turn.
fn quit(cx: &mut App) {
    let windows: Vec<WindowHandle<Workspace>> = cx
        .windows()
        .into_iter()
        .filter_map(|window| window.downcast::<Workspace>())
        .collect();
    let working: Vec<(WindowHandle<Workspace>, usize)> = windows
        .into_iter()
        .filter_map(|handle| {
            let working = handle.read(cx).ok()?.working_count();
            Some((handle, working))
        })
        .filter(|(_, working)| *working > 0)
        .collect();
    let total = working.iter().map(|(_, working)| working).sum();
    match working.first() {
        None => cx.quit(),
        Some((handle, _)) => {
            handle
                .update(cx, |workspace, window, cx| {
                    workspace.confirm_quit(total, window, cx)
                })
                .ok();
        }
    }
}

fn main() {
    logger::init();
    let inherited = env::vars_os().map(|(name, _)| name).filter(|name| {
        name.to_str().is_some_and(|name| {
            SCRUBBED_ENV_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
    });
    for name in SCRUBBED_ENV
        .map(Into::into)
        .into_iter()
        .chain(inherited.collect::<Vec<_>>())
    {
        // SAFETY: single-threaded here; no other thread reads the environment yet.
        unsafe { env::remove_var(&name) };
    }
    alacritty_terminal::tty::setup_env();
    let folder = window::initial_folder();

    gpui_platform::application().run(move |cx: &mut App| {
        cx.text_system()
            .add_fonts(vec![
                Cow::Borrowed(FONT),
                Cow::Borrowed(UI_FONT),
                Cow::Borrowed(UI_FONT_MEDIUM),
                Cow::Borrowed(UI_FONT_SEMIBOLD),
            ])
            .log_err();
        cx.set_global(prefs::load());
        bind_keys(cx);
        // Accessibility: spinner/pulse/fades render static (GPUI checks this per animation).
        cx.set_reduce_motion(theme::accessibility().reduce_motion);
        cx.on_window_closed(|cx, closed| {
            window::OpenProjects::closed(closed, cx);
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        window::open_window(folder, cx);
        cx.activate(true);
    });
}
