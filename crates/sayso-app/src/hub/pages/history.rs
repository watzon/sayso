//! The history page. See the Paper board "Hub — History".
//!
//! Left: search, filters, and entries grouped by day. Right: the selected
//! entry with its audio and the trail of how the text got there.

use super::kit::{self, caps, fraunces, ui};
use super::player::{self, Playing};
use crate::hub::{Route, SettingsPage};
use crate::model::AppModel;
use gpui_kit::component::input::{InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::history::{EnhanceOutcome, HistoryEntry, InsertOutcome, TargetApp};
use sayso_i18n::t;
use sayso_store::{AppCount, HistoryQuery};
use sayso_ui::a11y::{A11yExt as _, A11yText, LiveStatus, Pressable, Role};
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::fonts::{DISPLAY, Face, LocaleFonts as _};
use sayso_ui::paper;
use sayso_ui::{ActivePaper, Colors};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

const PAGE: usize = 50;
/// Display type for the selected dictation, from the design.
const DETAIL_SIZE: f32 = 33.;
const DETAIL_LINE: f32 = 44.;
/// A transcript longer than this gets "Show full transcript".
const TRANSCRIPT_PREVIEW_CHARS: usize = 140;
/// The step of the position slider.
const SEEK_STEP_MS: i64 = 5000;

/// A message under the player. It keeps the reason, not the sentence, so it
/// changes with the language.
enum Notice {
    AudioMissing,
    Play(player::PlayError),
}

impl Notice {
    fn text(&self) -> String {
        match self {
            Notice::AudioMissing => t!("history.notice.audio_missing"),
            Notice::Play(e) => e.text(),
        }
    }
}

pub struct HistoryPage {
    model: Entity<AppModel>,
    search: Entity<InputState>,
    enhanced: bool,
    has_audio: bool,
    /// (bundle id, name)
    app: Option<(String, String)>,
    app_menu: bool,
    /// The filter field of the app picker.
    app_filter: Entity<InputState>,
    /// The row of the app picker that Enter picks.
    app_cursor: usize,
    app_scroll: ScrollHandle,
    /// The three-dot menu of the open entry.
    actions_menu: bool,
    limit: usize,
    entries: Vec<HistoryEntry>,
    matches: usize,
    selected: Option<i64>,
    seen_revision: u64,
    /// The page has no room for the full list pane (set at each render).
    narrow: bool,
    playing: Option<Playing>,
    /// (entry id, position in ms) for the paused position.
    position: (i64, u64),
    audio: Option<(String, Rc<Vec<f32>>)>,
    notice: Option<Notice>,
    confirm_delete: bool,
    busy: Option<&'static str>,
    copied: bool,
    /// Scroll position of the detail text, so the fade knows when the end shows.
    text_scroll: ScrollHandle,
    /// The scroll position of the entry list, for its scrollbar and "Back to top".
    list_scroll: ScrollHandle,
    /// True when the list is far enough down to show "Back to top".
    list_scrolled: bool,
    /// The full transcript modal is open.
    transcript_open: bool,
    modal_focus: FocusHandle,
    bar_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The locale of the text in the placeholders.
    locale: String,
    _subs: Vec<Subscription>,
}

impl HistoryPage {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("history.search_placeholder")));
        let sub = cx.subscribe_in(&search, window, |this: &mut Self, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                this.limit = PAGE;
                this.refresh(cx);
            }
        });
        let app_filter = cx.new(|cx| InputState::new(window, cx).placeholder(t!("history.app_filter_placeholder")));
        let app_sub = cx.subscribe_in(&app_filter, window, |this: &mut Self, _, ev: &InputEvent, _, cx| match ev {
            InputEvent::Change => this.reset_app_cursor(cx),
            InputEvent::PressEnter { .. } => this.pick_app(this.app_cursor, cx),
            _ => {}
        });
        let mut page = Self {
            model,
            search,
            enhanced: false,
            has_audio: false,
            app: None,
            app_menu: false,
            app_filter,
            app_cursor: 0,
            app_scroll: ScrollHandle::new(),
            actions_menu: false,
            limit: PAGE,
            entries: Vec::new(),
            matches: 0,
            selected: None,
            seen_revision: u64::MAX,
            narrow: false,
            playing: None,
            position: (0, 0),
            audio: None,
            notice: None,
            confirm_delete: false,
            busy: None,
            copied: false,
            text_scroll: ScrollHandle::new(),
            list_scroll: ScrollHandle::new(),
            list_scrolled: false,
            transcript_open: false,
            modal_focus: cx.focus_handle(),
            bar_bounds: Rc::new(Cell::new(None)),
            locale: sayso_i18n::locale(),
            _subs: vec![observe, sub, app_sub],
        };
        page.refresh(cx);
        page
    }

    fn query(&self, cx: &App) -> HistoryQuery {
        let text = self.search.read(cx).value().trim().to_string();
        HistoryQuery {
            text: (!text.is_empty()).then_some(text),
            enhanced_only: self.enhanced,
            has_audio: self.has_audio,
            app_bundle_id: self.app.as_ref().map(|a| a.0.clone()),
            limit: Some(self.limit),
            offset: 0,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let q = self.query(cx);
        let m = self.model.read(cx);
        self.entries = m.query_history(&q);
        self.matches = m.count_history(&q);
        self.seen_revision = m.history_revision;
        if !self.selected.is_some_and(|id| self.entries.iter().any(|e| e.id == id)) {
            self.selected = self.entries.first().map(|e| e.id);
            self.confirm_delete = false;
        }
        cx.notify();
    }

    fn filters_active(&self, cx: &App) -> bool {
        self.enhanced || self.has_audio || self.app.is_some() || !self.search.read(cx).value().trim().is_empty()
    }

    fn clear_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.enhanced = false;
        self.has_audio = false;
        self.app = None;
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        self.limit = PAGE;
        self.refresh(cx);
    }

    // -----------------------------------------------------------------------
    // App picker
    // -----------------------------------------------------------------------

    fn app_rows(&self, cx: &App) -> Vec<Option<AppCount>> {
        app_rows(self.model.read(cx).history_apps(), &self.app_filter.read(cx).value())
    }

    fn toggle_app_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.app_menu = !self.app_menu;
        if self.app_menu {
            self.app_filter.update(cx, |s, cx| {
                s.set_value("", window, cx);
                s.focus(window, cx);
            });
            self.reset_app_cursor(cx);
        }
        cx.notify();
    }

    /// Put the cursor on the app of the filter, or on the first row.
    fn reset_app_cursor(&mut self, cx: &mut Context<Self>) {
        let current = self.app.as_ref().map(|a| a.0.as_str());
        let rows = self.app_rows(cx);
        self.app_cursor = rows.iter().position(|r| r.as_ref().map(|a| a.bundle_id.as_str()) == current).unwrap_or(0);
        self.app_scroll.scroll_to_item(self.app_cursor);
        cx.notify();
    }

    fn move_app_cursor(&mut self, down: bool, cx: &mut Context<Self>) {
        let last = self.app_rows(cx).len().saturating_sub(1);
        self.app_cursor = if down { (self.app_cursor + 1).min(last) } else { self.app_cursor.saturating_sub(1) };
        self.app_scroll.scroll_to_item(self.app_cursor);
        cx.notify();
    }

    fn pick_app(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(pick) = self.app_rows(cx).into_iter().nth(row) else { return };
        self.app = pick.map(|a| (a.bundle_id, a.name));
        self.app_menu = false;
        self.refresh(cx);
    }

    /// The floating sheet of the App filter: a filter field, then "All apps"
    /// and each app with its icon and its number of dictations.
    fn app_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let rows = self.app_rows(cx);
        let current = self.app.as_ref().map(|a| a.0.as_str());
        let mut list = div()
            .id("app-rows")
            .role(Role::ListBox)
            .aria_label(t!("history.a11y.app_list"))
            .track_scroll(&self.app_scroll)
            .flex()
            .flex_col()
            .gap(px(1.))
            .max_h(px(298.))
            .overflow_y_scroll()
            .px(px(5.))
            .pb(px(5.));
        if rows.is_empty() {
            list = list.child(div().px(px(9.)).py(px(8.)).child(ui(t!("history.app_none"), 13., 16., FontWeight::NORMAL, c.graphite)));
        }
        for (i, row) in rows.iter().enumerate() {
            let selected = row.as_ref().map(|a| a.bundle_id.as_str()) == current;
            let (badge, name, count, row_id) = match row {
                Some(a) => (
                    AppBadge::new(a.name.clone()).icon(self.model.read(cx).icons.get(&a.bundle_id)).size(18.).into_any_element(),
                    a.name.clone(),
                    a.count,
                    SharedString::from(format!("app-row-{}", a.bundle_id)),
                ),
                None => (
                    div().flex().flex_none().items_center().justify_center().size(px(18.)).child(icon(Icon::History, 14., c.graphite)).into_any_element(),
                    t!("history.all_apps"),
                    self.model.read(cx).history_total,
                    SharedString::from("app-row-all"),
                ),
            };
            list = list.child(
                // The Return key picks a row from the filter field, so the rows
                // are not tab stops.
                Pressable::new(row_id)
                    .a11y_role(Role::ListBoxOption)
                    .label(t!("history.a11y.app_option", app = name, count = sayso_core::stats::format_count(count)))
                    .aria_selected(selected)
                    .tab_stop(false)
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(9.))
                    .h(px(32.))
                    .px(px(9.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(i == self.app_cursor, |d| d.bg(c.deboss))
                    .hover(|s| s.bg(c.deboss))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_app(i, cx)))
                    .child(badge)
                    .child(kit::plain(name, 13., 16., if selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }, c.ink).flex_1().min_w_0().truncate())
                    .when(selected, |d| d.child(icon(Icon::Check, 12., c.ink)))
                    .child(kit::plain_mono(sayso_core::stats::format_count(count), 12., c.graphite).flex_none()),
            );
        }
        let sheet = div()
            .id("app-picker")
            .flex()
            .flex_col()
            .w(px(272.))
            .rounded(px(12.))
            .bg(c.sheet_raised)
            .border_1()
            .border_color(c.rule)
            .shadow(paper::floating(&c))
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.app_menu = false;
                cx.notify();
            }))
            // Capture, so the arrows move the cursor before the field sees them.
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                cx.stop_propagation();
                this.move_app_cursor(false, cx);
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                cx.stop_propagation();
                this.move_app_cursor(true, cx);
            }))
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.app_menu = false;
                    cx.notify();
                }
            }))
            .child(div().p(px(6.)).child(kit::search_well_named(&self.app_filter, Some(t!("history.app_filter_placeholder").into()), 32., None, cx)))
            .child(list);
        deferred(anchored().snap_to_window_with_margin(px(8.)).child(div().mt(px(6.)).child(sheet))).with_priority(2)
    }

    fn select(&mut self, id: i64, cx: &mut Context<Self>) {
        if self.selected != Some(id) {
            self.stop_audio();
            self.selected = Some(id);
            self.confirm_delete = false;
            self.actions_menu = false;
            self.notice = None;
            self.copied = false;
            self.transcript_open = false;
            self.text_scroll.set_offset(point(px(0.), px(0.)));
        }
        cx.notify();
    }

    fn current(&self) -> Option<&HistoryEntry> {
        let id = self.selected?;
        self.entries.iter().find(|e| e.id == id)
    }

    // -----------------------------------------------------------------------
    // Audio
    // -----------------------------------------------------------------------

    fn samples(&mut self, file: &str, cx: &App) -> Option<Rc<Vec<f32>>> {
        if let Some((f, s)) = &self.audio
            && f == file {
                return Some(s.clone());
            }
        let samples = Rc::new(self.model.read(cx).load_audio(file)?);
        self.audio = Some((file.to_string(), samples.clone()));
        Some(samples)
    }

    fn stop_audio(&mut self) {
        if let Some(p) = self.playing.take() {
            let entry = p.entry;
            let pos = p.stop();
            self.position = (entry, pos);
        }
    }

    fn position_of(&self, id: i64) -> u64 {
        match &self.playing {
            Some(p) if p.entry == id => p.position_ms(),
            _ if self.position.0 == id => self.position.1,
            _ => 0,
        }
    }

    fn toggle_play(&mut self, entry: &HistoryEntry, cx: &mut Context<Self>) {
        if self.playing.as_ref().is_some_and(|p| p.entry == entry.id) {
            self.stop_audio();
            cx.notify();
            return;
        }
        self.stop_audio();
        let Some(file) = entry.audio_file.clone() else { return };
        let Some(samples) = self.samples(&file, cx) else {
            self.notice = Some(Notice::AudioMissing);
            cx.notify();
            return;
        };
        let total = player::length_ms(&samples);
        let mut from = self.position_of(entry.id);
        if from + 200 >= total {
            from = 0;
        }
        self.play_from(entry.id, &samples, from, cx);
    }

    fn play_from(&mut self, id: i64, samples: &[f32], from: u64, cx: &mut Context<Self>) {
        match Playing::start(id, samples, from) {
            Ok(p) => {
                self.playing = Some(p);
                self.notice = None;
                cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(Duration::from_millis(50)).await;
                        let keep = this.update(cx, |page, cx| page.tick(cx)).unwrap_or(false);
                        if !keep {
                            break;
                        }
                    }
                })
                .detach();
            }
            Err(e) => self.notice = Some(Notice::Play(e)),
        }
        cx.notify();
    }

    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(p) = self.playing.as_mut() else { return false };
        if p.finished() {
            let entry = p.entry;
            self.playing = None;
            self.position = (entry, 0);
            cx.notify();
            return false;
        }
        cx.notify();
        true
    }

    fn seek(&mut self, entry: &HistoryEntry, x: Pixels, cx: &mut Context<Self>) {
        let Some(b) = self.bar_bounds.get() else { return };
        let Some(file) = entry.audio_file.clone() else { return };
        let Some(samples) = self.samples(&file, cx) else { return };
        let total = player::length_ms(&samples);
        let f = ((x - b.origin.x).as_f32() / b.size.width.as_f32()).clamp(0.0, 1.0);
        self.seek_to(entry, &samples, (f * total as f32) as u64, cx);
    }

    /// Move the position by `delta_ms`, for the keyboard and assistive clients.
    fn seek_by(&mut self, entry: &HistoryEntry, delta_ms: i64, cx: &mut Context<Self>) {
        let Some(file) = entry.audio_file.clone() else { return };
        let Some(samples) = self.samples(&file, cx) else { return };
        let total = player::length_ms(&samples);
        let pos = (self.position_of(entry.id) as i64 + delta_ms).clamp(0, total as i64) as u64;
        self.seek_to(entry, &samples, pos, cx);
    }

    fn seek_to(&mut self, entry: &HistoryEntry, samples: &[f32], pos: u64, cx: &mut Context<Self>) {
        let was_playing = self.playing.is_some();
        self.stop_audio();
        self.position = (entry.id, pos);
        if was_playing {
            self.play_from(entry.id, samples, pos, cx);
        }
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // List pane
    // -----------------------------------------------------------------------

    /// Copy text and show "Copied" for a moment.
    fn copy_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.model.read(cx).copy_text(text);
        self.copied = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            let _ = this.update(cx, |p, cx| {
                p.copied = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// True when the list is about half a screen down.
    fn list_is_scrolled(&self) -> bool {
        self.list_scroll.offset().y < px(-300.)
    }

    fn list_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        self.list_scrolled = self.list_is_scrolled();
        let total = self.model.read(cx).history_total;
        let filtered = self.filters_active(cx);
        let count = if filtered {
            t!("history.count_filtered", shown = sayso_core::stats::format_count(self.matches), total = sayso_core::stats::format_count(total))
        } else {
            t!("history.count", count = total, formatted = sayso_core::stats::format_count(total))
        };

        // The rows are direct children of the scroll area, so a row can be
        // scrolled into view by its child index. A day heading takes one index.
        let days: Vec<chrono::NaiveDate> = self.entries.iter().map(|e| e.created_at.with_timezone(&chrono::Local).date_naive()).collect();
        let mut child_ix = Vec::with_capacity(days.len());
        let mut n = 0;
        for (i, day) in days.iter().enumerate() {
            if i == 0 || days[i - 1] != *day {
                n += 1;
            }
            child_ix.push(n);
            n += 1;
        }
        let set_size = self.matches.max(self.entries.len());
        let mut list = div()
            .id("entries")
            .role(Role::List)
            .aria_label(t!("history.a11y.list"))
            .size_full()
            .flex()
            .flex_col()
            .gap(px(4.))
            .overflow_y_scroll()
            .track_scroll(&self.list_scroll)
            .on_scroll_wheel(cx.listener(|_, _, _, cx| {
                // The handle moves after this event, so read it on the next turn.
                let page = cx.entity();
                cx.defer(move |cx| {
                    page.update(cx, |this, cx| {
                        if this.list_is_scrolled() != this.list_scrolled {
                            cx.notify();
                        }
                    })
                });
            }))
            .px(px(20.))
            .pb(px(20.));
        for (i, e) in self.entries.iter().enumerate() {
            let day = days[i];
            if i == 0 || days[i - 1] != day {
                list = list.child(
                    div().flex_none().px(px(10.)).pb(px(6.)).pt(px(if i == 0 { 6. } else { 14. })).child(caps(kit::day_label(day), 12., c.graphite)),
                );
            }
            let neighbours = (i.checked_sub(1).map(|p| child_ix[p]), child_ix.get(i + 1).copied());
            list = list.child(self.entry_row(e, (i, set_size), neighbours, cx));
        }
        if self.entries.len() < self.matches {
            list = list.child(
                div().flex().flex_none().justify_center().py(px(12.)).child(
                    Button::new("more", t!("history.show_more", number = (self.matches - self.entries.len()).min(PAGE)))
                        .small()
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.limit += PAGE;
                            this.refresh(cx);
                        })),
                ),
            );
        }
        if self.entries.is_empty() && filtered {
            let clear = Button::new("clear", t!("history.clear_filters")).small().on_click(cx.listener(|this, _, w, cx| this.clear_filters(w, cx)));
            list = list.child(
                kit::empty_state(Icon::Search, &t!("history.no_match_title"), &t!("history.no_match_body"), Some(clear.into_any_element()), cx).flex_none(),
            );
        }

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(if self.narrow { 300. } else { 392. }))
            .h_full()
            .gap(px(18.))
            .pt(px(40.))
            .pl(px(28.))
            .pr(px(20.))
            .border_r_1()
            .border_color(c.rule)
            .child(
                div()
                    .flex()
                    .items_end()
                    .justify_between()
                    .px(px(4.))
                    .child(sayso_ui::text::title(t!("history.title"), 32., &c).line_height(px(40.)))
                    // The count changes with the filters: a screen reader reads it.
                    .child(LiveStatus::new("history-count", count.clone()).pb(px(6.)).child(kit::plain(count, 13., 16., FontWeight::NORMAL, c.graphite))),
            )
            .child(kit::search_well_named(&self.search, Some(t!("history.search_placeholder").into()), 38., Some(kit::mini_key(if cfg!(target_os = "macos") { "⌘F" } else { "Ctrl F" }, cx).into_any_element()), cx))
            .child(self.filters(cx))
            // The scroll area clips to its bounds. It reaches out to the pane edges
            // (with matching padding, so rows stay put) to leave room for the
            // selected row's soft shadow, which the design lets spread past the row.
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .mx(px(-20.))
                    .child(list)
                    .vertical_scrollbar(&self.list_scroll)
                    .when(self.list_scrolled, |d| {
                        d.child(
                            div().absolute().bottom(px(16.)).left_0().right_0().flex().justify_center().child(
                                div().rounded(px(9.)).shadow(sayso_ui::paper::floating(&c)).child(
                                    Button::new("back-to-top", t!("history.back_to_top")).small().on_click(cx.listener(|this, _, _, cx| {
                                        this.list_scroll.set_offset(point(px(0.), px(0.)));
                                        this.list_scrolled = false;
                                        cx.notify();
                                    })),
                                ),
                            ),
                        )
                    }),
            )
    }

    fn filters(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let all = !self.enhanced && !self.has_audio && self.app.is_none();
        let app_label = self.app.as_ref().map(|a| a.1.clone()).unwrap_or_else(|| t!("history.filter_app"));
        let mut row = div()
            .id("history-filters")
            .role(Role::Group)
            .aria_label(t!("history.a11y.filters"))
            .flex()
            .flex_none()
            .flex_wrap()
            .gap(px(6.))
            .child(Chip::new("f-all", t!("history.filter_all"), all).role(ChipRole::Filter).on_click(cx.listener(|this, _, _, cx| {
                this.enhanced = false;
                this.has_audio = false;
                this.app = None;
                this.refresh(cx);
            })))
            .child(Chip::new("f-enh", t!("history.filter_enhanced"), self.enhanced).role(ChipRole::Filter).on_click(cx.listener(|this, _, _, cx| {
                this.enhanced = !this.enhanced;
                this.refresh(cx);
            })))
            .child(Chip::new("f-audio", t!("history.filter_audio"), self.has_audio).role(ChipRole::Filter).on_click(cx.listener(|this, _, _, cx| {
                this.has_audio = !this.has_audio;
                this.refresh(cx);
            })));
        let chip = Chip::new("f-app", app_label.clone(), self.app.is_some())
            .role(ChipRole::Menu { open: self.app_menu })
            .a11y_label(t!("history.a11y.app_filter", app = app_label))
            .trailing(Icon::ChevronDown)
            .on_click(cx.listener(|this, _, window, cx| this.toggle_app_menu(window, cx)));
        let mut app = div().relative().child(chip);
        if self.app_menu {
            app = app.child(div().absolute().top_full().left_0().child(self.app_picker(cx)));
        }
        row = row.child(app);
        row
    }

    /// `place` is the row and the size of the list. `neighbours` are the child
    /// indexes of the rows above and below, for the arrow keys.
    fn entry_row(&self, e: &HistoryEntry, place: (usize, usize), neighbours: (Option<usize>, Option<usize>), cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let selected = self.selected == Some(e.id);
        let app = e.app.as_ref().map(|a| a.name.clone()).unwrap_or_else(|| t!("history.unknown_app"));
        let app_icon = e.app.as_ref().and_then(|a| self.model.read(cx).icons.get(&a.bundle_id));
        let site_icon = self.model.read(cx).site_icon(e);
        let id = e.id;
        let time = kit::clock(e.created_at);
        let scroll = self.list_scroll.clone();
        // A button, not a ListItem: macOS shows a ListItem as a group, and an
        // assistive client does not press a group.
        Pressable::new(("entry", e.id as u64))
            .label(t!("history.a11y.entry", app = app, time = time))
            .aria_selected(selected)
            .aria_position_in_set(place.0 + 1)
            .aria_size_of_set(place.1)
            // Focus does not scroll a row into view by itself, so the arrows do.
            .on_key_down(move |ev, w, cx| {
                let target = match ev.keystroke.key.as_str() {
                    "down" => neighbours.1,
                    "up" => neighbours.0,
                    _ => return,
                };
                let Some(ix) = target else { return };
                cx.stop_propagation();
                scroll.scroll_to_item(ix);
                if ev.keystroke.key == "down" { w.focus_next(cx) } else { w.focus_prev(cx) }
            })
            .flex()
            .flex_none()
            .gap(px(12.))
            .p(px(12.))
            .rounded(px(12.))
            .cursor_pointer()
            .when(selected, |d| {
                d.bg(c.sheet_raised).shadows(vec![
                    BoxShadow::new(px(0.), px(1.), c.highlight(1.0)).inset(),
                    BoxShadow::new(px(0.), px(1.), c.shadow(0.14)).blur_radius(px(2.)),
                    BoxShadow::new(px(0.), px(6.), c.shadow(0.10)).blur_radius(px(16.)),
                ])
            })
            .when(!selected, |d| d.hover(|s| s.bg(c.deboss.opacity(0.4))))
            .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
            .child(AppBadge::new(app.clone()).icon(app_icon).site(site_icon))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(kit::plain(app, 13., 16., FontWeight::SEMIBOLD, c.ink))
                            .child(kit::plain_mono(kit::clock(e.created_at), 12., c.graphite)),
                    )
                    .child(fraunces(e.final_text.replace('\n', " "), 14., 20., if selected { c.ink } else { c.graphite }).line_clamp(3).text_ellipsis()),
            )
    }

    // -----------------------------------------------------------------------
    // Detail pane
    // -----------------------------------------------------------------------

    fn detail_pane(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let Some(e) = self.current().cloned() else {
            return div().flex_1().into_any_element();
        };
        let app = e.app.as_ref().map(|a| a.name.clone()).unwrap_or_else(|| t!("history.unknown_app"));
        let app_icon = e.app.as_ref().and_then(|a| self.model.read(cx).icons.get(&a.bundle_id));
        let site_icon = self.model.read(cx).site_icon(&e);
        let (can_enhance, engine) = {
            let m = self.model.read(cx);
            let style = m.active_style();
            (m.provider_for(&style).is_some_and(|p| !p.needs_model() && style.run_plan(p) != sayso_core::style::RunPlan::Nothing), m.model_ready())
        };

        // The actions of the entry are in the three-dot menu. A delete asks
        // first, in the same corner.
        let actions = if self.confirm_delete {
            let id = e.id;
            let question = t!("history.confirm_delete");
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.))
                // The question appears after a menu pick, so a screen reader says it at once.
                .child(div().id("confirm-delete").role(Role::Status).a11y_live(question.clone(), true).child(kit::plain(question, 13., 16., FontWeight::MEDIUM, c.ink)))
                .child(Button::new("del-yes", t!("history.delete")).a11y_label(t!("history.a11y.delete_entry", time = kit::clock(e.created_at))).small().danger().on_click(cx.listener(move |this, _, _, cx| {
                    this.stop_audio();
                    this.confirm_delete = false;
                    this.model.update(cx, |m, cx| m.delete_entry(id, cx));
                })))
                .child(Button::new("del-no", t!("history.cancel")).small().ghost().on_click(cx.listener(|this, _, _, cx| {
                    this.confirm_delete = false;
                    cx.notify();
                })))
        } else {
            let status = match (self.busy, self.copied) {
                (Some("enhance"), _) => Some(t!("history.status_enhancing")),
                (Some("retranscribe"), _) => Some(t!("history.status_transcribing")),
                (_, true) => Some(t!("history.status_copied")),
                _ => None,
            };
            let open = self.actions_menu;
            let mut more = div().relative().child(kit::icon_button("entry-more", Icon::More, 16., cx).label(t!("history.a11y.more_actions")).aria_expanded(open).size(px(30.)).when(open, |d| d.bg(c.deboss)).on_click(cx.listener(|this, _, _, cx| {
                this.actions_menu = !this.actions_menu;
                cx.notify();
            })));
            if self.actions_menu {
                let mut items = vec![kit::MenuAction::new(Icon::Copy, t!("history.menu.copy")), kit::MenuAction::new(Icon::Code, t!("history.menu.copy_metadata"))];
                let mut picks: Vec<&'static str> = vec!["copy", "metadata"];
                if can_enhance && self.busy.is_none() {
                    // Says what a pick does: a second pass if a style already ran.
                    let again = matches!(e.enhance, EnhanceOutcome::Applied { .. });
                    items.push(kit::MenuAction::new(Icon::Sparkle, if again { t!("history.menu.re_enhance") } else { t!("history.menu.enhance") }));
                    picks.push("enhance");
                }
                if e.audio_file.is_some() && engine && self.busy.is_none() {
                    items.push(kit::MenuAction::new(Icon::Undo, t!("history.menu.transcribe_again")));
                    picks.push("retranscribe");
                }
                items.push(kit::MenuAction::new(Icon::Trash, t!("history.delete")).danger());
                picks.push("delete");
                let (pick_page, close_page) = (cx.entity(), cx.entity());
                let entry = e.clone();
                // The menu opens at its parent's left edge; a parent as wide as
                // the menu lines its right edge up with the button.
                more = more.child(div().absolute().top_full().right_0().w(px(184.)).child(kit::action_menu(
                    "entry-actions",
                    items,
                    move |i, _, cx| {
                        let pick = picks.get(i).copied();
                        let entry = entry.clone();
                        pick_page.update(cx, |this, cx| {
                            this.actions_menu = false;
                            match pick {
                                Some("copy") => this.copy_text(&entry.final_text, cx),
                                Some("metadata") => this.copy_text(&entry.metadata_json(), cx),
                                Some("enhance") => {
                                    this.busy = Some("enhance");
                                    this.model.update(cx, |m, cx| m.enhance_entry(entry, cx));
                                }
                                Some("retranscribe") => {
                                    this.busy = Some("retranscribe");
                                    this.model.update(cx, |m, cx| m.retranscribe_entry(entry, cx));
                                }
                                Some("delete") => this.confirm_delete = true,
                                _ => {}
                            }
                            cx.notify();
                        });
                    },
                    move |_, cx| {
                        close_page.update(cx, |this, cx| {
                            this.actions_menu = false;
                            cx.notify();
                        })
                    },
                    cx,
                )));
            }
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(10.))
                // Busy and copied: a screen reader reads each change.
                .children(status.map(|text| div().id("entry-status").role(Role::Status).a11y_live(text.clone(), false).child(kit::plain(text, 13., 16., FontWeight::MEDIUM, c.graphite))))
                .child(more)
        };

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(px(16.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .min_w_0()
                    .child(AppBadge::new(app.clone()).icon(app_icon).site(site_icon).size(24.))
                    .child(ui(app.clone(), 14., 18., FontWeight::SEMIBOLD, c.ink).truncate())
                    .child(
                        ui(
                            t!("history.time_and_duration", time = kit::clock(e.created_at), duration = kit::duration_words(e.duration_ms)),
                            14.,
                            18.,
                            FontWeight::NORMAL,
                            c.graphite,
                        )
                        .flex_none(),
                    ),
            )
            .child(actions);

        let side = if self.narrow { 28. } else { 48. };
        // Two parts. The top part (the text and the player) does not scroll:
        // only the text scrolls, in its box. The steps under it scroll on
        // their own. On a short window the steps give up height first, down
        // to 120, and then the text box, down to 1.5 lines.
        let content = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(px(28.))
            .pt(px(8.))
            .px(px(side))
            .child(self.final_text(&e.final_text, &c))
            .child(div().flex_none().child(self.player(&e, cx)))
            // The scroll area clips to its bounds. It is 4 wider on each side
            // (with matching padding, so the steps stay put) to leave room for
            // the halo of the last dot.
            .child(
                div().flex().flex_col().flex_1().min_h(px(120.)).border_t_1().border_color(c.rule).child(
                    div()
                        .id("history-trail")
                        .flex_1()
                        .min_h_0()
                        .mx(px(-4.))
                        .overflow_y_scroll()
                        .child(div().px(px(4.)).pt(px(20.)).pb(px(32.)).child(self.trail(&e, cx))),
                ),
            );
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .child(div().flex_none().pt(px(40.)).pb(px(20.)).px(px(side)).child(header))
            .child(content)
            .into_any_element()
    }

    /// The dictation in display type, at most 3.5 lines tall. Longer text
    /// scrolls inside the box, and a fade marks that more follows.
    fn final_text(&self, text: &str, c: &Colors) -> AnyElement {
        let mut body = div().flex().flex_col().gap(px((DETAIL_LINE * 0.5).round()));
        for (i, paragraph) in paragraphs(text).into_iter().enumerate() {
            body = body.child(div().child(A11yText::new(paragraph.to_string()).with_id(("paragraph", i))));
        }
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_shrink(1.)
            .min_h(px(DETAIL_LINE * 1.5))
            .max_h(px(DETAIL_LINE * 3.5))
            .child(
                div()
                    .id("final-text")
                    .role(Role::Group)
                    .aria_label(t!("history.a11y.final_text"))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.text_scroll)
                    .font_family(DISPLAY).locale_fonts(Face::Serif)
                    .text_size(px(DETAIL_SIZE))
                    .line_height(px(DETAIL_LINE))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c.ink)
                    .child(body),
            )
            // Decided at paint time: the scroll handle has this frame's layout
            // by then. Read during render, it would still hold the last entry's.
            .child({
                let scroll = self.text_scroll.clone();
                let sheet = c.sheet;
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let max = scroll.max_offset().y;
                        if max > px(1.) && scroll.offset().y > -(max - px(1.)) {
                            window.paint_quad(fill(
                                bounds,
                                linear_gradient(180., linear_color_stop(sheet.opacity(0.), 0.), linear_color_stop(sheet, 1.)),
                            ));
                        }
                    },
                )
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(DETAIL_LINE * 0.9))
            })
            .into_any_element()
    }

    /// The full transcript over the page. Esc, the backdrop, or Close dismiss it.
    fn transcript_modal(&self, e: &HistoryEntry, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let model_name = self.model.read(cx).model_name(&e.model);
        let close = cx.listener(|this, _, _, cx| {
            this.transcript_open = false;
            cx.notify();
        });
        let transcript = e.transcript.clone();
        div()
            .id("transcript-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(c.shadow(0.28))
            .occlude()
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.transcript_open = false;
                    cx.notify();
                }
            }))
            // A click outside the sheet closes it. The Close button is the keyboard way.
            .child(Pressable::new("transcript-scrim").label(t!("history.transcript.close")).tab_stop(false).absolute().inset_0().on_click(close))
            .child(
                div()
                    .id("transcript-modal")
                    .role(Role::Dialog)
                    .aria_label(t!("history.transcript.title"))
                    // The focus is on the dialog, so a screen reader reads its name.
                    .track_focus(&self.modal_focus)
                    .occlude()
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .w(px(560.))
                    .max_h(relative(0.8))
                    .p(px(28.))
                    .rounded(px(16.))
                    .bg(c.sheet_raised)
                    .shadow(paper::floating(&c))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(caps(t!("history.transcript.title"), 12., c.graphite))
                            .child(ui(t!("history.transcript.detail", model = model_name), 13., 18., FontWeight::NORMAL, c.graphite)),
                    )
                    .child(
                        div()
                            .id("transcript-full")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(fraunces(transcript.clone(), 17., 26., c.ink).italic().font_weight(FontWeight::NORMAL)),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(Button::new("transcript-copy", t!("history.menu.copy")).on_click(cx.listener(move |this, _, _, cx| {
                                this.model.read(cx).copy_text(&transcript);
                            })))
                            .child(Button::new("transcript-close", t!("history.transcript.close")).primary().on_click(cx.listener(|this, _, _, cx| {
                                this.transcript_open = false;
                                cx.notify();
                            }))),
                    ),
            )
            .into_any_element()
    }

    fn player(&mut self, e: &HistoryEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let has_audio = e.audio_file.is_some();
        let playing = self.playing.as_ref().is_some_and(|p| p.entry == e.id);
        let total = e.duration_ms.max(1);
        let pos = self.position_of(e.id).min(total);
        let progress = if has_audio { pos as f32 / total as f32 } else { 0.0 };

        let e1 = e.clone();
        let button = Pressable::new("play")
            .label(if playing { t!("history.a11y.pause") } else { t!("history.a11y.play") })
            .disabled(!has_audio)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(44.))
            .rounded_full()
            .bg(c.ink_fill)
            .shadows(vec![
                BoxShadow::new(px(0.), px(1.), white().opacity(0.14)).inset(),
                BoxShadow::new(px(0.), px(-2.), black().opacity(0.45)).blur_radius(px(4.)).inset(),
                BoxShadow::new(px(0.), px(2.), c.shadow(0.28)).blur_radius(px(4.)),
            ])
            .child(sayso_ui::components::icon(if playing { Icon::Pause } else { Icon::Play }, 16., c.on_ink))
            .map(|d| {
                if has_audio {
                    d.cursor_pointer()
                        .active(|s| s.shadow(paper::ink_pressed(&c)))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_play(&e1, cx)))
                } else {
                    d.opacity(0.35)
                }
            });

        let e2 = e.clone();
        let bounds = self.bar_bounds.clone();
        let rest = if c.is_dark() { c.pencil.opacity(0.55) } else { c.pencil.opacity(0.6) };
        let (total_s, pos_s) = (total as f64 / 1000., pos as f64 / 1000.);
        let (e3, e4, e5) = (e.clone(), e.clone(), e.clone());
        let (page_up, page_down, page_keys) = (cx.entity(), cx.entity(), cx.entity());
        // The position bar is a slider: Left and Right, or Increment and
        // Decrement, move it by 5 seconds.
        let wave = Pressable::new("wave")
            .a11y_role(Role::Slider)
            .label(t!("history.a11y.position"))
            .disabled(!has_audio)
            .aria_value(t!("history.a11y.position_value", pos = kit::duration(pos), total = kit::duration(total)))
            .aria_numeric_value(pos_s)
            .aria_min_numeric_value(0.)
            .aria_max_numeric_value(total_s)
            .aria_numeric_value_step(SEEK_STEP_MS as f64 / 1000.)
            .on_a11y_action(AccessibleAction::Increment, move |_, _, cx| page_up.update(cx, |this, cx| this.seek_by(&e3, SEEK_STEP_MS, cx)))
            .on_a11y_action(AccessibleAction::Decrement, move |_, _, cx| page_down.update(cx, |this, cx| this.seek_by(&e4, -SEEK_STEP_MS, cx)))
            .on_key_down(move |ev, _, cx| {
                let delta = match ev.keystroke.key.as_str() {
                    "right" | "up" => SEEK_STEP_MS,
                    "left" | "down" => -SEEK_STEP_MS,
                    _ => return,
                };
                cx.stop_propagation();
                page_keys.update(cx, |this, cx| this.seek_by(&e5, delta, cx));
            })
            .flex_1()
            .min_w_0()
            .h(px(44.))
            .when(has_audio, |d| {
                d.cursor_pointer().on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                    // Return and Space on the bar do nothing: there is no pointer position.
                    if !ev.is_keyboard() {
                        this.seek(&e2, ev.position().x, cx)
                    }
                }))
            })
            .child(bars(e.waveform.clone(), progress, if has_audio { c.ink } else { rest }, rest, bounds).size_full());

        let time = if has_audio { t!("history.player.position", pos = kit::duration(pos), total = kit::duration(total)) } else { kit::duration(total) };
        let tray = div()
            .flex()
            .items_center()
            .gap(px(16.))
            .pl(px(14.))
            .pr(px(18.))
            .py(px(14.))
            .rounded(px(16.))
            .bg(c.deboss)
            .shadow(vec![
                BoxShadow::new(px(0.), px(2.), c.shadow(0.18)).blur_radius(px(4.)).inset(),
                BoxShadow::new(px(0.), px(-1.), c.highlight(0.75)).inset(),
            ])
            .child(button)
            .child(wave)
            .child(kit::plain_mono(time, 12., c.graphite).flex_none());

        let m = self.model.read(cx);
        let note = if let Some(n) = &self.notice {
            n.text()
        } else if !has_audio {
            if m.config.history.save_audio { t!("history.audio.expired") } else { t!("history.audio.not_saved") }
        } else if let Some(at) = m.audio_expiry(e) {
            t!("history.audio.kept_until", date = sayso_i18n::dates::day_month(&at))
        } else {
            t!("history.audio.kept")
        };
        let model = self.model.clone();
        let failed = self.notice.is_some();
        div().flex().flex_col().flex_none().gap(px(10.)).child(tray).child(
            Pressable::new("audio-note")
                .a11y_role(Role::Link)
                .label(note.clone())
                // An error under the player is read when it appears.
                .when(failed, |p| p.a11y_live(note.clone(), true))
                .pl(px(4.))
                .cursor_pointer()
                .on_click(move |_, _, cx| model.update(cx, |m, cx| m.navigate(Route::Settings(SettingsPage::HistoryPrivacy), cx)))
                .child(kit::plain(note, 13., 16., FontWeight::NORMAL, if failed { c.danger } else { c.graphite })),
        )
    }

    fn trail(&self, e: &HistoryEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let mut steps: Vec<(String, Option<AnyElement>)> = Vec::new();

        let long = e.transcript.chars().count() > TRANSCRIPT_PREVIEW_CHARS || e.transcript.contains('\n');
        let preview = fraunces(e.transcript.replace('\n', " "), 15., 21., c.graphite).italic().line_clamp(2).text_ellipsis();
        steps.push((
            if e.transcribe_ms > 0 {
                t!("history.trail.transcript_ms", model = m.model_name(&e.model), ms = e.transcribe_ms)
            } else {
                t!("history.trail.transcript", model = m.model_name(&e.model))
            },
            Some(if long {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(preview)
                    .child(
                        Pressable::new("show-transcript")
                            .label(t!("history.trail.show_transcript"))
                            .cursor_pointer()
                            .child(kit::plain(t!("history.trail.show_transcript"), 13., 16., FontWeight::SEMIBOLD, c.accent))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.transcript_open = true;
                                this.modal_focus.focus(window, cx);
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            } else {
                preview.into_any_element()
            }),
        ));

        let n: usize = e.replacements.iter().map(|r| r.count.max(1)).sum();
        if e.replacements.is_empty() {
            steps.push((t!("history.trail.dictionary_none"), None));
        } else {
            let mut body = div().flex().flex_col().gap(px(2.));
            // Three lines at most, so the trail keeps its height.
            for (i, r) in e.replacements.iter().take(3).enumerate() {
                let times = if r.count > 1 { format!("  ×{}", r.count) } else { String::new() };
                // The same replacement can be in the list twice: the row id keeps its text node unique.
                body = body.child(
                    div().id(("replacement", i)).child(ui(format!("“{}” → “{}”{times}", r.from, r.to.replace("\\n", "↵")), 14., 18., FontWeight::NORMAL, c.graphite)),
                );
            }
            if e.replacements.len() > 3 {
                body = body.child(ui(t!("history.trail.more", more = e.replacements.len() - 3), 14., 18., FontWeight::NORMAL, c.pencil));
            }
            steps.push((t!("history.trail.dictionary", count = n), Some(body.into_any_element())));
        }

        let style_name = m.style_name(&e.style_id);
        let style_desc = m.styles.get(&e.style_id).map(|s| s.display_description()).unwrap_or_default();
        match &e.enhance {
            EnhanceOutcome::Applied { provider_id, model, elapsed_ms } => steps.push((
                t!(
                    "history.trail.style_applied",
                    style = style_name,
                    provider = m.provider_name(provider_id),
                    model = super::ext::short_model(model),
                    seconds = sayso_i18n::numbers::decimal(*elapsed_ms as f64 / 1000.0, 2)
                ),
                (!style_desc.is_empty()).then(|| ui(style_desc.clone(), 14., 18., FontWeight::NORMAL, c.graphite).into_any_element()),
            )),
            EnhanceOutcome::Failed { provider_id, reason } => steps.push((
                t!("history.trail.style_failed", style = style_name, provider = m.provider_name(provider_id)),
                Some(
                    ui(t!("history.trail.style_failed_detail", reason = reason), 14., 18., FontWeight::NORMAL, c.danger)
                        .into_any_element(),
                ),
            )),
            EnhanceOutcome::NotUsed => steps.push((
                t!("history.trail.style_unused", style = style_name),
                Some(
                    ui(
                        if e.style_id == sayso_core::style::RAW_STYLE_ID { t!("history.trail.raw_no_ai") } else { t!("history.trail.ai_not_run") },
                        14.,
                        18.,
                        FontWeight::NORMAL,
                        c.graphite,
                    )
                    .into_any_element(),
                ),
            )),
        }

        // With a site: "chatgpt.com in Aside".
        let app = match &e.app {
            Some(TargetApp { name, site: Some(site), .. }) => t!("history.trail.site_in_app", site = site, app = name),
            Some(a) => a.name.clone(),
            None => t!("history.trail.the_app"),
        };
        let (last, last_color) = match &e.insert {
            InsertOutcome::Pasted { clipboard_restored } => (
                if *clipboard_restored { t!("history.trail.pasted_restored", app = app) } else { t!("history.trail.pasted_kept", app = app) },
                c.accent,
            ),
            InsertOutcome::Typed => (t!("history.trail.typed", app = app), c.accent),
            InsertOutcome::Failed { reason } => (t!("history.trail.not_inserted", reason = reason), c.danger),
            InsertOutcome::NotInserted => (t!("history.trail.cancelled"), c.pencil),
        };

        let mut col = div().flex().flex_col().flex_none().pt(px(4.)).child(div().pb(px(14.)).child(caps(t!("history.trail.title"), 12., c.graphite)));
        for (i, (title, body)) in steps.into_iter().enumerate() {
            col = col.child(
                div()
                    .id(("step", i))
                    .flex()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .flex_none()
                            .w(px(12.))
                            .child(div().flex_none().mt(px(3.)).size(px(10.)).rounded_full().border_1().border_color(c.ink))
                            .child(div().flex_1().w(px(1.5)).bg(c.rule)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(3.))
                            .pb(px(16.))
                            .child(ui(title, 13., 16., FontWeight::SEMIBOLD, c.ink))
                            .when_some(body, |d, b| d.child(b)),
                    ),
            );
        }
        col.child(
            div()
                .flex()
                .gap(px(16.))
                .child(
                    div().flex().flex_col().items_center().flex_none().w(px(12.)).child(
                        div()
                            .relative()
                            .mt(px(3.))
                            .size(px(10.))
                            .rounded_full()
                            .bg(last_color)
                            .child(sayso_ui::components::halo(10., 3., last_color.opacity(0.18))),
                    ),
                )
                .child(ui(last, 13., 16., FontWeight::SEMIBOLD, c.ink)),
        )
    }
}

/// Audio bars that remember where they were painted, for seeking.
fn bars(peaks: Vec<u8>, progress: f32, played: Hsla, rest: Hsla, out: Rc<Cell<Option<Bounds<Pixels>>>>) -> Canvas<()> {
    canvas(
        move |bounds, _, _| out.set(Some(bounds)),
        move |bounds, _, window, _| {
            if peaks.is_empty() {
                return;
            }
            // Thin bars as in the design: at most one bar per 3.3 px.
            let max_bars = (bounds.size.width.as_f32() / 3.3).floor().max(1.0) as usize;
            let peaks: Vec<u8> = if peaks.len() > max_bars {
                (0..max_bars).map(|i| peaks[i * peaks.len() / max_bars]).collect()
            } else {
                peaks.clone()
            };
            let n = peaks.len() as f32;
            let step = bounds.size.width.as_f32() / n;
            let bar_w = (step * 0.55).clamp(1.5, 3.0);
            for (i, p) in peaks.iter().enumerate() {
                let a = (*p as f32 / 255.0).max(0.08);
                let bh = bounds.size.height.as_f32() * a;
                let x = bounds.origin.x.as_f32() + step * i as f32 + (step - bar_w) / 2.0;
                let y = bounds.origin.y.as_f32() + (bounds.size.height.as_f32() - bh) / 2.0;
                let color = if (i as f32 + 0.5) / n <= progress { played } else { rest };
                window.paint_quad(
                    fill(Bounds { origin: point(px(x), px(y)), size: size(px(bar_w), px(bh)) }, color).corner_radii(px(bar_w / 2.0)),
                );
            }
        },
    )
}

impl Render for HistoryPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c: Colors = cx.paper().colors;
        // The inputs keep their placeholders, so they change with the language.
        if self.locale != sayso_i18n::locale() {
            self.locale = sayso_i18n::locale();
            self.search.update(cx, |s, cx| s.set_placeholder(t!("history.search_placeholder"), window, cx));
            self.app_filter.update(cx, |s, cx| s.set_placeholder(t!("history.app_filter_placeholder"), window, cx));
        }
        self.narrow = crate::layout::page_narrow(window, &self.model.read(cx).config);
        if let Some(id) = kit::take_history_selection(cx) {
            if !self.entries.iter().any(|e| e.id == id) {
                self.clear_filters(window, cx);
            }
            self.select(id, cx);
        }
        let revision = self.model.read(cx).history_revision;
        if revision != self.seen_revision {
            self.busy = None;
            self.refresh(cx);
        }
        let total = self.model.read(cx).history_total;
        let root = div().absolute().inset_0().flex().text_color(c.ink);
        if total == 0 && !self.model.read(cx).config.history.enabled {
            let model = self.model.clone();
            let turn_on = Button::new("turn-on-history", t!("history.off.turn_on"))
                .small()
                .on_click(move |_, _, cx| model.update(cx, |m, cx| m.edit_config(cx, |c| c.history.enabled = true)));
            return root.items_center().justify_center().child(kit::empty_state(
                Icon::History,
                &t!("history.off.title"),
                &t!("history.off.body"),
                Some(turn_on.into_any_element()),
                cx,
            ));
        }
        if total == 0 {
            let hk = self
                .model
                .read(cx)
                .config
                .hotkeys
                .toggle
                .as_ref()
                .map(|h| h.keycaps().join(" "))
                .unwrap_or_else(|| t!("history.empty.your_hotkey"));
            return root.items_center().justify_center().child(kit::empty_state(
                Icon::History,
                &t!("history.empty.title"),
                &t!("history.empty.body", hotkey = hk),
                None,
                cx,
            ));
        }
        let detail = self.detail_pane(cx);
        let modal = self.transcript_open.then(|| self.current().cloned()).flatten().map(|e| self.transcript_modal(&e, cx));
        root.child(self.list_pane(cx)).child(detail).children(modal)
    }
}

impl Drop for HistoryPage {
    fn drop(&mut self) {
        self.stop_audio();
    }
}


/// Paragraphs split at blank lines. Single line breaks stay inside a paragraph.
fn paragraphs(text: &str) -> Vec<&str> {
    let parts: Vec<&str> = text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() { vec![text.trim()] } else { parts }
}

/// The rows of the app picker: "All apps" (None), then the apps. With a
/// query, only the apps whose name has it.
fn app_rows(apps: Vec<AppCount>, query: &str) -> Vec<Option<AppCount>> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return std::iter::once(None).chain(apps.into_iter().map(Some)).collect();
    }
    apps.into_iter().filter(|a| a.name.to_lowercase().contains(&query)).map(Some).collect()
}

#[cfg(test)]
mod tests {
    // Not a glob: gpui_kit exports its own `test` macro.
    use super::{AppCount, app_rows, paragraphs};

    #[test]
    fn app_rows_filter_by_name() {
        let apps = || {
            ["Orca", "Grok Bot", "OpenMausBot"]
                .into_iter()
                .map(|n| AppCount { bundle_id: n.to_lowercase(), name: n.to_string(), count: 1 })
                .collect::<Vec<_>>()
        };
        let names = |q: &str| app_rows(apps(), q).into_iter().map(|r| r.map(|a| a.name)).collect::<Vec<_>>();
        assert_eq!(names(" "), [None, Some("Orca".into()), Some("Grok Bot".into()), Some("OpenMausBot".into())]);
        assert_eq!(names("BOT "), [Some("Grok Bot".to_string()), Some("OpenMausBot".into())]);
        assert!(names("zed").is_empty());
    }

    #[test]
    fn paragraphs_split_at_blank_lines() {
        assert_eq!(paragraphs("One.\n\nTwo.\n\n\nThree."), ["One.", "Two.", "Three."]);
        assert_eq!(paragraphs("- a\n- b"), ["- a\n- b"]);
        assert_eq!(paragraphs("  "), [""]);
    }
}
