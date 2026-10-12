//! One window's state: its project folder, its tabs (one `claude` each) and the project's sessions.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::{Duration, Instant},
};

use futures::{
    StreamExt,
    channel::mpsc::{UnboundedSender, unbounded},
    future,
};
use gpui::{
    AnimationExt, App, ClipboardItem, Context, Entity, FocusHandle, Focusable, KeyDownEvent,
    MouseButton, PathPromptOptions, Pixels, Point, PromptLevel, SharedString, SpringAnimation,
    StyleRefinement, Subscription, Task, Window, actions, div, font, point, prelude::*, px, rgb,
    rgba,
};

use crate::{
    autoname::{self, Given},
    input::{self, Edit},
    launch::{self, Claude},
    prefs::{self, Prefs},
    sessions::{self, Row, Status},
    sidebar::Sidebar,
    terminal::{
        FONT_FAMILY, FONT_SIZE, Launch, Layout, MAX_FONT_SIZE, MIN_FONT_SIZE, PADDING, Terminal,
        TerminalEvent, line_height,
    },
    theme::*,
    watch, window,
};

/// Sessions are re-read this often. Claude Code rewrites its pid files on every status flip.
const POLL: Duration = Duration::from_secs(2);
/// With the pid-file watcher running, status flips arrive as events; the poll only catches
/// what isn't watched (transcript titles, first prompts).
const POLL_WATCHED: Duration = Duration::from_secs(5);
/// Relative times ("3m") go stale even when no session changes.
const CLOCK_REFRESH: Duration = Duration::from_secs(30);
/// Cmd-1..9.
pub const SHORTCUT_TABS: usize = 9;
/// An exit this soon after launch is a failed launch, whatever the exit code.
const LAUNCH_GRACE: Duration = Duration::from_secs(3);

actions!(
    control_plane,
    [
        NewSession,
        OpenProject,
        CloseTab,
        PreviousTab,
        NextTab,
        ToggleSidebar,
        RenameAgent,
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize,
        Minimize,
        Zoom
    ]
);

/// Focus the tab at this index (Cmd-1..9).
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = control_plane, no_json)]
pub struct ActivateTab(pub usize);

pub type TabId = u64;

pub struct Tab {
    pub id: TabId,
    pub view: Entity<Terminal>,
    /// The pty child; because of `exec`, the `claude` process. Binds the tab to its session.
    pub pid: u32,
    /// Session this tab was spawned to resume, so a second click refocuses instead of spawning
    /// again before the session registers. Cleared once the tab is bound.
    resumed: Option<String>,
    /// Title the program set (OSC 0/2); shown until the session's pid file appears.
    title: String,
    started: Instant,
    _subscription: Subscription,
}

/// What a right-click menu acts on.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum MenuTarget {
    Tab(TabId),
    /// A History row, by session id.
    Session(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MenuAction {
    Open,
    Rename,
    CopyId,
    RevealTranscript,
    Close,
}

/// The right-click menu on a sidebar row.
pub(crate) struct ContextMenu {
    pub(crate) target: MenuTarget,
    /// Window position of the click; the menu opens there.
    pub(crate) position: Point<Pixels>,
    pub(crate) focus_handle: FocusHandle,
}

/// A tab's title being edited in place (double-click, right-click › Rename…, or Agent
/// Management › Rename Current Agent). Enter renames, Escape cancels, a click elsewhere saves.
pub(crate) struct Rename {
    pub(crate) tab: TabId,
    pub(crate) name: String,
    pub(crate) focus_handle: FocusHandle,
}

pub struct Workspace {
    /// The project. One window per project; it never changes once tabs are open.
    pub folder: PathBuf,
    /// Spawn order; never re-sorted, so Cmd-1..9 match what the sidebar shows.
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active: Option<TabId>,
    next_id: TabId,
    /// This project's sessions, live first.
    pub(crate) rows: Vec<Row>,
    /// Index into `rows` of the newest session per pid; rebuilt with `rows`.
    by_pid: HashMap<u32, usize>,
    /// Names this app gave its sessions.
    autonamed: HashMap<String, Given>,
    /// None while the login shell is still being asked; Err says why there is no claude.
    pub(crate) claude: Option<Result<Claude, String>>,
    /// Why the last New or resume failed to start.
    pub(crate) launch_error: Option<String>,
    /// Cmd is down: every tab shows its Cmd-N shortcut.
    pub(crate) cmd_held: bool,
    /// Left button went down on the title bar; the next move drags the window.
    pub(crate) title_drag: bool,
    /// The sidebar row under the pointer; drives its spring-animated fill.
    pub(crate) hovered: Option<SharedString>,
    /// Cmd-B. Hidden: its title bar moves above the terminal (traffic lights, drag, switcher).
    pub(crate) sidebar_open: bool,
    /// Dragged at the sidebar's right edge; remembered in `Prefs`.
    sidebar_width: Pixels,
    /// The sidebar edge is being dragged.
    resizing_sidebar: bool,
    /// The History accordion is expanded. Starts closed in every new window (not remembered).
    pub(crate) history_open: bool,
    /// Ended sessions to list; "Show more" raises it.
    pub(crate) history_limit: usize,
    /// Right-click menu, while open.
    pub(crate) menu: Option<ContextMenu>,
    /// Inline rename in progress.
    pub(crate) renaming: Option<Rename>,
    /// Wakes the poll loop now instead of after `POLL`.
    refresh: UnboundedSender<()>,
    /// Terminal font size (Cmd-+ / Cmd-- / Cmd-0), for every tab in the window.
    font_size: Pixels,
    cell_width: Pixels,
    focus_handle: FocusHandle,
    window_title: String,
    /// Grid size last pushed to the tabs; they are refit only when it changes.
    layout: Option<Layout>,
    /// Embedded `.cached`, so terminal output and redraws elsewhere don't rebuild every row.
    sidebar: Option<Entity<Sidebar>>,
    /// Tabs waiting on the user at the last poll; a tab entering this set may ask for attention.
    waiting: HashSet<TabId>,
    /// Bounce the Dock icon on the next render (polls have no window).
    attention: bool,
    /// A close or quit prompt is up; a second sheet would stack on it.
    prompting: bool,
    /// The user agreed to close a window with working agents; the next close goes through.
    close_confirmed: bool,
    last_refresh: Instant,
    /// The window is key. Off: the chrome steps back and the terminal dims (macOS convention).
    pub(crate) window_active: bool,
    /// Wakes the poll on pid-file changes; None if FSEvents couldn't start (polling only).
    _watcher: Option<watch::Watcher>,
    _resolve: Task<()>,
    _poll: Task<()>,
    _activation: Subscription,
}

impl Workspace {
    pub fn new(folder: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prefs = *cx.default_global::<Prefs>();
        let font_size = px(prefs.font_size).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        let cell_width = cell_width(font_size, window);

        let (refresh, mut refresh_requests) = unbounded();
        let watcher = watch::Watcher::new(&sessions::sessions_dir(), refresh.clone());
        let interval = if watcher.is_some() {
            POLL_WATCHED
        } else {
            POLL
        };
        let resolve = Self::resolve_task(cx);
        let poll = cx.spawn(async move |this, cx| {
            let mut cache = sessions::Cache::default();
            loop {
                // Read each time: an empty window can be retargeted to another project.
                let Ok((folder, limit)) =
                    this.read_with(cx, |this, _| (this.folder.clone(), this.history_limit))
                else {
                    break;
                };
                let (rows, returned) = cx
                    .background_executor()
                    .spawn({
                        let folder = folder.clone();
                        async move { (sessions::list(&folder, limit, &mut cache), cache) }
                    })
                    .await;
                cache = returned;
                let current = this.update(cx, |this, cx| {
                    // Retargeted while reading: these rows are another project's.
                    if this.folder == folder {
                        this.set_rows(rows, cx);
                    }
                });
                if current.is_err() {
                    break;
                }
                // Sleep until the next poll, or until something asks for fresh rows.
                let timer = cx.background_executor().timer(interval);
                future::select(timer, refresh_requests.next()).await;
                // One read for a burst of wake-ups (a status flip touches several files).
                while refresh_requests.try_recv().is_ok() {}
            }
        });

        // Tabs are cached views: tell each one, so its cursor turns hollow/solid.
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            this.window_active = window.is_window_active();
            // No modifier event reaches a window that isn't key: Cmd released after Cmd-Tab
            // would leave the shortcuts showing.
            if !this.window_active {
                this.cmd_held = false;
            }
            for tab in &this.tabs {
                tab.view.update(cx, |_, cx| cx.notify());
            }
            cx.notify();
        });

        // Closing the window hangs up on every tab; ask first if any agent is mid-turn.
        let this = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            this.update(cx, |this, cx| this.should_close(window, cx))
                .unwrap_or(true)
        });

        Self {
            folder,
            tabs: Vec::new(),
            active: None,
            next_id: 0,
            rows: Vec::new(),
            by_pid: HashMap::new(),
            autonamed: HashMap::new(),
            claude: None,
            launch_error: None,
            cmd_held: false,
            title_drag: false,
            hovered: None,
            history_open: false,
            history_limit: sessions::RECENT_LIMIT,
            menu: None,
            renaming: None,
            sidebar_open: prefs.sidebar_open,
            sidebar_width: px(prefs.sidebar_width).clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH),
            resizing_sidebar: false,
            refresh,
            font_size,
            cell_width,
            focus_handle: cx.focus_handle(),
            window_title: String::new(),
            layout: None,
            sidebar: None,
            waiting: HashSet::new(),
            attention: false,
            prompting: false,
            close_confirmed: false,
            last_refresh: Instant::now(),
            window_active: window.is_window_active(),
            _watcher: watcher,
            _resolve: resolve,
            _poll: poll,
            _activation: activation,
        }
    }

    /// Asks the login shell for claude. Separate from the poll: History shows while a slow
    /// login shell is still starting.
    fn resolve_task(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let claude = cx
                .background_executor()
                .spawn(async { launch::resolve() })
                .await;
            if let Err(error) = &claude {
                log::warn!("resolve claude: {error}");
            }
            this.update(cx, |this, cx| {
                this.claude = Some(claude);
                cx.notify();
            })
            .ok();
        })
    }

    /// Looks for claude again, e.g. after the user installed it. No relaunch needed.
    pub(crate) fn retry_resolve(&mut self, cx: &mut Context<Self>) {
        if self.claude.is_none() {
            return;
        }
        self.claude = None;
        self._resolve = Self::resolve_task(cx);
        cx.notify();
    }

    pub(crate) fn set_history_open(&mut self, open: bool) {
        if self.history_open != open {
            self.history_open = open;
            self.refresh_rows();
        }
    }

    /// Ended sessions, newest first, so today's come before earlier ones as the sidebar
    /// groups them.
    pub(crate) fn history_rows(&self) -> Vec<&Row> {
        self.rows.iter().filter(|row| !row.live()).collect()
    }

    /// The active tab's terminal, else the window itself.
    fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active {
            Some(id) => self.focus_tab(id, window, cx),
            None => window.focus(&self.focus_handle, cx),
        }
    }

    pub(crate) fn show_more_history(&mut self, cx: &mut Context<Self>) {
        self.history_limit += sessions::RECENT_LIMIT;
        self.refresh_rows();
        cx.notify();
    }

    pub(crate) fn open_menu(
        &mut self,
        target: MenuTarget,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        self.menu = Some(ContextMenu {
            target,
            position,
            focus_handle,
        });
        cx.notify();
    }

    pub(crate) fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// The session a menu target refers to, if it has one yet.
    pub(crate) fn menu_row(&self, target: &MenuTarget) -> Option<&Row> {
        match target {
            MenuTarget::Tab(id) => self
                .tabs
                .iter()
                .find(|tab| tab.id == *id)
                .and_then(|tab| self.tab_row(tab)),
            MenuTarget::Session(id) => self.rows.iter().find(|row| &row.id == id),
        }
    }

    /// The items a target's menu offers, in order.
    pub(crate) fn menu_actions(&self, target: &MenuTarget) -> Vec<MenuAction> {
        let row = self.menu_row(target);
        let mut actions = Vec::new();
        if matches!(target, MenuTarget::Session(_)) {
            actions.push(MenuAction::Open);
        }
        // Only a live session takes a rename (over its control socket).
        if row.is_some_and(|row| row.socket.is_some()) {
            actions.push(MenuAction::Rename);
        }
        if row.is_some() {
            actions.push(MenuAction::CopyId);
        }
        if row.is_some_and(|row| row.transcript.is_some()) {
            actions.push(MenuAction::RevealTranscript);
        }
        if matches!(target, MenuTarget::Tab(_)) {
            actions.push(MenuAction::Close);
        }
        actions
    }

    pub(crate) fn run_menu_action(
        &mut self,
        action: MenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.menu.as_ref().map(|menu| menu.target.clone()) else {
            return;
        };
        let row = self.menu_row(&target).cloned();
        self.close_menu(window, cx);
        match (action, &target, row) {
            (MenuAction::Open, _, Some(row)) => self.open_session(&row, window, cx),
            (MenuAction::Rename, MenuTarget::Tab(id), _) => self.start_rename(*id, window, cx),
            (MenuAction::CopyId, _, Some(row)) => {
                cx.write_to_clipboard(ClipboardItem::new_string(row.id))
            }
            (MenuAction::RevealTranscript, _, Some(row)) => {
                if let Some(path) = &row.transcript {
                    cx.reveal_path(path);
                }
            }
            (MenuAction::Close, MenuTarget::Tab(id), _) => self.request_close_tab(*id, window, cx),
            _ => {}
        }
    }

    /// Keys in the menu: Escape closes it.
    pub(crate) fn menu_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        if event.keystroke.key == "escape" {
            self.close_menu(window, cx);
        }
    }

    /// The selected tab hosts a running session, which is what a rename needs.
    fn can_rename_active(&self) -> bool {
        self.active
            .and_then(|id| self.tabs.iter().find(|tab| tab.id == id))
            .and_then(|tab| self.tab_row(tab))
            .is_some_and(|row| row.socket.is_some())
    }

    /// Turns a tab's title into a name field. Only a running session can be renamed (over its
    /// control socket); a tab still starting has nothing to rename yet.
    pub(crate) fn start_rename(&mut self, tab: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self
            .tabs
            .iter()
            .find(|candidate| candidate.id == tab)
            .and_then(|tab| self.tab_row(tab))
        else {
            return;
        };
        if row.socket.is_none() {
            return;
        }
        let name = row.live_name.clone().unwrap_or_else(|| row.title.clone());
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        self.renaming = Some(Rename {
            tab,
            name,
            focus_handle,
        });
        cx.notify();
    }

    pub(crate) fn rename_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Typing goes to the name, never to the terminal or the workspace shortcuts' keys.
        cx.stop_propagation();
        let Some(rename) = self.renaming.as_mut() else {
            return;
        };
        match input::apply(&mut rename.name, &event.keystroke) {
            Edit::Submit { .. } => self.finish_rename(true, window, cx),
            Edit::Cancel => self.finish_rename(false, window, cx),
            Edit::Changed => cx.notify(),
            _ => {}
        }
    }

    /// Ends the inline rename; `save` sends the name (if non-empty and changed). Focus goes back
    /// to the active tab.
    pub(crate) fn finish_rename(
        &mut self,
        save: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rename) = self.renaming.take() else {
            return;
        };
        let name = rename.name;
        let row = self
            .tabs
            .iter()
            .find(|tab| tab.id == rename.tab)
            .and_then(|tab| self.tab_row(tab))
            .cloned();
        if save && let Some((socket, id, name)) = row.and_then(|row| rename_request(&row, &name)) {
            // The user's choice: autoname leaves a name it didn't send alone.
            cx.background_executor()
                .spawn(async move { sessions::rename(&socket, &id, &name) })
                .detach_and_log_err(cx);
            self.refresh_rows();
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Re-reads sessions now, e.g. when History is toggled, so it isn't up to `POLL` stale.
    pub(crate) fn refresh_rows(&self) {
        self.refresh.unbounded_send(()).ok();
    }

    fn set_rows(&mut self, rows: Vec<Row>, cx: &mut Context<Self>) {
        if rows == self.rows && self.last_refresh.elapsed() < CLOCK_REFRESH {
            return;
        }
        let mut by_pid: HashMap<u32, usize> = HashMap::new();
        for (index, row) in rows.iter().enumerate() {
            let Some(pid) = row.pid else {
                continue;
            };
            // A pid can briefly show two sessions (old file not yet removed): newest wins.
            match by_pid.get(&pid) {
                Some(&known) if rows[known].updated >= row.updated => {}
                _ => {
                    by_pid.insert(pid, index);
                }
            }
        }
        self.by_pid = by_pid;
        self.rows = rows;
        for index in 0..self.tabs.len() {
            if self.tab_row(&self.tabs[index]).is_some() {
                // The session may have moved on (`/clear`, in-tab `/resume`); the pid is the truth.
                self.tabs[index].resumed = None;
            }
        }
        self.autoname(cx);
        self.note_waiting();
        self.last_refresh = Instant::now();
        cx.notify();
    }

    /// A tab that just started waiting where the user can't see it (another tab is shown, or
    /// another app is key) bounces the Dock icon once, on the next render.
    fn note_waiting(&mut self) {
        let waiting: HashSet<TabId> = self
            .tabs
            .iter()
            .filter(|tab| {
                self.tab_row(tab)
                    .is_some_and(|row| row.status == Some(Status::Waiting))
            })
            .map(|tab| tab.id)
            .collect();
        let unseen = |id: &TabId| !self.window_active || self.active != Some(*id);
        if waiting
            .iter()
            .any(|id| !self.waiting.contains(id) && unseen(id))
        {
            self.attention = true;
        }
        self.waiting = waiting;
    }

    /// Sends each tab's session the name `autoname::next_name` picks. Recorded before sending so
    /// the next poll doesn't send it twice; forgotten on failure so a later poll retries.
    fn autoname(&mut self, cx: &mut Context<Self>) {
        if !cx.default_global::<Prefs>().autoname {
            return;
        }
        let mut renames = Vec::new();
        for tab in &self.tabs {
            let Some(row) = self.tab_row(tab) else {
                continue;
            };
            let given = self.autonamed.get(&row.id);
            if row.background || given.is_some_and(|given| given.ai) {
                continue;
            }
            let (Some(socket), Some(next)) = (&row.socket, autoname::next_name(row, given)) else {
                continue;
            };
            renames.push((socket.clone(), row.id.clone(), next));
        }
        for (socket, id, next) in renames {
            self.autonamed.insert(id.clone(), next.clone());
            let rename = cx.background_executor().spawn({
                let (id, name) = (id.clone(), next.name.clone());
                async move { sessions::rename(&socket, &id, &name) }
            });
            cx.spawn(async move |this, cx| {
                if let Err(error) = rename.await {
                    log::warn!("autoname {id}: {error}");
                    this.update(cx, |this, _| {
                        if this.autonamed.get(&id) == Some(&next) {
                            this.autonamed.remove(&id);
                        }
                    })
                    .ok();
                }
            })
            .detach();
        }
    }

    /// Sets the sidebar width from a drag, within bounds, leaving the terminal its minimum.
    fn resize_sidebar(&mut self, x: Pixels, window: &Window, cx: &mut Context<Self>) {
        let max = (window.viewport_size().width - MIN_TERMINAL_WIDTH).min(MAX_SIDEBAR_WIDTH);
        let width = x
            .clamp(MIN_SIDEBAR_WIDTH, max.max(MIN_SIDEBAR_WIDTH))
            .round();
        if width != self.sidebar_width {
            self.sidebar_width = width;
            cx.notify();
        }
    }

    fn save_sidebar_width(&self, cx: &mut Context<Self>) {
        let width = f32::from(self.sidebar_width);
        prefs::update(cx, |prefs| prefs.sidebar_width = width);
    }

    /// The grab strip on the sidebar's right edge. Drag to resize; double-click resets.
    fn render_sidebar_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("sidebar-resize")
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(5.))
            .occlude()
            .cursor_col_resize()
            .when(self.resizing_sidebar, |handle| handle.bg(rgba(HAIRLINE)))
            .hover(|handle| handle.bg(rgba(HAIRLINE)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if event.click_count >= 2 {
                        this.resizing_sidebar = false;
                        this.sidebar_width = SIDEBAR_WIDTH;
                        this.save_sidebar_width(cx);
                    } else {
                        this.resizing_sidebar = true;
                    }
                    cx.notify();
                }),
            )
    }

    /// Estimate for every tab, hidden ones included; the visible tab refines it from its
    /// painted bounds (`Terminal::fit`).
    fn layout(&self, window: &Window) -> Layout {
        let viewport = window.viewport_size();
        // Sidebar shown: the grid runs to the top edge. Hidden: under the moved title bar.
        let origin = if self.sidebar_open {
            point(self.sidebar_width, px(0.))
        } else {
            point(px(0.), TITLE_BAR)
        };
        let line_height = line_height(self.font_size);
        let columns = (viewport.width - origin.x - PADDING * 2.) / self.cell_width;
        let rows = (viewport.height - origin.y - PADDING * 2.) / line_height;
        Layout {
            columns: columns.max(2.) as usize,
            rows: rows.max(1.) as usize,
            font_size: self.font_size,
            cell_width: self.cell_width,
            line_height,
            origin,
        }
    }

    fn set_font_size(&mut self, font_size: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        let font_size = font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        if font_size == self.font_size {
            return;
        }
        self.font_size = font_size;
        self.cell_width = cell_width(font_size, window);
        prefs::update(cx, |prefs| prefs.font_size = f32::from(font_size));
        // Render refits every tab once the layout differs.
        cx.notify();
    }

    fn spawn_tab(
        &mut self,
        args: Vec<String>,
        resumed: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Ok(claude)) = self.claude.clone() else {
            return;
        };
        let launch = Launch {
            claude,
            args,
            cwd: self.folder.clone(),
        };
        let layout = self.layout(window);
        let spawned = match Terminal::spawn(&launch, layout) {
            Ok(spawned) => spawned,
            Err(error) => {
                log::error!("launch: {error}");
                self.launch_error = Some(error);
                cx.notify();
                return;
            }
        };
        self.launch_error = None;
        let view = cx.new(|cx| Terminal::new(spawned, layout, cx));
        let pid = view.read(cx).pid;
        let id = self.next_id;
        self.next_id += 1;
        // Quitting claude closes its tab; a tab never lingers at a bare shell prompt.
        let subscription =
            cx.subscribe_in(
                &view,
                window,
                move |this, _, event, window, cx| match event {
                    TerminalEvent::Exited { code, last_line } => {
                        this.tab_exited(id, *code, last_line.as_deref(), window, cx)
                    }
                    TerminalEvent::Title(title) => {
                        if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == id) {
                            tab.title = title.clone();
                            cx.notify();
                        }
                    }
                },
            );
        self.tabs.push(Tab {
            id,
            view,
            pid,
            resumed,
            title: String::new(),
            started: Instant::now(),
            _subscription: subscription,
        });
        self.focus_tab(id, window, cx);
    }

    pub(crate) fn new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.claude, Some(Err(_))) {
            return self.retry_resolve(cx);
        }
        self.spawn_tab(Vec::new(), None, window, cx);
    }

    pub(crate) fn focus_tab(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) {
            self.active = Some(id);
            window.focus(&tab.view.focus_handle(cx), cx);
            cx.notify();
        }
    }

    /// Cmd-Shift-[ / ]: the neighbouring tab in sidebar order, wrapping around.
    fn cycle_tab(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(current) = self
            .active
            .and_then(|id| self.tabs.iter().position(|tab| tab.id == id))
        else {
            return;
        };
        let index = (current as isize + step).rem_euclid(self.tabs.len() as isize) as usize;
        self.activate_index(index, window, cx);
    }

    fn activate_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.tabs.get(index).map(|tab| tab.id) {
            self.focus_tab(id, window, cx);
        }
    }

    /// A failed launch (`--resume` rejected, crash, shell error) would just vanish; say why.
    fn tab_exited(
        &mut self,
        id: TabId,
        code: Option<i32>,
        last_line: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let quick = self
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .is_some_and(|tab| tab.started.elapsed() < LAUNCH_GRACE);
        if quick || code != Some(0) {
            let how = code.map_or("stopped by a signal".into(), |code| format!("code {code}"));
            self.launch_error = Some(match last_line {
                Some(line) => format!("claude exited ({how}): {line}"),
                None => format!("claude exited ({how})."),
            });
        }
        self.close_tab(id, window, cx);
    }

    /// Mid-turn: closing would stop work in progress.
    fn working(&self, tab: &Tab) -> bool {
        self.tab_row(tab)
            .is_some_and(|row| matches!(row.status, Some(Status::Busy | Status::Waiting)))
    }

    pub(crate) fn working_count(&self) -> usize {
        self.tabs.iter().filter(|tab| self.working(tab)).count()
    }

    /// Close button and Cmd-W: a working agent is closed only after a confirmation.
    pub(crate) fn request_close_tab(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        if !self.working(tab) {
            return self.close_tab(id, window, cx);
        }
        let message = format!("Close \u{201c}{}\u{201d}?", self.tab_title(tab));
        self.confirm(
            &message,
            "The agent is still working. Closing the tab stops it.",
            "Close",
            window,
            cx,
            move |this, window, cx| this.close_tab(id, window, cx),
        );
    }

    /// The window's close button. Returns whether it may close now.
    fn should_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let working = self.working_count();
        if self.close_confirmed || working == 0 {
            return true;
        }
        let message = match working {
            1 => "Close the window with 1 agent working?".to_string(),
            _ => format!("Close the window with {working} agents working?"),
        };
        self.confirm(
            &message,
            "Closing the window stops every agent in it.",
            "Close",
            window,
            cx,
            |this, window, _| {
                this.close_confirmed = true;
                window.remove_window();
            },
        );
        false
    }

    /// Quit with working agents anywhere: asked once, in this window.
    pub(crate) fn confirm_quit(
        &mut self,
        working: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message = match working {
            1 => "Quit with 1 agent working?".to_string(),
            _ => format!("Quit with {working} agents working?"),
        };
        window.activate_window();
        self.confirm(
            &message,
            "Quitting stops every agent.",
            "Quit",
            window,
            cx,
            |_, _, cx| cx.quit(),
        );
    }

    /// A warning sheet with a destructive button and Cancel; `then` runs on the destructive one.
    fn confirm(
        &mut self,
        message: &str,
        detail: &str,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if self.prompting {
            return;
        }
        self.prompting = true;
        let answer = window.prompt(
            PromptLevel::Warning,
            message,
            Some(detail),
            &[action, "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let confirmed = answer.await == Ok(0);
            this.update_in(cx, |this, window, cx| {
                this.prompting = false;
                if confirmed {
                    then(this, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn close_tab(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        // Dropping the view stops the PTY; alacritty's `Pty` drop sends SIGHUP and reaps claude.
        self.tabs.remove(index);
        if self
            .renaming
            .as_ref()
            .is_some_and(|rename| rename.tab == id)
        {
            self.renaming = None;
        }
        if self.active == Some(id) {
            self.active = None;
            match self.tabs.get(index.min(self.tabs.len().saturating_sub(1))) {
                Some(next) => self.focus_tab(next.id, window, cx),
                None => window.focus(&self.focus_handle, cx),
            }
        }
        cx.notify();
    }

    /// History click: focus the tab that has it, else resume it. Sessions live in another
    /// terminal are inert; resuming them would put two writers on one transcript.
    pub(crate) fn open_session(&mut self, row: &Row, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self
            .tabs
            .iter()
            .find(|tab| {
                self.tab_row(tab).is_some_and(|bound| bound.id == row.id)
                    || tab.resumed.as_ref() == Some(&row.id)
            })
            .map(|tab| tab.id);
        if let Some(tab) = tab {
            return self.focus_tab(tab, window, cx);
        }
        // The polled rows can be 2s old; it may have been opened elsewhere since. A non-UUID id
        // could be read as a flag by `--resume`.
        if !sessions::is_session_id(&row.id) || sessions::is_live(&row.id) {
            return;
        }
        let mut args = vec!["--resume".into(), row.id.clone()];
        // Only a deliberate name; otherwise claude mints a fresh autoname.
        if let Some(name) = &row.name {
            args.extend(["--name".into(), name.clone()]);
        }
        self.spawn_tab(args, Some(row.id.clone()), window, cx);
    }

    /// The project's own window comes forward if open; else an empty window is retargeted;
    /// else the project opens in a new window.
    pub(crate) fn open_project(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Project".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = paths.await else {
                return;
            };
            let Some(folder) = paths.pop().map(window::canonical) else {
                return;
            };
            if cx.update(|cx| window::activate_existing(&folder, cx)) {
                return;
            }
            let retargeted = this.update(cx, |this, cx| {
                if !this.tabs.is_empty() {
                    return false;
                }
                window::OpenProjects::retarget(&this.folder, folder.clone(), cx);
                this.folder = folder.clone();
                this.rows.clear();
                this.by_pid.clear();
                this.autonamed.clear();
                // Rows for the old folder may be in flight; `set_rows` drops them. Don't wait
                // a full poll for the new project's.
                this.refresh_rows();
                cx.notify();
                true
            });
            if matches!(retargeted, Ok(false)) {
                cx.update(|cx| window::open_window(folder, cx));
            }
        })
        .detach();
    }

    /// The session a tab hosts, once its pid file has appeared.
    pub(crate) fn tab_row(&self, tab: &Tab) -> Option<&Row> {
        self.by_pid.get(&tab.pid).map(|&index| &self.rows[index])
    }

    /// From state kept here, never the tab's view: reading a hidden terminal during render
    /// would subscribe the window to its output.
    pub(crate) fn tab_title(&self, tab: &Tab) -> String {
        match self.tab_row(tab) {
            Some(row) => row.title.clone(),
            None if tab.title.is_empty() => "Starting\u{2026}".into(),
            None => tab.title.clone(),
        }
    }

    pub(crate) fn waiting_count(&self) -> usize {
        self.tabs
            .iter()
            .filter(|tab| {
                self.tab_row(tab)
                    .is_some_and(|row| row.status == Some(Status::Waiting))
            })
            .count()
    }

    /// The OS window title (Mission Control, Window menu); there is no title drawn over the
    /// terminal. Set only when it changes: animations re-render every frame.
    fn update_window_title(&mut self, window: &mut Window) {
        let project = crate::sidebar::folder_name(&self.folder);
        let session = self
            .active
            .and_then(|id| self.tabs.iter().find(|tab| tab.id == id))
            .map(|tab| self.tab_title(tab));
        let title = match session {
            Some(session) => format!("{project} \u{2014} {session}"),
            None => project,
        };
        // Window menu, Mission Control and the Dock's window list show who needs the user.
        let title = match self.waiting.len() {
            0 => title,
            waiting => format!("({waiting}) {title}"),
        };
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title;
        }
    }
}

/// What to send for an edited name: (control socket, session id, name). Nothing for an empty or
/// unchanged name, or a session without a socket (not running).
fn rename_request(row: &Row, name: &str) -> Option<(PathBuf, String, String)> {
    let name = name.trim();
    let socket = row.socket.clone()?;
    (!name.is_empty() && row.live_name.as_deref() != Some(name))
        .then(|| (socket, row.id.clone(), name.to_string()))
}

/// Advance of one monospace cell at `font_size`.
fn cell_width(font_size: Pixels, window: &Window) -> Pixels {
    let font_id = window.text_system().resolve_font(&font(FONT_FAMILY));
    window
        .text_system()
        .advance(font_id, font_size, 'm')
        .map(|size| size.width)
        .unwrap_or(font_size * 0.6)
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Every tab is refit, not just the visible one, so hidden sessions keep the right width.
        // Only on change: touching a hidden tab here would redraw the window on its output.
        let layout = self.layout(window);
        if self.layout != Some(layout) {
            self.layout = Some(layout);
            for tab in &self.tabs {
                // Cached views: a font change alone wouldn't re-render them.
                tab.view.update(cx, |view, cx| {
                    view.set_layout(layout);
                    cx.notify();
                });
            }
        }
        let sidebar = self
            .sidebar
            .get_or_insert_with(|| {
                let workspace = cx.entity();
                cx.new(|cx| Sidebar::new(&workspace, cx))
            })
            .clone();

        let main = match self
            .active
            .and_then(|id| self.tabs.iter().find(|tab| tab.id == id))
        {
            // Cached: the sidebar's animations repaint the window every frame; the grid is
            // rebuilt only when the terminal itself notifies.
            Some(tab) => div().flex_1().min_h_0().child(
                tab.view
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            ),
            None => self.render_welcome(window, cx),
        };

        let root = div()
            // Drag tracking on the root: it spans the window, so the drag keeps going wherever
            // the pointer is (element listeners only fire while hovered).
            .on_mouse_move(
                cx.listener(|this, event: &gpui::MouseMoveEvent, window, cx| {
                    if this.resizing_sidebar {
                        this.resize_sidebar(event.position.x, window, cx);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if std::mem::take(&mut this.resizing_sidebar) {
                        this.save_sidebar_width(cx);
                        cx.notify();
                    }
                }),
            )
            .when(self.resizing_sidebar, |root| root.cursor_col_resize())
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &NewSession, window, cx| this.new_session(window, cx)))
            .on_action(cx.listener(|this, _: &OpenProject, _, cx| this.open_project(cx)))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                match this.active {
                    Some(id) => this.request_close_tab(id, window, cx),
                    // macOS: Cmd-W in a window with nothing to close closes the window.
                    None if this.tabs.is_empty() => window.remove_window(),
                    None => {}
                }
            }))
            .on_modifiers_changed(cx.listener(
                |this, event: &gpui::ModifiersChangedEvent, _, cx| {
                    let held = event.modifiers.platform;
                    if held != this.cmd_held {
                        this.cmd_held = held;
                        cx.notify();
                    }
                },
            ));
        let root = root
            .on_action(cx.listener(|this, action: &ActivateTab, window, cx| {
                this.activate_index(action.0, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &PreviousTab, window, cx| this.cycle_tab(-1, window, cx)),
            )
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.cycle_tab(1, window, cx)))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.sidebar_open = !this.sidebar_open;
                let open = this.sidebar_open;
                prefs::update(cx, |prefs| prefs.sidebar_open = open);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &IncreaseFontSize, window, cx| {
                this.set_font_size(this.font_size + px(1.), window, cx)
            }))
            .on_action(cx.listener(|this, _: &DecreaseFontSize, window, cx| {
                this.set_font_size(this.font_size - px(1.), window, cx)
            }))
            .on_action(cx.listener(|this, _: &ResetFontSize, window, cx| {
                this.set_font_size(FONT_SIZE, window, cx)
            }))
            // Registered only when it would work: macOS greys out a menu item whose action
            // has no handler, so Rename Current Agent is disabled with no agent selected (or
            // one still starting).
            .when(self.can_rename_active(), |root| {
                root.on_action(cx.listener(|this, _: &RenameAgent, window, cx| {
                    if let Some(id) = this.active {
                        this.start_rename(id, window, cx);
                    }
                }))
            })
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &Zoom, window, _| window.zoom_window());

        self.update_window_title(window);
        if std::mem::take(&mut self.attention) {
            window.request_attention();
        }
        let menu = self.render_menu(cx);
        root.size_full()
            .flex()
            .font_family(UI_FONT_FAMILY)
            .text_color(rgb(BODY))
            .when(self.sidebar_open, |root| {
                root.child(
                    div()
                        .w(self.sidebar_width)
                        .flex_none()
                        .h_full()
                        .relative()
                        .child(sidebar.cached(StyleRefinement::default().size_full()))
                        .child(self.render_sidebar_handle(cx)),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(rgba(glass()))
                    .text_size(self.font_size)
                    .line_height(line_height(self.font_size))
                    .relative()
                    .when(!self.sidebar_open, |column| {
                        column.child(
                            div()
                                .text_size(px(13.))
                                .child(self.render_title_bar(window, cx)),
                        )
                    })
                    .child(main)
                    // Inactive scrim: the terminal recedes while another app is key. A plain
                    // fill with no listeners, so clicks still reach the grid (and focus it).
                    .child(div().absolute().inset_0().with_spring(
                        "inactive-scrim",
                        SpringAnimation::new(spring(0.3, 1.)).to(!self.window_active),
                        |scrim, phase| {
                            let shown = phase.clamp(0.0..=1.).0;
                            scrim.bg(gpui::Hsla::from(rgb(INACTIVE_SCRIM))
                                .opacity(shown * INACTIVE_SCRIM_ALPHA))
                        },
                    )),
            )
            .children(menu)
    }
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use super::*;

    #[test]
    fn rename_sends_only_real_changes() {
        let row = Row {
            id: "s".into(),
            pid: Some(1),
            status: Some(Status::Idle),
            background: false,
            cwd: PathBuf::new(),
            branch: None,
            name: None,
            live_name: Some("old-name".into()),
            ai_title: None,
            first_prompt: None,
            socket: Some("/tmp/s.sock".into()),
            transcript: None,
            title: "old-name".into(),
            updated: UNIX_EPOCH,
        };
        assert_eq!(
            rename_request(&row, "  new name "),
            Some(("/tmp/s.sock".into(), "s".into(), "new name".into()))
        );
        assert_eq!(rename_request(&row, "old-name"), None);
        assert_eq!(rename_request(&row, "   "), None);
        let ended = Row {
            socket: None,
            ..row
        };
        assert_eq!(rename_request(&ended, "new"), None);
    }
}
