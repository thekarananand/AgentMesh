//! One PTY running one `claude` process, rendered as a GPUI grid.

use std::{borrow::Cow, path::PathBuf, sync::Arc};

use alacritty_terminal::{
    Term,
    event::{Event, EventListener, Notify, OnResize, WindowSize},
    event_loop::{EventLoop, Msg, Notifier},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point as GridPoint, Side},
    selection::{Selection, SelectionType},
    sync::FairMutex,
    term::{Config, TermMode, cell::Flags, color::Colors, test::TermSize},
    tty::{self, Shell},
    vte::ansi::{Color, CursorShape, NamedColor, Rgb},
};
use futures::{
    StreamExt,
    channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded},
};
use gpui::{
    App, BorderStyle, Bounds, ClipboardEntry, ClipboardItem, Context, EventEmitter, ExternalPaths,
    FocusHandle, Focusable, Font, FontStyle, FontWeight, Hsla, KeyDownEvent, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent,
    StyledText, Task, TextRun, Window, actions, canvas, div, font, outline, point, prelude::*, px,
    rgb, rgba, size,
};

use crate::{glyphs, launch::Claude};

pub const FONT_FAMILY: &str = "JetBrainsMono Nerd Font Mono";
pub const FONT_SIZE: Pixels = px(14.);
pub const LINE_HEIGHT: Pixels = px(18.);
/// Font size bounds for Cmd-+ / Cmd--.
pub const MIN_FONT_SIZE: Pixels = px(9.);
pub const MAX_FONT_SIZE: Pixels = px(28.);

/// Line height for a font size, in the default's proportion (14 / 18), whole pixels so rows
/// don't drift.
pub fn line_height(font_size: Pixels) -> Pixels {
    (font_size * (f32::from(LINE_HEIGHT) / f32::from(FONT_SIZE))).round()
}
pub const PADDING: Pixels = px(8.);

// Colors from Zed's "One Dark Pro Glass" theme (one-dark-pro-max extension). The theme leaves
// terminal.foreground unset, so Zed's default #fffffff2 is used, flattened.
const FOREGROUND: Rgb = Rgb {
    r: 0xf2,
    g: 0xf2,
    b: 0xf2,
};
const BACKGROUND: Rgb = Rgb {
    r: 0x08,
    g: 0x09,
    b: 0x09,
};
/// How much of its color faint (SGR 2) text keeps.
const DIM_FACTOR: f32 = 0.66;
const CURSOR: u32 = 0x74ade8;
const SELECTION: u32 = 0x67769640;
// ANSI 0-15.
pub const ANSI: [u32; 16] = [
    0x3f4451, 0xe05561, 0x8cc265, 0xd18f52, 0x4aa5f0, 0xc162de, 0x42b3c2, 0xd7dae0, //
    0x4f5666, 0xff616e, 0xa5e075, 0xf0a45d, 0x4dc4ff, 0xde73ff, 0x4cd1e0, 0xe6e6e6,
];

actions!(terminal, [Copy, Paste, SelectAll, Clear]);

/// PTY events handled per entity update.
const EVENT_BATCH: usize = 256;

/// Key context for the terminal's bindings (Cmd-C, Cmd-V).
pub const KEY_CONTEXT: &str = "Terminal";

// xterm mouse protocol.
const MOTION: u8 = 32;
/// Legacy encoding has no button on release.
const RELEASE_BUTTON: u8 = 3;
const WHEEL_UP: u8 = 64;
const WHEEL_DOWN: u8 = 65;
/// Legacy encoding offsets every byte by 32 so it stays printable.
const COORD_OFFSET: usize = 32;

/// What a tab runs: `claude` plus arguments, in a folder.
#[derive(Clone, Debug)]
pub struct Launch {
    pub claude: Claude,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

/// Grid size and where the terminal sits in the window (for mouse hit testing). The workspace
/// estimates it for every tab; the visible tab corrects it from its painted bounds (`fit`).
#[derive(Clone, Copy, PartialEq)]
pub struct Layout {
    pub columns: usize,
    pub rows: usize,
    pub font_size: Pixels,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub origin: Point<Pixels>,
}

pub enum TerminalEvent {
    /// `claude` exited; the tab should close. `code` is None when a signal ended it.
    Exited {
        code: Option<i32>,
        /// Last non-empty screen line: usually why a launch failed.
        last_line: Option<String>,
    },
    /// The program set the window title (OSC 0/2), or reset it to empty.
    Title(String),
}

/// Forwards terminal events from the PTY thread to the UI thread.
#[derive(Clone)]
struct Listener(UnboundedSender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        self.0.unbounded_send(event).ok();
    }
}

pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    pty: Notifier,
    /// The pty's child. Because of `exec`, this is the `claude` process itself.
    pub pid: u32,
    layout: Layout,
    scroll_remainder: f32,
    /// Last cell a mouse report was sent for, so motion is reported once per cell.
    last_mouse_cell: Option<(usize, usize)>,
    /// Exit code from `ChildExit`, which arrives just before `Exit`.
    exit_code: Option<i32>,
    /// Button pressed inside the grid: its drag and release are followed outside it too.
    pressed: Option<MouseButton>,
    focus_handle: FocusHandle,
    /// Last frame's cell snapshot, kept for its allocation.
    cells: Vec<CellSnapshot>,
    /// Regular, bold, italic, bold italic; built once, not per cell.
    fonts: [Font; 4],
    _events: Task<()>,
}

/// A running PTY, before it gets a view. Split from `Terminal::new` so a failed launch is an
/// error the sidebar can show instead of a panic.
pub struct Spawned {
    term: Arc<FairMutex<Term<Listener>>>,
    pty: Notifier,
    pid: u32,
    events: UnboundedReceiver<Event>,
}

impl EventEmitter<TerminalEvent> for Terminal {}

impl Focusable for Terminal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Drop for Terminal {
    /// Stops the PTY thread; dropping its `Pty` sends claude SIGHUP.
    fn drop(&mut self) {
        self.pty.0.send(Msg::Shutdown).ok();
    }
}

impl Terminal {
    /// Starts `claude` in a PTY: the login shell loads the user's PATH and toolchain, then `exec`
    /// replaces it with claude, so there is no bare shell prompt to fall back to.
    pub fn spawn(launch: &Launch, layout: Layout) -> Result<Spawned, String> {
        // The child ignores a failed `chdir` and would run claude wherever this app runs.
        if !launch.cwd.is_dir() {
            return Err(format!("{} no longer exists", launch.cwd.display()));
        }
        // claude itself, with the login shell's environment: no shell parses the arguments.
        let options = tty::Options {
            shell: Some(Shell::new(
                launch.claude.path.to_string_lossy().into_owned(),
                launch.args.clone(),
            )),
            working_directory: Some(launch.cwd.clone()),
            env: launch.claude.env.clone(),
            ..Default::default()
        };
        let pty = tty::new(&options, window_size(&layout), 0)
            .map_err(|error| format!("Can't open a terminal: {error}"))?;
        let pid = pty.child().id();

        let (events_tx, events) = unbounded();
        let listener = Listener(events_tx);
        let term = Arc::new(FairMutex::new(Term::new(
            Config::default(),
            &TermSize::new(layout.columns, layout.rows),
            listener.clone(),
        )));
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)
            .map_err(|error| format!("Can't start the terminal: {error}"))?;
        let pty = Notifier(event_loop.channel());
        event_loop.spawn();
        Ok(Spawned {
            term,
            pty,
            pid,
            events,
        })
    }

    pub fn new(spawned: Spawned, layout: Layout, cx: &mut Context<Self>) -> Self {
        let Spawned {
            term,
            pty,
            pid,
            events: events_rx,
        } = spawned;
        let events = cx.spawn(async move |this, cx| {
            // Everything queued since the last turn is handled in one update: a busy claude
            // sends a wakeup per PTY read.
            let mut batches = events_rx.ready_chunks(EVENT_BATCH);
            while let Some(batch) = batches.next().await {
                let alive = this.update(cx, |this, cx| {
                    for event in batch {
                        this.handle_event(event, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let regular = font(FONT_FAMILY);
        let bold = Font {
            weight: FontWeight::BOLD,
            ..regular.clone()
        };
        let italic = Font {
            style: FontStyle::Italic,
            ..regular.clone()
        };
        let bold_italic = Font {
            style: FontStyle::Italic,
            ..bold.clone()
        };
        Self {
            term,
            pty,
            pid,
            layout,
            scroll_remainder: 0.,
            last_mouse_cell: None,
            exit_code: None,
            pressed: None,
            focus_handle: cx.focus_handle(),
            cells: Vec::new(),
            fonts: [regular, bold, italic, bold_italic],
            _events: events,
        }
    }

    fn handle_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            // Several wakeups before the next frame still render once.
            Event::Wakeup => cx.notify(),
            Event::PtyWrite(text) => self.pty.notify(text.into_bytes()),
            // OSC 4/10/11/12 queries; unanswered, programs probing the theme stall.
            Event::ColorRequest(index, format) => {
                let color =
                    self.term.lock().colors()[index].unwrap_or_else(|| default_color(index));
                self.pty.notify(format(color).into_bytes());
            }
            // CSI 14 t / 18 t: text area size in pixels or cells.
            Event::TextAreaSizeRequest(format) => {
                self.pty
                    .notify(format(window_size(&self.layout)).into_bytes());
            }
            Event::Title(title) => cx.emit(TerminalEvent::Title(title)),
            Event::ResetTitle => cx.emit(TerminalEvent::Title(String::new())),
            Event::ChildExit(status) => self.exit_code = status.code(),
            Event::Exit => cx.emit(TerminalEvent::Exited {
                code: self.exit_code,
                last_line: self.last_line(),
            }),
            _ => {}
        }
    }

    fn write(&mut self, bytes: impl Into<Cow<'static, [u8]>>) {
        let mut term = self.term.lock();
        term.selection = None;
        term.scroll_display(Scroll::Bottom);
        drop(term);
        self.pty.notify(bytes);
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let app_cursor = self.term.lock().mode().contains(TermMode::APP_CURSOR);
        let Some(bytes) = key_bytes(
            &keystroke.key,
            keystroke.key_char.as_deref(),
            &keystroke.modifiers,
            app_cursor,
        ) else {
            return;
        };
        self.write(bytes);
        cx.stop_propagation();
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        // Files copied in Finder come first as paths; the text entry is only their names.
        if let Some(ClipboardEntry::ExternalPaths(paths)) = item.entries().first() {
            self.paste_text(&quote_paths(paths.paths()));
        } else if let Some(text) = item.text() {
            self.paste_text(&text);
        }
    }

    fn paste_text(&mut self, text: &str) {
        let text = paste_bytes(text);
        if self.term.lock().mode().contains(TermMode::BRACKETED_PASTE) {
            self.write(format!("\x1b[200~{text}\x1b[201~").into_bytes());
        } else {
            self.write(text.into_bytes());
        }
    }

    /// Dropped files/folders are typed in as shell-quoted paths, like Terminal.app.
    fn drop_paths(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        self.paste_text(&quote_paths(paths.paths()));
        window.focus(&self.focus_handle, cx);
    }

    /// Scrollback and screen, ready for Cmd-C.
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let mut term = self.term.lock();
        let grid = term.grid();
        let start = GridPoint::new(grid.topmost_line(), Column(0));
        let end = GridPoint::new(grid.bottommost_line(), grid.last_column());
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.update(end, Side::Right);
        term.selection = Some(selection);
        drop(term);
        cx.notify();
    }

    /// Cmd-K: drops the scrollback. The screen stays: clearing it under a running full-screen
    /// program (claude) would leave it blank until the program redraws.
    fn clear(&mut self, _: &Clear, _: &mut Window, cx: &mut Context<Self>) {
        let mut term = self.term.lock();
        term.selection = None;
        term.scroll_display(Scroll::Bottom);
        term.grid_mut().clear_history();
        drop(term);
        cx.notify();
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.term.lock().selection_to_string() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Window position to the visible cell (column, row), plus which half of the cell was hit.
    fn cell(&self, position: Point<Pixels>) -> (usize, usize, Side) {
        let layout = &self.layout;
        let x = ((position.x - layout.origin.x - PADDING) / layout.cell_width).max(0.);
        let y = ((position.y - layout.origin.y - PADDING) / layout.line_height).max(0.);
        let column = (x as usize).min(layout.columns - 1);
        let row = (y as usize).min(layout.rows - 1);
        let side = if x.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        (column, row, side)
    }

    /// Window position to grid cell (scrollback aware), plus which half of the cell was hit.
    fn grid_point(&self, position: Point<Pixels>) -> (GridPoint, Side) {
        let display_offset = self.term.lock().grid().display_offset() as i32;
        let (column, row, side) = self.cell(position);
        (
            GridPoint::new(Line(row as i32 - display_offset), Column(column)),
            side,
        )
    }

    /// The program asked for mouse events (xterm 1000/1002/1003). Shift bypasses it so text can
    /// still be selected, like xterm.
    fn mouse_mode(&self, modifiers: &Modifiers) -> bool {
        !modifiers.shift && self.term.lock().mode().intersects(TermMode::MOUSE_MODE)
    }

    /// Sends one xterm mouse report: SGR (1006) when enabled, else the legacy `ESC [ M` form.
    fn report_mouse(
        &mut self,
        code: u8,
        pressed: bool,
        position: Point<Pixels>,
        modifiers: &Modifiers,
    ) {
        let (column, row, _) = self.cell(position);
        self.last_mouse_cell = Some((column, row));
        let sgr = self.term.lock().mode().contains(TermMode::SGR_MOUSE);
        let bytes = mouse_report(code, pressed, column, row, modifiers, sgr);
        if let Some(bytes) = bytes {
            self.pty.notify(bytes);
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        self.pressed = Some(event.button);
        if self.mouse_mode(&event.modifiers) {
            if let Some(code) = button_code(event.button) {
                self.report_mouse(code, true, event.position, &event.modifiers);
            }
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
        let (point, side) = self.grid_point(event.position);
        let kind = match event.click_count {
            2 => SelectionType::Semantic,
            3.. => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        self.term.lock().selection = Some(Selection::new(kind, point, side));
        cx.notify();
    }

    /// Window-wide while a button is held, so a release over the sidebar still ends the drag.
    fn mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        if self.pressed != Some(event.button) {
            return;
        }
        self.pressed = None;
        if self.mouse_mode(&event.modifiers)
            && let Some(code) = button_code(event.button)
        {
            self.report_mouse(code, false, event.position, &event.modifiers);
        }
    }

    /// Hover motion comes from the grid; drags from the window (`drag_listeners`), so they
    /// keep going outside it.
    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.mouse_mode(&event.modifiers) {
            let mode = *self.term.lock().mode();
            // With no button held the button bits read as a release.
            let code = match event.pressed_button.and_then(button_code) {
                Some(code) if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) => {
                    code + MOTION
                }
                None if mode.contains(TermMode::MOUSE_MOTION) => RELEASE_BUTTON + MOTION,
                _ => return,
            };
            let (column, row, _) = self.cell(event.position);
            if self.last_mouse_cell != Some((column, row)) {
                self.report_mouse(code, true, event.position, &event.modifiers);
            }
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let (point, side) = self.grid_point(event.position);
        if let Some(selection) = self.term.lock().selection.as_mut() {
            selection.update(point, side);
            cx.notify();
        }
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let line_height = self.layout.line_height;
        self.scroll_remainder += event.delta.pixel_delta(line_height).y / line_height;
        let lines = self.scroll_remainder.trunc();
        if lines == 0. {
            return;
        }
        self.scroll_remainder -= lines;
        let up = lines > 0.;
        let count = lines.abs() as usize;
        let mode = *self.term.lock().mode();
        if self.mouse_mode(&event.modifiers) {
            // The wheel is two buttons, press only.
            for _ in 0..count {
                self.report_mouse(
                    if up { WHEEL_UP } else { WHEEL_DOWN },
                    true,
                    event.position,
                    &event.modifiers,
                );
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            // Full-screen apps without mouse mode get arrow keys, like xterm's alternateScroll.
            let arrow = cursor_key(
                mode.contains(TermMode::APP_CURSOR),
                if up { 'A' } else { 'B' },
            );
            self.pty.notify(arrow.repeat(count).into_bytes());
        } else {
            self.term.lock().scroll_display(Scroll::Delta(lines as i32));
            cx.notify();
        }
    }

    /// Called for every tab, hidden ones too, so a background claude redraws at the right width.
    pub fn set_layout(&mut self, layout: Layout) {
        let resized = (layout.columns, layout.rows) != (self.layout.columns, self.layout.rows);
        self.layout = layout;
        if resized {
            self.term
                .lock()
                .resize(TermSize::new(layout.columns, layout.rows));
            self.pty.on_resize(window_size(&layout));
        }
    }
}

/// One visible cell, copied out so the PTY lock is not held while text runs are built.
struct CellSnapshot {
    row: usize,
    point: GridPoint,
    c: char,
    /// Combining marks and joiners drawn on top of `c` (none for almost every cell).
    zerowidth: Option<Vec<char>>,
    fg: Color,
    bg: Color,
    flags: Flags,
}

/// A painted glyph, or a run of identical ones (`─`, `█`, ...) drawn as one shape.
struct GlyphRun {
    column: usize,
    row: usize,
    /// Cells covered.
    width: usize,
    c: char,
    color: Hsla,
}

impl GlyphRun {
    /// Glyphs that tile horizontally: a run of them is the same glyph drawn wider. Claude Code
    /// draws whole-width rules and bars with these.
    const TILES: [char; 8] = ['─', '━', '═', '█', '▀', '▄', '▔', '▁'];

    fn push(runs: &mut Vec<GlyphRun>, column: usize, row: usize, c: char, color: Hsla) {
        if let Some(last) = runs.last_mut()
            && Self::TILES.contains(&c)
            && last.c == c
            && last.row == row
            && last.color == color
            && last.column + last.width == column
        {
            last.width += 1;
            return;
        }
        runs.push(GlyphRun {
            column,
            row,
            width: 1,
            c,
            color,
        });
    }
}

/// A piece of one grid row drawn as one `StyledText`, placed at its column. ASCII cells batch
/// into one segment; any other char gets its own, because a fallback font's advance isn't the
/// grid's cell width and would shift every cell after it.
#[derive(Default)]
struct Segment {
    column: usize,
    /// Column after the last cell; the next ASCII cell joins only if it lands exactly here.
    end: usize,
    /// Takes more ASCII cells.
    open: bool,
    text: String,
    runs: Vec<TextRun>,
    /// Font index of each run (cheaper to compare than a `Font`).
    run_fonts: Vec<usize>,
}

impl Segment {
    /// Makes the last segment the one a cell at `column` spanning `width` cells goes into:
    /// extends it, or starts a new one.
    fn place(segments: &mut Vec<Segment>, column: usize, width: usize, solo: bool) {
        match segments.last_mut() {
            Some(last) if !solo && last.open && last.end == column => last.end = column + width,
            _ => segments.push(Segment {
                column,
                end: column + width,
                open: !solo,
                ..Default::default()
            }),
        }
    }
}

impl Terminal {
    /// Paints `(column, row, char, color)` glyph cells over the text, one cell each, plus the
    /// hollow cursor (column, row) when the window isn't focused.
    fn glyph_canvas(
        &self,
        cells: Vec<GlyphRun>,
        hollow_cursor: Option<(usize, usize)>,
    ) -> impl IntoElement {
        let cell_width = self.layout.cell_width;
        let line_height = self.layout.line_height;
        canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let scale = window.scale_factor();
                let cell_bounds = |column: usize, row: usize, width: usize| {
                    let origin =
                        bounds.origin + point(cell_width * column as f32, line_height * row as f32);
                    Bounds::new(origin, size(cell_width * width as f32, line_height))
                };
                for run in cells {
                    let bounds = cell_bounds(run.column, run.row, 1);
                    let shapes = glyphs::shapes(run.c, bounds, run.width, scale);
                    for shape in shapes.into_iter().flatten() {
                        glyphs::paint(shape, run.color, window);
                    }
                }
                // macOS convention (Terminal, Zed): an inactive window's cursor is an outline.
                if let Some((column, row)) = hollow_cursor {
                    window.paint_quad(outline(
                        cell_bounds(column, row, 1),
                        rgb(CURSOR),
                        BorderStyle::Solid,
                    ));
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}

impl Terminal {
    /// Release and drag listeners on the whole window while a button is held in the grid. Element
    /// mouse listeners only fire over the element, so a drag onto the sidebar would stall and a
    /// mouse-reporting program would never see the release.
    fn drag_listeners(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let up = cx.entity().downgrade();
        let moved = up.clone();
        canvas(
            |_, _, _| {},
            move |_, (), window, _| {
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase.bubble() {
                        up.update(cx, |this, cx| this.mouse_up(event, window, cx))
                            .ok();
                    }
                });
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase.bubble() {
                        moved
                            .update(cx, |this, cx| {
                                if this.pressed.is_some() {
                                    this.mouse_move(event, window, cx);
                                }
                            })
                            .ok();
                    }
                });
            },
        )
        .absolute()
        .size_0()
    }

    /// Fits the grid to the bounds this terminal was actually painted in, so the PTY size and
    /// mouse hit testing follow the real layout rather than the workspace's arithmetic.
    fn measure(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                let resized = this
                    .update(cx, |terminal, _| terminal.fit(bounds))
                    .unwrap_or(false);
                // A notify while painting is dropped; this one lands on the next frame.
                if resized {
                    window.request_animation_frame();
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// Returns whether the grid size changed (the frame just painted used the old one).
    fn fit(&mut self, bounds: Bounds<Pixels>) -> bool {
        let layout = Layout {
            columns: ((bounds.size.width - PADDING * 2.) / self.layout.cell_width).max(2.) as usize,
            rows: ((bounds.size.height - PADDING * 2.) / self.layout.line_height).max(1.) as usize,
            origin: bounds.origin,
            ..self.layout
        };
        let resized = (layout.columns, layout.rows) != (self.layout.columns, self.layout.rows);
        if layout != self.layout {
            self.set_layout(layout);
        }
        resized
    }

    /// The last non-empty line on screen, trimmed.
    fn last_line(&self) -> Option<String> {
        let term = self.term.lock();
        let grid = term.grid();
        (0..grid.screen_lines()).rev().find_map(|line| {
            let row = &grid[Line(line as i32)];
            let text: String = (0..grid.columns())
                .map(|column| row[Column(column)].c)
                .collect();
            let text = text.trim_matches(|c: char| c.is_whitespace() || c == '\0');
            (!text.is_empty()).then(|| text.to_string())
        })
    }
}

impl Render for Terminal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Solid block only while keys would land here; the workspace re-renders tabs on window
        // (de)activation, since this view is cached.
        let focused = window.is_window_active() && self.focus_handle.is_focused(window);
        // Copy under the lock; the PTY reader thread waits on it.
        let term = self.term.lock();
        let content = term.renderable_content();
        let cursor = (content.cursor.shape != CursorShape::Hidden).then_some(content.cursor.point);
        let selection = content.selection;
        let colors = *content.colors;
        let display_offset = content.display_offset as i32;
        // Reused across frames: a full screen is thousands of cells.
        let mut cells = std::mem::take(&mut self.cells);
        cells.clear();
        cells.extend(
            content
                .display_iter
                .filter(|indexed| !indexed.cell.flags.contains(Flags::WIDE_CHAR_SPACER))
                .map(|indexed| CellSnapshot {
                    row: (indexed.point.line.0 + display_offset) as usize,
                    point: indexed.point,
                    c: indexed.cell.c,
                    zerowidth: indexed.cell.zerowidth().map(<[char]>::to_vec),
                    fg: indexed.cell.fg,
                    bg: indexed.cell.bg,
                    flags: indexed.cell.flags,
                }),
        );
        drop(term);

        let mut rows: Vec<Vec<Segment>> = (0..self.layout.rows).map(|_| Vec::new()).collect();
        // Drawn as shapes, not font glyphs: glyphs at fractional x leave seams.
        let mut glyphs = Vec::new();
        let mut hollow_cursor = None;
        let mut last_colors = None;
        for cell in &cells {
            let Some(segments) = rows.get_mut(cell.row) else {
                continue;
            };
            if !focused && cursor == Some(cell.point) {
                hollow_cursor = Some((cell.point.column.0, cell.row));
            }
            let block_cursor = cursor.filter(|_| focused);
            // Neighbouring cells usually share colors; convert once per run of them.
            let key = (
                cell.fg,
                cell.bg,
                cell.flags,
                block_cursor == Some(cell.point),
                selection.is_some_and(|range| range.contains(cell.point)),
            );
            let (fg, bg) = match last_colors {
                Some((last_key, colors)) if last_key == key => colors,
                _ => {
                    let resolved = cell_colors(cell, &colors, block_cursor, selection);
                    last_colors = Some((key, resolved));
                    resolved
                }
            };
            let font_index = cell_font(cell.flags);
            let mut ch = if cell.c == '\0' { ' ' } else { cell.c };
            if glyphs::covers(ch) {
                GlyphRun::push(&mut glyphs, cell.point.column.0, cell.row, ch, fg);
                ch = ' ';
            }

            let width = if cell.flags.contains(Flags::WIDE_CHAR) {
                2
            } else {
                1
            };
            let solo = !ch.is_ascii() || width == 2 || cell.zerowidth.is_some();
            Segment::place(segments, cell.point.column.0, width, solo);
            let Some(segment) = segments.last_mut() else {
                continue;
            };
            let start = segment.text.len();
            segment.text.push(ch);
            segment.text.extend(cell.zerowidth.iter().flatten());
            let len = segment.text.len() - start;
            match segment.runs.last_mut() {
                Some(run)
                    if run.color == fg
                        && run.background_color == bg
                        && segment.run_fonts.last() == Some(&font_index) =>
                {
                    run.len += len;
                }
                _ => {
                    segment.runs.push(TextRun {
                        len,
                        font: self.fonts[font_index].clone(),
                        color: fg,
                        background_color: bg,
                        underline: None,
                        strikethrough: None,
                    });
                    segment.run_fonts.push(font_index);
                }
            }
        }

        self.cells = cells;

        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::clear))
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if this.pressed.is_none() {
                    this.mouse_move(event, window, cx);
                }
            }))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .on_drop(cx.listener(Self::drop_paths))
            .relative()
            .size_full()
            .p(PADDING)
            .child(self.measure(cx))
            .font_family(FONT_FAMILY)
            .text_size(self.layout.font_size)
            .line_height(self.layout.line_height)
            .child(
                div()
                    .relative()
                    .children(rows.into_iter().map(|segments| {
                        div().h(self.layout.line_height).relative().children(
                            segments.into_iter().map(|segment| {
                                div()
                                    .absolute()
                                    .left(self.layout.cell_width * segment.column as f32)
                                    .whitespace_nowrap()
                                    .child(StyledText::new(segment.text).with_runs(segment.runs))
                            }),
                        )
                    }))
                    .child(self.glyph_canvas(glyphs, hollow_cursor))
                    .child(self.drag_listeners(cx)),
            )
    }
}

/// Foreground and (optional) background for a cell. No background means the glass shows.
fn cell_colors(
    cell: &CellSnapshot,
    colors: &Colors,
    cursor: Option<GridPoint>,
    selection: Option<alacritty_terminal::selection::SelectionRange>,
) -> (Hsla, Option<Hsla>) {
    let mut fg = cell.fg;
    if cell.flags.contains(Flags::BOLD)
        && let Color::Named(named) = fg
        && (named as usize) < 8
    {
        fg = Color::Named(named.to_bright());
    }
    let mut fg = resolve(fg, colors);
    // Faint (SGR 2): Claude Code draws its prompt suggestions this way.
    if cell.flags.intersects(Flags::DIM) {
        fg = dim(fg);
    }
    let mut fg = to_hsla(fg);
    // Default background stays unpainted so the glass shows through.
    let default_bg =
        cell.bg == Color::Named(NamedColor::Background) && colors[NamedColor::Background].is_none();
    let mut bg = (!default_bg).then(|| to_hsla(resolve(cell.bg, colors)));
    if cell.flags.contains(Flags::INVERSE) {
        (fg, bg) = (bg.unwrap_or(to_hsla(BACKGROUND)), Some(fg));
    }
    if cell.flags.contains(Flags::HIDDEN) {
        fg = bg.unwrap_or(to_hsla(BACKGROUND));
    }
    if cursor == Some(cell.point) {
        (fg, bg) = (to_hsla(BACKGROUND), Some(rgb(CURSOR).into()));
    } else if selection.is_some_and(|range| range.contains(cell.point)) {
        bg = Some(rgba(SELECTION).into());
    }
    (fg, bg)
}

/// Index into `Terminal::fonts`.
fn cell_font(flags: Flags) -> usize {
    usize::from(flags.contains(Flags::BOLD)) + 2 * usize::from(flags.contains(Flags::ITALIC))
}

fn button_code(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        _ => None,
    }
}

/// xterm mouse report for a 0-based cell. `code` is the button (0-2, or a wheel), plus `MOTION`.
fn mouse_report(
    code: u8,
    pressed: bool,
    column: usize,
    row: usize,
    modifiers: &Modifiers,
    sgr: bool,
) -> Option<Vec<u8>> {
    let mut code = code;
    if modifiers.shift {
        code += 4;
    }
    if modifiers.alt {
        code += 8;
    }
    if modifiers.control {
        code += 16;
    }
    if sgr {
        let suffix = if pressed { 'M' } else { 'm' };
        return Some(format!("\x1b[<{code};{};{}{suffix}", column + 1, row + 1).into_bytes());
    }
    // Legacy encoding: release has no button (3), and coordinates must fit in one byte.
    if !pressed {
        code = (code & !0b11) | RELEASE_BUTTON;
    }
    let column = u8::try_from(column + 1 + COORD_OFFSET).ok()?;
    let row = u8::try_from(row + 1 + COORD_OFFSET).ok()?;
    Some(vec![
        0x1b,
        b'[',
        b'M',
        COORD_OFFSET as u8 + code,
        column,
        row,
    ])
}

/// Bytes for a key press, or None when it isn't typed (Cmd shortcuts, unmapped keys).
///
/// Option is Meta for ASCII letters, digits and named keys (`ESC` + key, like iTerm's "Esc+"):
/// Claude Code and readline use Meta for word motion. Option with punctuation still types what
/// the OS composes. Trade-off: layouts that need Option+letter for a character (German `@` =
/// Option-L) can't type it here; fine on the U.S. layout this is built for.
fn key_bytes(
    key: &str,
    key_char: Option<&str>,
    modifiers: &Modifiers,
    app_cursor: bool,
) -> Option<Vec<u8>> {
    let meta = |bytes: &[u8]| [b"\x1b", bytes].concat();
    if modifiers.platform {
        // macOS line editing: start, end, and delete to start of line. Other Cmd keys are
        // actions (bound in main), or nothing.
        return match key {
            "left" => Some(vec![0x01]),
            "right" => Some(vec![0x05]),
            "backspace" => Some(vec![0x15]),
            _ => None,
        };
    }
    // xterm modifier parameter: 1 + Shift 1 + Alt 2 + Ctrl 4.
    let param = 1
        + u8::from(modifiers.shift)
        + 2 * u8::from(modifiers.alt)
        + 4 * u8::from(modifiers.control);
    let csi = |final_byte: char| match param {
        1 => format!("\x1b[{final_byte}"),
        _ => format!("\x1b[1;{param}{final_byte}"),
    };
    let tilde = |code: u8| match param {
        1 => format!("\x1b[{code}~"),
        _ => format!("\x1b[{code};{param}~"),
    };
    let arrow = |code: char| match param {
        1 => cursor_key(app_cursor, code),
        _ => csi(code),
    };
    let bytes = match key {
        // Claude Code reads ESC CR as Option+Enter: a newline instead of submit.
        "enter" if modifiers.shift || modifiers.control || modifiers.alt => meta(b"\r"),
        "enter" => b"\r".to_vec(),
        "backspace" if modifiers.alt => meta(b"\x7f"),
        "backspace" if modifiers.control => vec![0x08],
        "backspace" => b"\x7f".to_vec(),
        "tab" if modifiers.shift => b"\x1b[Z".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" => b"\x1b".to_vec(),
        // Option-arrows move by word, as in Terminal.app.
        "left" if param == 3 => meta(b"b"),
        "right" if param == 3 => meta(b"f"),
        "up" => arrow('A').into_bytes(),
        "down" => arrow('B').into_bytes(),
        "right" => arrow('C').into_bytes(),
        "left" => arrow('D').into_bytes(),
        "home" => csi('H').into_bytes(),
        "end" => csi('F').into_bytes(),
        "pageup" => tilde(5).into_bytes(),
        "pagedown" => tilde(6).into_bytes(),
        "delete" => tilde(3).into_bytes(),
        "f1" | "f2" | "f3" | "f4" => {
            let final_byte = (b'P' + key.as_bytes()[1] - b'1') as char;
            match param {
                1 => format!("\x1bO{final_byte}"),
                _ => csi(final_byte),
            }
            .into_bytes()
        }
        "f5" => tilde(15).into_bytes(),
        "f6" => tilde(17).into_bytes(),
        "f7" => tilde(18).into_bytes(),
        "f8" => tilde(19).into_bytes(),
        "f9" => tilde(20).into_bytes(),
        "f10" => tilde(21).into_bytes(),
        "f11" => tilde(23).into_bytes(),
        "f12" => tilde(24).into_bytes(),
        "space" if modifiers.control => vec![0],
        "space" if modifiers.alt => meta(b" "),
        key if modifiers.control && key.len() == 1 => match key.as_bytes()[0] {
            byte @ (b'a'..=b'z' | b'@' | b'[' | b'\\' | b']' | b'^' | b'_') => {
                let control = byte & 0x1f;
                if modifiers.alt {
                    meta(&[control])
                } else {
                    vec![control]
                }
            }
            _ => return None,
        },
        key if modifiers.alt && key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric() => {
            // `key` is lowercase; Shift picks the capital (Meta-B, not Meta-b).
            let byte = key.as_bytes()[0];
            meta(&[if modifiers.shift {
                byte.to_ascii_uppercase()
            } else {
                byte
            }])
        }
        _ => key_char?.as_bytes().to_vec(),
    };
    Some(bytes)
}

/// Arrow key bytes; full-screen apps switch to application cursor mode (`ESC O`).
fn cursor_key(app_cursor: bool, code: char) -> String {
    let prefix = if app_cursor { "\x1bO" } else { "\x1b[" };
    format!("{prefix}{code}")
}

/// macOS (APFS) paths are UTF-8, so the lossy conversion never changes one in practice.
fn quote_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .filter_map(|path| {
            shlex::try_quote(&path.to_string_lossy())
                .ok()
                .map(Cow::into_owned)
        })
        .map(|path| format!("{path} "))
        .collect()
}

fn window_size(layout: &Layout) -> WindowSize {
    WindowSize {
        num_cols: layout.columns as u16,
        num_lines: layout.rows as u16,
        cell_width: f32::from(layout.cell_width) as u16,
        cell_height: f32::from(layout.line_height) as u16,
    }
}

/// Program-set colors (OSC 4/10/11) win; otherwise our palette.
fn resolve(color: Color, colors: &Colors) -> Rgb {
    match color {
        Color::Spec(rgb) => rgb,
        Color::Named(named) => colors[named].unwrap_or_else(|| default_named(named)),
        Color::Indexed(index) => colors[index as usize].unwrap_or_else(|| default_indexed(index)),
    }
}

/// Palette entry by alacritty color index: 0-255 indexed, then `NamedColor` slots.
fn default_color(index: usize) -> Rgb {
    match index {
        0..256 => default_indexed(index as u8),
        _ if index == NamedColor::Background as usize => BACKGROUND,
        _ if index == NamedColor::Cursor as usize => {
            let [_, r, g, b] = CURSOR.to_be_bytes();
            Rgb { r, g, b }
        }
        _ => FOREGROUND,
    }
}

fn default_named(named: NamedColor) -> Rgb {
    let dims = NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize;
    match named as usize {
        index @ 0..16 => default_indexed(index as u8),
        _ if named == NamedColor::Background => BACKGROUND,
        index if dims.contains(&index) => dim(default_indexed((index - dims.start()) as u8)),
        _ => FOREGROUND,
    }
}

/// xterm 256-color table: 16 ANSI, 6x6x6 cube, 24 grays.
fn default_indexed(index: u8) -> Rgb {
    match index {
        0..16 => {
            let [_, r, g, b] = ANSI[index as usize].to_be_bytes();
            Rgb { r, g, b }
        }
        16..232 => {
            let level = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
            let n = index - 16;
            Rgb {
                r: level(n / 36),
                g: level(n / 6 % 6),
                b: level(n % 6),
            }
        }
        _ => {
            let gray = 8 + (index - 232) * 10;
            Rgb {
                r: gray,
                g: gray,
                b: gray,
            }
        }
    }
}

/// Faint text: blended toward the background, like Alacritty's 0.66 dim factor.
fn dim(color: Rgb) -> Rgb {
    let blend = |c: u8, bg: u8| (bg as f32 + (c as f32 - bg as f32) * DIM_FACTOR).round() as u8;
    Rgb {
        r: blend(color.r, BACKGROUND.r),
        g: blend(color.g, BACKGROUND.g),
        b: blend(color.b, BACKGROUND.b),
    }
}

fn to_hsla(color: Rgb) -> Hsla {
    rgb(u32::from_be_bytes([0, color.r, color.g, color.b])).into()
}

/// Clipboard text as typed: newlines become Enter. Control chars are dropped: they could end
/// a bracketed paste early (`ESC [201~`) and turn the rest into keystrokes, e.g. answering a
/// permission prompt.
fn paste_bytes(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
        .collect::<String>()
        .replace("\r\n", "\r")
        .replace('\n', "\r")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Debug aid, not a check: runs the real `claude` in a headless `Term` and prints the bottom
    /// rows with their colors. `cargo test dump_claude_screen -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_claude_screen() {
        use std::time::{Duration, Instant};
        let claude = crate::launch::resolve().unwrap();
        let layout = Layout {
            columns: 100,
            rows: 30,
            font_size: FONT_SIZE,
            cell_width: px(8.),
            line_height: LINE_HEIGHT,
            origin: point(px(0.), px(0.)),
        };
        let options = tty::Options {
            shell: Some(Shell::new(
                claude.path.to_string_lossy().into_owned(),
                Vec::new(),
            )),
            working_directory: std::env::current_dir().ok(),
            env: claude.env.clone(),
            ..Default::default()
        };
        let pty = tty::new(&options, window_size(&layout), 0).unwrap();
        let (events_tx, mut events) = unbounded();
        let listener = Listener(events_tx);
        let term = Arc::new(FairMutex::new(Term::new(
            Config::default(),
            &TermSize::new(layout.columns, layout.rows),
            listener.clone(),
        )));
        let event_loop = EventLoop::new(term.clone(), listener, pty, false, false).unwrap();
        let notifier = Notifier(event_loop.channel());
        event_loop.spawn();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(12) {
            match events.try_recv() {
                Ok(Event::PtyWrite(text)) => notifier.notify(text.into_bytes()),
                Ok(Event::ColorRequest(index, format)) => {
                    let color = term.lock().colors()[index].unwrap_or_else(|| default_color(index));
                    notifier.notify(format(color).into_bytes());
                }
                Ok(Event::TextAreaSizeRequest(format)) => {
                    notifier.notify(format(window_size(&layout)).into_bytes())
                }
                Ok(_) => {}
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        let term = term.lock();
        let grid = term.grid();
        for line in 0..layout.rows as i32 {
            let row = &grid[Line(line)];
            let text: String = (0..layout.columns)
                .map(|column| row[Column(column)].c)
                .collect();
            println!("{line:2} |{}|", text.replace('\0', " "));
            // Runs of identical (fg, bg, flags) for cells that aren't plain default.
            let mut column = 0;
            while column < layout.columns {
                let cell = &row[Column(column)];
                let key = (cell.fg, cell.bg, cell.flags);
                let start = column;
                while column < layout.columns {
                    let next = &row[Column(column)];
                    if (next.fg, next.bg, next.flags) != key {
                        break;
                    }
                    column += 1;
                }
                let plain = cell.bg == Color::Named(NamedColor::Background)
                    && cell.fg == Color::Named(NamedColor::Foreground)
                    && cell.flags.is_empty();
                if !plain {
                    let chars: String = (start..column).map(|c| row[Column(c)].c).take(6).collect();
                    println!(
                        "      {start}..{column} fg={:?} bg={:?} flags={:?} {chars:?}",
                        cell.fg, cell.bg, cell.flags
                    );
                }
            }
        }
        drop(term);
        notifier.0.send(Msg::Shutdown).ok();
    }

    #[test]
    fn mouse_reports_match_xterm() {
        let none = Modifiers::default();
        let ctrl = Modifiers {
            control: true,
            ..Default::default()
        };
        assert_eq!(
            mouse_report(0, true, 4, 9, &none, true).unwrap(),
            b"\x1b[<0;5;10M"
        );
        assert_eq!(
            mouse_report(0, false, 4, 9, &none, true).unwrap(),
            b"\x1b[<0;5;10m"
        );
        assert_eq!(
            mouse_report(64, true, 0, 0, &ctrl, true).unwrap(),
            b"\x1b[<80;1;1M"
        );
        assert_eq!(
            mouse_report(2, false, 0, 0, &none, false).unwrap(),
            b"\x1b[M#!!"
        );
        assert_eq!(mouse_report(0, true, 300, 0, &none, false), None);
    }

    #[test]
    fn line_height_scales_with_font_size() {
        assert_eq!(line_height(FONT_SIZE), LINE_HEIGHT);
        assert_eq!(line_height(px(28.)), px(36.));
        assert_eq!(line_height(px(9.)), px(12.));
    }

    #[test]
    fn fonts_and_cursor_keys() {
        assert_eq!(cell_font(Flags::empty()), 0);
        assert_eq!(cell_font(Flags::BOLD), 1);
        assert_eq!(cell_font(Flags::ITALIC), 2);
        assert_eq!(cell_font(Flags::BOLD | Flags::ITALIC), 3);
        assert_eq!(cursor_key(false, 'A'), "\x1b[A");
        assert_eq!(cursor_key(true, 'A'), "\x1bOA");
    }

    #[test]
    fn tiling_glyphs_merge_into_runs() {
        let white = gpui::hsla(0., 0., 1., 1.);
        let red = gpui::hsla(0., 1., 0.5, 1.);
        let mut runs = Vec::new();
        for column in 0..4 {
            GlyphRun::push(&mut runs, column, 0, '─', white);
        }
        GlyphRun::push(&mut runs, 4, 0, '─', red);
        GlyphRun::push(&mut runs, 5, 0, '╮', white);
        GlyphRun::push(&mut runs, 6, 0, '╮', white);
        GlyphRun::push(&mut runs, 0, 1, '─', white);
        let spans: Vec<(usize, usize, usize, char)> = runs
            .iter()
            .map(|run| (run.row, run.column, run.width, run.c))
            .collect();
        // Corners don't tile; a color change or a new row starts a new run.
        assert_eq!(
            spans,
            [
                (0, 0, 4, '─'),
                (0, 4, 1, '─'),
                (0, 5, 1, '╮'),
                (0, 6, 1, '╮'),
                (1, 0, 1, '─')
            ]
        );
    }

    #[test]
    fn rows_split_where_cell_width_is_not_guaranteed() {
        let mut segments = Vec::new();
        let mut put = |column, width, solo, ch| {
            Segment::place(&mut segments, column, width, solo);
            if let Some(segment) = segments.last_mut() {
                segment.text.push(ch);
            }
        };
        // "ab", a wide char at 2-3, "cd" at 4-5, then "e" after a gap: not joined.
        put(0, 1, false, 'a');
        put(1, 1, false, 'b');
        put(2, 2, true, '\u{4e2d}');
        put(4, 1, false, 'c');
        put(5, 1, false, 'd');
        put(7, 1, false, 'e');
        let placed: Vec<(usize, &str)> = segments
            .iter()
            .map(|segment| (segment.column, segment.text.as_str()))
            .collect();
        assert_eq!(placed, [(0, "ab"), (2, "\u{4e2d}"), (4, "cd"), (7, "e")]);
    }

    #[test]
    fn keys_follow_xterm_and_macos() {
        let none = Modifiers::default();
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        let cmd = Modifiers {
            platform: true,
            ..Default::default()
        };
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        let key = |key, key_char, modifiers: &Modifiers| key_bytes(key, key_char, modifiers, false);
        assert_eq!(key("b", Some("\u{222b}"), &alt), Some(b"\x1bb".to_vec()));
        assert_eq!(
            key(
                "b",
                Some("B"),
                &Modifiers {
                    alt: true,
                    shift: true,
                    ..Default::default()
                }
            ),
            Some(b"\x1bB".to_vec())
        );
        assert_eq!(key("l", Some("@"), &none), Some(b"@".to_vec()));
        assert_eq!(key("2", Some("\u{20ac}"), &alt), Some(b"\x1b2".to_vec()));
        assert_eq!(
            key("-", Some("\u{2013}"), &alt),
            Some("\u{2013}".as_bytes().to_vec())
        );
        assert_eq!(key("left", None, &alt), Some(b"\x1bb".to_vec()));
        assert_eq!(key("backspace", None, &alt), Some(b"\x1b\x7f".to_vec()));
        assert_eq!(key("enter", None, &alt), Some(b"\x1b\r".to_vec()));
        assert_eq!(key("left", None, &cmd), Some(vec![0x01]));
        assert_eq!(key("backspace", None, &cmd), Some(vec![0x15]));
        assert_eq!(key("c", Some("c"), &cmd), None);
        assert_eq!(key("up", None, &shift), Some(b"\x1b[1;2A".to_vec()));
        assert_eq!(key("up", None, &none), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_bytes("up", None, &none, true), Some(b"\x1bOA".to_vec()));
        assert_eq!(key("f1", None, &none), Some(b"\x1bOP".to_vec()));
        assert_eq!(key("f5", None, &none), Some(b"\x1b[15~".to_vec()));
        assert_eq!(key("delete", None, &shift), Some(b"\x1b[3;2~".to_vec()));
    }

    #[test]
    fn paste_cannot_escape_bracketed_mode() {
        assert_eq!(paste_bytes("a\x1b[201~1\r\nb\n"), "a[201~1\rb\r");
        assert_eq!(paste_bytes("tab\tok\u{9b}\x07"), "tab\tok");
    }

    #[test]
    fn dropped_paths_are_shell_quoted() {
        let paths = [PathBuf::from("/tmp/a b"), PathBuf::from("/tmp/it's.png")];
        assert_eq!(quote_paths(&paths), r#"'/tmp/a b' "/tmp/it's.png" "#);
    }

    #[test]
    fn faint_text_is_darker() {
        assert_eq!(dim(BACKGROUND), BACKGROUND);
        let faint = dim(FOREGROUND);
        assert!(faint.r < FOREGROUND.r && faint.r > BACKGROUND.r);
        assert_eq!(default_named(NamedColor::DimRed), dim(default_indexed(1)));
    }

    #[test]
    fn indexed_colors_match_xterm() {
        assert_eq!(default_indexed(16), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(default_indexed(196), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(
            default_indexed(231),
            Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
        assert_eq!(default_indexed(232), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(
            default_indexed(255),
            Rgb {
                r: 238,
                g: 238,
                b: 238
            }
        );
    }
}
