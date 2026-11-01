//! The sidebar: project switcher, OPEN tabs, HISTORY. Also the welcome pane and small shared
//! pieces (status glyphs, tooltip).

use std::{path::Path, time::Duration};

use gpui::{
    Action, Animation, AnimationExt, AnyElement, ClickEvent, Context, Entity, FontWeight,
    KeyContext, Modifiers, MouseButton, MouseDownEvent, SharedString, SpringAnimation,
    Subscription, WeakEntity, Window, anchored, deferred, div, prelude::*, pulsating_between, px,
    rgb, rgba,
};

use crate::{
    sessions::{self, Row, Status},
    terminal::FONT_FAMILY,
    theme::*,
    workspace::{
        ActivateTab, MenuAction, MenuTarget, NewSession, OpenProject, SHORTCUT_TABS, Tab, TabId,
        Workspace,
    },
};

/// Braille spinner frames (the bundled Nerd Font has them).
const SPINNER: [&str; 10] = [
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280f}",
];
/// One on/off cycle of the rename caret (macOS uses about 1.06 s).
const CARET_BLINK: Duration = Duration::from_millis(1060);
/// Horizontal padding inside the rename field (plus its 1px border, cancelled by margins).
const RENAME_INSET: gpui::Pixels = px(4.);
const SPINNER_CYCLE: Duration = Duration::from_millis(800);
const DOT: &str = "\u{25cf}";
const CHEVRON: &str = "\u{f078}";
const CHEVRON_RIGHT: &str = "\u{f054}";
/// Branches that say nothing about the session.
const PLAIN_BRANCHES: [&str; 3] = ["HEAD", "main", "master"];

/// The sidebar as its own view, embedded `.cached` by the workspace: terminal output and the
/// workspace's own redraws reuse it, and its spinner and springs redraw only it. The state lives
/// in `Workspace`; this re-renders whenever the workspace notifies.
pub struct Sidebar {
    workspace: WeakEntity<Workspace>,
    _observe: Subscription,
}

impl Sidebar {
    pub fn new(workspace: &Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        Self {
            workspace: workspace.downgrade(),
            _observe: cx.observe(workspace, |_, _, cx| cx.notify()),
        }
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.render_sidebar(window, cx).into_any_element()
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
}

impl Workspace {
    pub(crate) fn render_sidebar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open: Vec<AnyElement> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| self.render_tab(index, tab, window, cx))
            .collect();
        // Only ended sessions. Running ones are either this window's tabs (listed above) or run
        // elsewhere (another terminal, background agents) and are left out entirely.
        let ended = self.rows.iter().filter(|row| !row.live()).count();
        let history = self.history_rows();
        // Newest first, so today's rows are a prefix.
        let today_count = history
            .iter()
            .take_while(|row| sessions::is_today(row.updated))
            .count();
        // Each group keeps its heading even when it is the only one.
        let has_today = today_count > 0;
        let has_earlier = today_count < history.len();
        let more = ended >= self.history_limit;
        let rows: Vec<AnyElement> = history
            .iter()
            .map(|row| self.render_history(row, cx))
            .collect();
        let (today, earlier) = {
            let mut rows = rows;
            let earlier = rows.split_off(today_count);
            (rows, earlier)
        };

        div()
            .w_full()
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgba(sidebar()))
            .border_r_1()
            .border_color(rgba(HAIRLINE))
            // macOS sidebar body size.
            .text_size(px(13.))
            .child(self.render_title_bar(window, cx))
            .child(
                div()
                    .id("sessions")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_2()
                    .child(section("Current Agents", open.len(), self.waiting_count()))
                    .when(open.is_empty(), |list| {
                        list.child(hint("No agents running"))
                    })
                    .children(open)
                    // Hairline between Open and History, inset like the rows.
                    .child(div().mt_3().mx_2().h_px().bg(rgba(HAIRLINE)))
                    .child(self.render_history_header(ended, cx))
                    .when(self.history_open, |list| {
                        list.child(
                            div()
                                .flex()
                                .flex_col()
                                .when(ended == 0, |rows| {
                                    rows.child(hint("No past sessions in this project."))
                                })
                                .when(has_today, |rows| rows.child(group_label("Today", false)))
                                .children(today)
                                .when(has_earlier, |rows| {
                                    rows.child(group_label("Earlier", has_today))
                                })
                                .children(earlier)
                                .when(more, |rows| rows.child(self.render_show_more(cx)))
                                // Expanding fades the rows in from the header; collapsing is
                                // instant (GPUI can't animate an auto height).
                                .with_spring(
                                    "history-rows",
                                    SpringAnimation::new(spring(0.25, 1.)).to(1.0_f32).from(0.),
                                    |rows, phase| {
                                        rows.opacity(phase.clamp(0., 1.))
                                            .relative()
                                            .top(px(-4. * (1. - phase.clamp(0., 1.))))
                                    },
                                ),
                        )
                    }),
            )
            .children(self.render_problem(cx))
    }

    /// The window's only title bar, over the sidebar (Zed-style): traffic lights, project
    /// switcher, New. The app owns dragging (`app_owns_titlebar_drag`), so the terminal's top
    /// rows get clicks instead of moving the window.
    pub(crate) fn render_title_bar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let can_spawn = matches!(self.agent, Some(Ok(_)));
        let open_tip = with_shortcut("Open project", &OpenProject, window);
        let new_tip = with_shortcut("New session", &NewSession, window);
        // Buttons stop the press so it doesn't arm a window drag.
        let swallow =
            |_: &gpui::MouseDownEvent, _: &mut Window, cx: &mut gpui::App| cx.stop_propagation();
        div()
            .id("title-bar")
            .h(TITLE_BAR)
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            // Full screen hides the traffic lights: the switcher moves to the left edge.
            .map(|bar| {
                if window.is_fullscreen() {
                    bar.pl_2()
                } else {
                    bar.pl(TRAFFIC_LIGHT_PADDING)
                }
            })
            .pr_2()
            .border_b_1()
            .border_color(rgba(HAIRLINE))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.title_drag = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.title_drag = false),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, _| this.title_drag = false))
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.title_drag {
                    this.title_drag = false;
                    window.start_window_move();
                }
            }))
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    window.titlebar_double_click();
                }
            })
            .child(
                div()
                    .id("project")
                    .flex_initial()
                    .min_w_0()
                    .h(px(24.))
                    .px_2()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .cursor_pointer()
                    .hover(|style| style.bg(rgba(HOVER)))
                    .active(|style| style.bg(rgba(PRESSED)))
                    .on_mouse_down(MouseButton::Left, swallow)
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            // Native title text greys out in an inactive window.
                            .text_color(rgb(if self.window_active { INK } else { SUBTLE }))
                            .child(folder_name(&self.folder)),
                    )
                    // Reads as a switcher.
                    .child(
                        div()
                            .flex_none()
                            .font_family(FONT_FAMILY)
                            .text_size(px(8.))
                            .text_color(rgb(TERTIARY))
                            .child(CHEVRON),
                    )
                    .tooltip(move |_, cx| tooltip(open_tip.clone(), cx))
                    .on_click(cx.listener(|this, _, _, cx| this.open_project(cx))),
            )
            // Empty space between the buttons stays draggable.
            .child(div().flex_1())
            .child(
                div()
                    .id("new")
                    .flex_none()
                    .size(px(24.))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(16.))
                    .text_color(rgb(if can_spawn { SUBTLE } else { TERTIARY }))
                    .on_mouse_down(MouseButton::Left, swallow)
                    .when(can_spawn, |button| {
                        button
                            .cursor_pointer()
                            .hover(|style| style.bg(rgba(HOVER)).text_color(rgb(INK)))
                            .active(|style| style.bg(rgba(PRESSED)))
                    })
                    .child("+")
                    .tooltip(move |_, cx| tooltip(new_tip.clone(), cx))
                    .on_click(cx.listener(|this, _, window, cx| this.new_session(window, cx))),
            )
    }

    fn render_tab(
        &self,
        index: usize,
        tab: &Tab,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = self.tab_row(tab);
        let status = row.and_then(|row| row.status);
        let active = self.active == Some(tab.id);
        let shortcut = (index < SHORTCUT_TABS)
            .then(|| shortcut(&ActivateTab(index), window))
            .flatten();
        let right = match (&shortcut, row) {
            (Some(shortcut), _) if self.cmd_held => shortcut.clone(),
            (_, Some(row)) => sessions::relative_time(row.updated),
            _ => String::new(),
        };
        let id = tab.id;
        let close = div()
            .id(SharedString::from(format!("tab-{id}-close")))
            .px_1()
            .rounded_sm()
            .text_color(rgb(SUBTLE))
            .hover(|style| style.text_color(rgb(INK)).bg(rgba(HOVER)))
            .active(|style| style.bg(rgba(PRESSED)))
            .child("\u{d7}")
            // On click, not press: a press that slides off the button cancels, like any button.
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.request_close_tab(id, window, cx);
            }))
            .into_any_element();
        let card_id = SharedString::from(format!("tab-{id}"));
        let hovered = self.hovered.as_ref() == Some(&card_id);
        let hover = self.hover_listener(card_id.clone(), cx);
        let card = card(Card {
            id: card_id,
            glyph: Some(status_glyph(status)),
            title: self.tab_title(tab),
            // Inactive window: the selection steps back to body text (macOS dims key chrome).
            title_color: if active && self.window_active {
                INK
            } else {
                BODY
            },
            hover_title_color: None,
            title_editor: self.render_rename(id, active, cx),
            detail: row.and_then(|row| plain_branch(row.branch.as_deref())),
            right,
            hover_right: (!self.cmd_held).then_some(close),
            active,
            window_active: self.window_active,
            hovered,
        })
        .map_element(|card| {
            card.on_hover(hover)
                // Double-click renames the agent, as Finder renames a file.
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        this.start_rename(id, window, cx);
                    } else {
                        this.focus_tab(id, window, cx);
                    }
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        this.open_menu(MenuTarget::Tab(id), event.position, window, cx)
                    }),
                )
        });
        // New tabs arrive once (the id keeps it from replaying): fade up from 4px below on a
        // critically damped curve, so the row settles rather than stops.
        let (duration, easing) = gpui::sampled_easing(spring(0.3, 1.), 0.001);
        div()
            .relative()
            .child(card)
            .with_animation(
                SharedString::from(format!("tab-{id}-in")),
                Animation::new(duration).with_easing(easing),
                |row, delta| row.opacity(delta).top(px(4. * (1. - delta))),
            )
            .into_any_element()
    }

    fn render_history(&self, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let clicked = row.clone();
        let session = row.id.clone();
        let card_id = SharedString::from(format!("row-{}", row.id));
        let hovered = self.hovered.as_ref() == Some(&card_id);
        // The row ellipsizes; the tooltip has the whole title, id and branch.
        let details: SharedString = match plain_branch(row.branch.as_deref()) {
            Some(branch) => format!("{} \u{b7} {branch}", sessions::short_id(&row.id)),
            None => sessions::short_id(&row.id).to_string(),
        }
        .into();
        let full_title: SharedString = row.title.clone().into();
        let hover = self.hover_listener(card_id.clone(), cx);
        let card = card(Card {
            id: card_id,
            glyph: None,
            title: row.title.clone(),
            title_color: SUBTLE,
            hover_title_color: Some(BODY),
            title_editor: None,
            detail: None,
            right: sessions::relative_time(row.updated),
            hover_right: None,
            active: false,
            window_active: self.window_active,
            hovered,
        });
        let open = cx.listener(move |this, _, window, cx| this.open_session(&clicked, window, cx));
        let menu = cx.listener(move |this, event: &MouseDownEvent, window, cx| {
            this.open_menu(
                MenuTarget::Session(session.clone()),
                event.position,
                window,
                cx,
            )
        });
        card.map_element(move |card| {
            card.on_hover(hover)
                .on_mouse_down(MouseButton::Right, menu)
                .on_click(open)
                .tooltip(move |_, cx| {
                    cx.new(|_| Tooltip(full_title.clone(), Some(details.clone())))
                        .into()
                })
        })
        .into_any_element()
    }

    /// `History` as an accordion header: click toggles the list, the count stays visible.
    fn render_history_header(&self, count: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.history_open;
        div()
            .id("history-header")
            .mt_2()
            .mb_0p5()
            .px_2()
            .py_1()
            .rounded_md()
            .flex()
            .items_center()
            .gap_1p5()
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(BODY))
            .cursor_pointer()
            .hover(|style| style.bg(rgba(HOVER)).text_color(rgb(INK)))
            .active(|style| style.bg(rgba(PRESSED)))
            .child("History")
            .child(
                div()
                    .font_family(FONT_FAMILY)
                    .text_size(px(8.))
                    .text_color(rgb(TERTIARY))
                    .child(if open { CHEVRON } else { CHEVRON_RIGHT }),
            )
            .child(div().flex_1())
            .child(
                div()
                    .font_weight(FontWeight::NORMAL)
                    .child(count.to_string()),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.set_history_open(!this.history_open);
                cx.notify();
            }))
    }

    /// The name field over a tab's title while it is being renamed.
    /// The field takes the title's exact place: border and padding are cancelled by negative
    /// margins, the weight matches the title (`medium` = active tab), and the caret is a drawn
    /// bar rather than a glyph from a fallback font. Entering or leaving rename moves nothing.
    fn render_rename(
        &self,
        tab: TabId,
        medium: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let rename = self.renaming.as_ref().filter(|rename| rename.tab == tab)?;
        Some(
            div()
                .id(SharedString::from(format!("tab-{tab}-rename")))
                .track_focus(&rename.focus_handle)
                .on_key_down(cx.listener(Workspace::rename_key))
                // A click elsewhere keeps the new name (Finder), a click inside stays here.
                .on_mouse_down_out(
                    cx.listener(|this, _, window, cx| this.finish_rename(true, window, cx)),
                )
                .on_click(|_, _, cx| cx.stop_propagation())
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .px(RENAME_INSET)
                .mx(-RENAME_INSET - px(1.))
                .my(px(-1.))
                .rounded_sm()
                .border_1()
                .border_color(rgba(ACCENT_SELECTION))
                .bg(rgba(TOOLTIP_BG))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(rgb(INK))
                .when(medium, |field| field.font_weight(FontWeight::MEDIUM))
                .cursor_text()
                .child(rename.name.clone())
                .child(
                    div()
                        .flex_none()
                        .w(px(1.))
                        .h(px(15.))
                        .bg(rgb(ACCENT))
                        // Blinks like a macOS caret: solid for the first half of each cycle.
                        // The id carries the text length, so every keystroke restarts the
                        // cycle and the caret stays solid while typing. Reduce Motion keeps it
                        // solid (a repeating animation renders its start state).
                        .with_animation(
                            ("rename-caret", rename.name.len()),
                            Animation::new(CARET_BLINK).repeat().with_max_fps(4.),
                            |caret, delta| caret.opacity(if delta < 0.5 { 1. } else { 0. }),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_show_more(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("history-more")
            .mx_1()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_size(px(12.))
            .text_color(rgb(SUBTLE))
            .hover(|style| style.bg(rgba(HOVER)).text_color(rgb(BODY)))
            .active(|style| style.bg(rgba(PRESSED)))
            .child("Show more")
            .on_click(cx.listener(|this, _, _, cx| this.show_more_history(cx)))
    }

    /// The right-click menu, drawn above everything at the click point. Dismissed by a click
    /// outside, Escape, or picking an item.
    pub(crate) fn render_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let panel = div()
            .id("context-menu")
            .track_focus(&menu.focus_handle)
            .on_key_down(cx.listener(Workspace::menu_key))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_menu(window, cx)))
            .occlude()
            .min_w(px(200.))
            .p_1()
            .flex()
            .flex_col()
            .rounded_lg()
            .bg(rgba(TOOLTIP_BG))
            .border_1()
            .border_color(rgba(HAIRLINE))
            .shadow_lg()
            .font_family(UI_FONT_FAMILY)
            .text_size(px(13.))
            .text_color(rgb(BODY));
        let panel =
            panel.children(self.menu_actions(&menu.target).into_iter().map(|action| {
                let label = match action {
                    MenuAction::Open => "Open",
                    MenuAction::Rename => "Rename\u{2026}",
                    MenuAction::CopyId => "Copy Session ID",
                    MenuAction::RevealTranscript => "Reveal Transcript in Finder",
                    MenuAction::Close => "Close Tab",
                };
                div()
                    .id(label)
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|style| style.bg(rgba(ACCENT_SELECTION)).text_color(rgb(INK)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_menu_action(action, window, cx)
                    }))
            }));
        Some(
            deferred(
                anchored()
                    .position(menu.position)
                    .snap_to_window()
                    .child(panel),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    /// Tracks which row is under the pointer; its fill springs toward the new state.
    fn hover_listener(
        &self,
        id: SharedString,
        cx: &mut Context<Self>,
    ) -> impl Fn(&bool, &mut Window, &mut gpui::App) + 'static {
        cx.listener(move |this, hovered: &bool, _, cx| {
            if *hovered {
                this.hovered = Some(id.clone());
            } else if this.hovered.as_ref() == Some(&id) {
                this.hovered = None;
            } else {
                return;
            }
            cx.notify();
        })
    }

    /// Why New does nothing (with Retry), or why the last launch failed (dismissable).
    fn render_problem(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (message, action) = match (&self.agent, &self.launch_error) {
            (Some(Err(error)), _) => (error.clone(), "Retry"),
            (_, Some(error)) => (error.clone(), "Dismiss"),
            _ => return None,
        };
        Some(
            div()
                .flex_none()
                .p_3()
                .flex()
                .flex_col()
                .gap_1p5()
                .border_t_1()
                .border_color(rgba(HAIRLINE))
                .text_size(px(11.))
                .child(div().text_color(rgb(AMBER)).child(message))
                .child(
                    div()
                        .id("problem-action")
                        .self_start()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .border_1()
                        .border_color(rgba(HAIRLINE))
                        .text_color(rgb(BODY))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgba(HOVER)).text_color(rgb(INK)))
                        .active(|style| style.bg(rgba(PRESSED)))
                        .child(action)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if matches!(this.agent, Some(Err(_))) {
                                this.retry_resolve(cx);
                            } else {
                                this.launch_error = None;
                                cx.notify();
                            }
                        })),
                ),
        )
    }

    pub(crate) fn render_welcome(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let new_key = shortcut(&NewSession, window);
        let open_hint = match shortcut(&OpenProject, window) {
            Some(key) => format!("{key} opens another project"),
            None => "File \u{203a} Open Project opens another project".into(),
        };
        let message = match &self.agent {
            Some(Ok(_)) => format!("New session in {}", folder_name(&self.folder)),
            None => "Looking for agent\u{2026}".to_string(),
            Some(Err(_)) => "Agent CLI not found: look again".to_string(),
        };
        // Looking: nothing to click yet. Not found: the click looks again.
        let enabled = self.agent.is_some();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .font_family(UI_FONT_FAMILY)
            .text_size(px(13.))
            .child(div().text_color(rgb(SUBTLE)).child("No session open"))
            .child(
                div()
                    .id("welcome-new")
                    .flex()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .rounded_lg()
                    .bg(rgba(HOVER))
                    .border_1()
                    .border_color(rgba(HAIRLINE))
                    .text_color(rgb(if enabled { ACCENT } else { TERTIARY }))
                    .when(enabled, |button| {
                        button
                            .cursor_pointer()
                            .hover(|style| style.bg(rgba(ACTIVE)))
                            .active(|style| style.bg(rgba(PRESSED)))
                    })
                    .child(message)
                    .children(new_key.map(|key| div().text_color(rgb(SUBTLE)).child(key)))
                    .on_click(cx.listener(|this, _, window, cx| this.new_session(window, cx))),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(TERTIARY))
                    .child(open_hint),
            )
    }
}

/// `Current Agents 3` with an amber `1 waiting` pill when something needs the user.
fn section(label: &'static str, count: usize, waiting: usize) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .pt_4()
        .pb_1()
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(BODY))
        .child(div().flex_1().child(label))
        .when(waiting > 0, |header| {
            header.child(
                div()
                    .px_1p5()
                    .rounded_full()
                    .bg(rgba(AMBER_PILL))
                    .text_size(px(10.))
                    .text_color(rgb(AMBER))
                    .child(format!("{waiting} waiting")),
            )
        })
        .child(
            div()
                .font_weight(FontWeight::NORMAL)
                .child(count.to_string()),
        )
}

/// A subheading inside History (`~~ Today ~~`): the label sits centered between two wavy
/// rules within the list's inset, so the groups read as parts of History, not new sections.
/// `spaced` adds room above a group that follows another.
fn group_label(label: &'static str, spaced: bool) -> impl IntoElement {
    div()
        .px_2()
        .pt(if spaced { px(14.) } else { px(8.) })
        .pb_0p5()
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(10.))
        // Same height as the rules: the label's line box and the waves share one center.
        .line_height(px(12.))
        .text_color(rgb(TERTIARY))
        .child(wavy_rule())
        .child(label)
        .child(wavy_rule())
}

/// A thin sine-like squiggle across its width: quadratic arcs alternating up and down.
fn wavy_rule() -> impl IntoElement {
    const WAVELENGTH: f32 = 12.;
    const AMPLITUDE: f32 = 1.5;
    gpui::canvas(
        |_, _, _| {},
        |bounds, (), window, _| {
            let left = f32::from(bounds.left());
            let right = f32::from(bounds.right());
            let mid = f32::from(bounds.center().y);
            let half = WAVELENGTH / 2.;
            // Whole half-waves only, centered: the leftover splits between both ends, so the
            // gap beside the label is the same on either side and the label reads centered.
            let count = ((right - left) / half).floor();
            if count < 1. {
                return;
            }
            let mut x = left + ((right - left) - count * half) / 2.;
            let end = x + count * half;
            let mut path = gpui::PathBuilder::stroke(px(1.));
            path.move_to(gpui::point(px(x), px(mid)));
            let mut up = true;
            while x + half <= end + 0.01 {
                // A quadratic's peak is half its control offset.
                let lift = if up { -2. * AMPLITUDE } else { 2. * AMPLITUDE };
                path.curve_to(
                    gpui::point(px(x + half), px(mid)),
                    gpui::point(px(x + half / 2.), px(mid + lift)),
                );
                x += half;
                up = !up;
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, gpui::Hsla::from(rgb(TERTIARY)).opacity(0.45));
            }
        },
    )
    .flex_1()
    .h(px(12.))
}

/// Muted helper text.
fn hint(text: &'static str) -> impl IntoElement {
    div().px_2().py_1().text_color(rgb(SUBTLE)).child(text)
}

/// A branch worth showing: not detached and not the default branch.
fn plain_branch(branch: Option<&str>) -> Option<String> {
    branch
        .filter(|branch| !PLAIN_BRANCHES.contains(branch))
        .map(String::from)
}

/// One signal per meaning: spinner = working, pulsing amber = needs you, green dot = idle.
/// None means the pid file hasn't appeared yet.
fn status_glyph(status: Option<Status>) -> AnyElement {
    let slot = || {
        div()
            .w(px(14.))
            .flex_none()
            .flex()
            .justify_center()
            .font_family(FONT_FAMILY)
    };
    match status {
        Some(Status::Busy) | None => {
            let color = if status.is_some() { BODY } else { TERTIARY };
            // Text can't rotate: the animator picks this cycle's frame. Capped at the frame rate
            // (12.5 fps) instead of the display's, since every frame redraws the sidebar.
            slot()
                .text_color(rgb(color))
                .with_animation(
                    "spinner",
                    Animation::new(SPINNER_CYCLE)
                        .repeat()
                        .with_max_fps(SPINNER.len() as f32 / SPINNER_CYCLE.as_secs_f32()),
                    |slot, delta| {
                        let frame = (delta * SPINNER.len() as f32) as usize;
                        slot.child(SPINNER[frame.min(SPINNER.len() - 1)])
                    },
                )
                .into_any_element()
        }
        Some(Status::Waiting) => slot()
            .text_size(px(10.))
            .text_color(rgb(AMBER))
            .child(DOT)
            .with_animation(
                "waiting",
                Animation::new(Duration::from_millis(1600))
                    .repeat()
                    // Smooth enough for a slow fade; display rate would redraw the sidebar ~120x/s.
                    .with_max_fps(30.)
                    .with_easing(pulsating_between(0.35, 1.)),
                |slot, delta| slot.opacity(delta),
            )
            .into_any_element(),
        Some(Status::Idle) => slot()
            .text_size(px(8.))
            .text_color(gpui::Hsla::from(rgb(GREEN)).opacity(0.7))
            .child(DOT)
            .into_any_element(),
    }
}

struct Card {
    id: SharedString,
    glyph: Option<AnyElement>,
    title: String,
    /// Shown in place of the title while it is being renamed.
    title_editor: Option<AnyElement>,
    title_color: u32,
    /// Title color while hovered, if it changes.
    hover_title_color: Option<u32>,
    /// Second line (the branch); the row is one line without it.
    detail: Option<String>,
    /// Time, or the Cmd-N shortcut while Cmd is held.
    right: String,
    /// Shown instead of `right` while hovered (close button).
    hover_right: Option<AnyElement>,
    active: bool,
    /// The window is key; an inactive window's selection is a fainter fill.
    window_active: bool,
    hovered: bool,
}

/// A row's fill as one springable level: 0 none, `HOVER` / `ACTIVE` as a fraction of 1 (gray
/// fills), 2 the accent selection. The active row keeps its fill under the pointer. Finder-style:
/// the selection is accent while the window is key and turns gray when it isn't.
fn fill_level(active: bool, window_active: bool, hovered: bool) -> f32 {
    let gray = |color: u32| (color & 0xff) as f32 / (ACTIVE & 0xff) as f32;
    match (active, window_active, hovered) {
        (true, true, _) => 2.,
        (true, false, _) => 1.,
        (false, _, true) => gray(HOVER),
        _ => 0.,
    }
}

/// Fill color for a `fill_level`; continuous, so the spring glides gray into accent.
fn fill_color(level: f32) -> gpui::Hsla {
    let level = level.clamp(0., 2.);
    let gray = gpui::rgba(ACTIVE);
    let to = if level <= 1. {
        gpui::Rgba {
            a: gray.a * level,
            ..gray
        }
    } else {
        let accent = gpui::rgba(ACCENT_SELECTION);
        let t = level - 1.;
        let mix = |from: f32, to: f32| from + (to - from) * t;
        gpui::Rgba {
            r: mix(gray.r, accent.r),
            g: mix(gray.g, accent.g),
            b: mix(gray.b, accent.b),
            a: mix(gray.a, accent.a),
        }
    };
    to.into()
}

/// A sidebar row. Fill, title color and the time/hover swap all ride springs keyed by the row
/// id, so a fast sweep across rows retargets mid-flight from the on-screen value instead of
/// jumping. Pressing darkens instantly; feedback lives on mouse-down.
fn card(card: Card) -> gpui::SpringAnimationElement<gpui::Stateful<gpui::Div>> {
    let id = card.id.clone();
    let hover_phase = || SpringAnimation::new(hover_spring()).to(card.hovered);
    let title_color = rgb(card.title_color);
    let title = div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .when(card.active, |title| title.font_weight(FontWeight::MEDIUM))
        .child(card.title);
    let title = match (card.title_editor, card.hover_title_color) {
        (Some(editor), _) => editor,
        (None, Some(hover)) => title
            .with_spring(
                SharedString::from(format!("{id}-title")),
                hover_phase(),
                move |title, phase| {
                    title.text_color(phase.interpolate_clamped(title_color, rgb(hover)))
                },
            )
            .into_any_element(),
        (None, None) => title.text_color(title_color).into_any_element(),
    };
    // Time and hover content share one slot and crossfade, so the row never reflows. The hover
    // content is only mounted while visible, so a faded-out close button can't be clicked.
    let time = card.right;
    let hover_right = card.hover_right;
    let right = div()
        .flex_none()
        .relative()
        .flex()
        .items_center()
        .justify_end()
        .min_w(px(24.))
        .text_size(px(11.))
        .text_color(rgb(SUBTLE))
        .whitespace_nowrap()
        .with_spring(
            SharedString::from(format!("{id}-right")),
            hover_phase(),
            move |slot, phase| {
                let shown = phase.clamp(0.0..=1.).0;
                match hover_right {
                    Some(hover) => slot
                        .child(div().opacity(1. - shown).child(time))
                        .when(shown > 0.01, |slot| {
                            slot.child(div().absolute().right_0().opacity(shown).child(hover))
                        }),
                    None => slot.child(time),
                }
            },
        );
    div()
        .id(id.clone())
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .py_1p5()
        .rounded_lg()
        .cursor_pointer()
        .active(|style| style.bg(rgba(PRESSED)))
        .children(card.glyph)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(title)
                        .child(right),
                )
                .children(card.detail.map(|detail| {
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        // Ids and branches read as code: monospace, like the terminal.
                        .font_family(FONT_FAMILY)
                        .text_size(px(9.))
                        .text_color(rgb(SUBTLE))
                        .child(detail)
                })),
        )
        .with_spring(
            SharedString::from(format!("{id}-fill")),
            SpringAnimation::new(hover_spring()).to(fill_level(
                card.active,
                card.window_active,
                card.hovered,
            )),
            |row, level| row.bg(fill_color(level)),
        )
}

/// Text, plus an optional second, monospace line (ids, branches).
struct Tooltip(SharedString, Option<SharedString>);

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(360.))
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgba(TOOLTIP_BG))
            .border_1()
            .border_color(rgba(HAIRLINE))
            .font_family(UI_FONT_FAMILY)
            .text_size(px(11.))
            .text_color(rgb(BODY))
            .child(self.0.clone())
            .children(self.1.clone().map(|detail| {
                div()
                    .font_family(FONT_FAMILY)
                    .text_size(px(10.))
                    .text_color(rgb(SUBTLE))
                    .child(detail)
            }))
    }
}

fn tooltip(text: SharedString, cx: &mut gpui::App) -> gpui::AnyView {
    cx.new(|_| Tooltip(text, None)).into()
}

/// The key bound to `action` in the workspace, macOS style (`⌘T`), so labels follow the keymap.
fn shortcut(action: &dyn Action, window: &Window) -> Option<String> {
    let mut context = KeyContext::default();
    context.add("Workspace");
    let binding = window.highest_precedence_binding_for_action_in_context(action, context)?;
    let [keystroke] = binding.keystrokes() else {
        return None;
    };
    Some(key_label(keystroke.modifiers(), keystroke.key()))
}

/// `label  ⌘K`, or just the label when nothing is bound.
fn with_shortcut(label: &str, action: &dyn Action, window: &Window) -> SharedString {
    match shortcut(action, window) {
        Some(key) => format!("{label}  {key}").into(),
        None => label.to_string().into(),
    }
}

/// Modifier symbols in Apple's order (⌃⌥⇧⌘), then the key.
fn key_label(modifiers: &Modifiers, key: &str) -> String {
    let symbols = [
        (modifiers.control, '\u{2303}'),
        (modifiers.alt, '\u{2325}'),
        (modifiers.shift, '\u{21e7}'),
        (modifiers.platform, '\u{2318}'),
    ];
    let key = match key {
        "left" => "\u{2190}".to_string(),
        "right" => "\u{2192}".to_string(),
        "up" => "\u{2191}".to_string(),
        "down" => "\u{2193}".to_string(),
        "enter" => "\u{21a9}".to_string(),
        "backspace" => "\u{232b}".to_string(),
        "escape" => "\u{238b}".to_string(),
        "tab" => "\u{21e5}".to_string(),
        "space" => "Space".to_string(),
        key => key.to_uppercase(),
    };
    symbols
        .iter()
        .filter(|(held, _)| *held)
        .map(|(_, symbol)| *symbol)
        .collect::<String>()
        + &key
}

pub fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_branches_are_hidden() {
        assert_eq!(plain_branch(Some("main")), None);
        assert_eq!(plain_branch(Some("HEAD")), None);
        assert_eq!(plain_branch(None), None);
        assert_eq!(
            plain_branch(Some("fix/login")).as_deref(),
            Some("fix/login")
        );
    }

    #[test]
    fn fills_step_from_hover_to_selection() {
        let none = fill_level(false, true, false);
        let hover = fill_level(false, true, true);
        let inactive_selection = fill_level(true, false, false);
        let selection = fill_level(true, true, false);
        assert!(none < hover && hover < inactive_selection && inactive_selection < selection);
        // The selection keeps its fill under the pointer.
        assert_eq!(fill_level(true, true, true), selection);
    }

    #[test]
    fn key_labels_read_like_macos_menus() {
        let cmd = Modifiers {
            platform: true,
            ..Default::default()
        };
        let cmd_shift = Modifiers {
            platform: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(key_label(&cmd, "t"), "\u{2318}T");
        assert_eq!(key_label(&cmd_shift, "]"), "\u{21e7}\u{2318}]");
        assert_eq!(key_label(&Modifiers::default(), "left"), "\u{2190}");
    }
}
