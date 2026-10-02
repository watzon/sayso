//! The history page. See the Paper board "Hub — History".
//!
//! Left: search, filters, and entries grouped by day. Right: the selected
//! entry with its audio and the trail of how the text got there.

use super::kit::{self, caps, fraunces, mono, ui};
use super::player::{self, Playing};
use crate::hub::{Route, SettingsPage};
use crate::model::AppModel;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::history::{EnhanceOutcome, HistoryEntry, InsertOutcome};
use sayso_store::HistoryQuery;
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::fonts::DISPLAY;
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

pub struct HistoryPage {
    model: Entity<AppModel>,
    search: Entity<InputState>,
    enhanced: bool,
    has_audio: bool,
    /// (bundle id, name)
    app: Option<(String, String)>,
    app_menu: bool,
    limit: usize,
    entries: Vec<HistoryEntry>,
    matches: usize,
    selected: Option<i64>,
    seen_revision: u64,
    playing: Option<Playing>,
    /// (entry id, position in ms) for the paused position.
    position: (i64, u64),
    audio: Option<(String, Rc<Vec<f32>>)>,
    notice: Option<String>,
    confirm_delete: bool,
    busy: Option<&'static str>,
    copied: bool,
    /// Scroll position of the detail text, so the fade knows when the end shows.
    text_scroll: ScrollHandle,
    /// The full transcript modal is open.
    transcript_open: bool,
    modal_focus: FocusHandle,
    bar_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    _subs: Vec<Subscription>,
}

impl HistoryPage {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search what you said"));
        let sub = cx.subscribe_in(&search, window, |this: &mut Self, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                this.limit = PAGE;
                this.refresh(cx);
            }
        });
        let mut page = Self {
            model,
            search,
            enhanced: false,
            has_audio: false,
            app: None,
            app_menu: false,
            limit: PAGE,
            entries: Vec::new(),
            matches: 0,
            selected: None,
            seen_revision: u64::MAX,
            playing: None,
            position: (0, 0),
            audio: None,
            notice: None,
            confirm_delete: false,
            busy: None,
            copied: false,
            text_scroll: ScrollHandle::new(),
            transcript_open: false,
            modal_focus: cx.focus_handle(),
            bar_bounds: Rc::new(Cell::new(None)),
            _subs: vec![observe, sub],
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

    fn select(&mut self, id: i64, cx: &mut Context<Self>) {
        if self.selected != Some(id) {
            self.stop_audio();
            self.selected = Some(id);
            self.confirm_delete = false;
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
            self.notice = Some("The audio file is missing. It may have expired.".into());
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
            Err(e) => self.notice = Some(e),
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
        let pos = (f * total as f32) as u64;
        let was_playing = self.playing.is_some();
        self.stop_audio();
        self.position = (entry.id, pos);
        if was_playing {
            self.play_from(entry.id, &samples, pos, cx);
        }
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // List pane
    // -----------------------------------------------------------------------

    fn list_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let total = self.model.read(cx).history_total;
        let filtered = self.filters_active(cx);
        let count = if filtered {
            format!("{} of {}", sayso_core::stats::format_count(self.matches), sayso_core::stats::format_count(total))
        } else {
            kit::plural(total, "dictation", "dictations").replacen(&total.to_string(), &sayso_core::stats::format_count(total), 1)
        };

        let mut list = div().flex().flex_col().gap(px(4.));
        let mut last_day = None;
        for (i, e) in self.entries.iter().enumerate() {
            let day = e.created_at.with_timezone(&chrono::Local).date_naive();
            if last_day != Some(day) {
                list = list.child(
                    div().px(px(10.)).pb(px(6.)).pt(px(if i == 0 { 6. } else { 14. })).child(caps(kit::day_label(day), 12., c.graphite)),
                );
                last_day = Some(day);
            }
            list = list.child(self.entry_row(e, cx));
        }
        if self.entries.len() < self.matches {
            list = list.child(
                div().flex().justify_center().py(px(12.)).child(
                    Button::new("more", format!("Show {} more", (self.matches - self.entries.len()).min(PAGE)))
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
            let clear = Button::new("clear", "Clear filters").small().on_click(cx.listener(|this, _, w, cx| this.clear_filters(w, cx)));
            list = list.child(kit::empty_state(
                Icon::Search,
                "Nothing matches",
                "No dictation has these words or filters. Try other words.",
                Some(clear.into_any_element()),
                cx,
            ));
        }

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(392.))
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
                    .child(sayso_ui::text::title("History", 32., &c).line_height(px(40.)))
                    .child(ui(count, 13., 16., FontWeight::NORMAL, c.graphite).pb(px(6.))),
            )
            .child(kit::search_well(&self.search, 38., Some(kit::mini_key("⌘F", cx).into_any_element()), cx))
            .child(self.filters(cx))
            // The scroll area clips to its bounds. It reaches out to the pane edges
            // (with matching padding, so rows stay put) to leave room for the
            // selected row's soft shadow, which the design lets spread past the row.
            .child(div().id("entries").flex_1().min_h_0().overflow_y_scroll().mx(px(-20.)).px(px(20.)).pb(px(20.)).child(list))
    }

    fn filters(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let all = !self.enhanced && !self.has_audio && self.app.is_none();
        let app_label = self.app.as_ref().map(|a| a.1.clone()).unwrap_or_else(|| "App".into());
        let mut row = div()
            .flex()
            .flex_none()
            .gap(px(6.))
            .child(Chip::new("f-all", "All", all).on_click(cx.listener(|this, _, _, cx| {
                this.enhanced = false;
                this.has_audio = false;
                this.app = None;
                this.refresh(cx);
            })))
            .child(Chip::new("f-enh", "Enhanced", self.enhanced).on_click(cx.listener(|this, _, _, cx| {
                this.enhanced = !this.enhanced;
                this.refresh(cx);
            })))
            .child(Chip::new("f-audio", "Has audio", self.has_audio).on_click(cx.listener(|this, _, _, cx| {
                this.has_audio = !this.has_audio;
                this.refresh(cx);
            })));
        let chip = Chip::new("f-app", app_label, self.app.is_some())
            .trailing(Icon::ChevronDown)
            .on_click(cx.listener(|this, _, _, cx| {
                this.app_menu = !this.app_menu;
                cx.notify();
            }));
        let mut app = div().relative().child(chip);
        if self.app_menu {
            let apps = self.model.read(cx).history_apps();
            let mut items: Vec<(SharedString, bool)> = vec![("All apps".into(), self.app.is_none())];
            for a in &apps {
                let sel = self.app.as_ref().is_some_and(|x| x.0 == a.bundle_id);
                items.push((format!("{} ({})", a.name, a.count).into(), sel));
            }
            let entity = cx.entity();
            let entity2 = cx.entity();
            app = app.child(div().absolute().top_full().left_0().child(kit::menu(
                "app-menu",
                items,
                move |i, _, cx| {
                    let pick = if i == 0 { None } else { apps.get(i - 1).map(|a| (a.bundle_id.clone(), a.name.clone())) };
                    entity.update(cx, |this, cx| {
                        this.app = pick;
                        this.app_menu = false;
                        this.refresh(cx);
                    });
                },
                move |_, cx| {
                    entity2.update(cx, |this, cx| {
                        this.app_menu = false;
                        cx.notify();
                    })
                },
                cx,
            )));
        }
        row = row.child(app);
        row
    }

    fn entry_row(&self, e: &HistoryEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let selected = self.selected == Some(e.id);
        let app = e.app.as_ref().map(|a| a.name.clone()).unwrap_or_else(|| "Unknown app".into());
        let app_icon = e.app.as_ref().and_then(|a| self.model.read(cx).icons.get(&a.bundle_id));
        let id = e.id;
        div()
            .id(("entry", e.id as u64))
            .flex()
            .flex_none()
            .gap(px(12.))
            .p(px(12.))
            .rounded(px(12.))
            .cursor_pointer()
            .when(selected, |d| {
                d.bg(c.sheet_raised).shadow(vec![
                    BoxShadow::new(px(0.), px(1.), c.highlight(1.0)).inset(),
                    BoxShadow::new(px(0.), px(1.), c.shadow(0.14)).blur_radius(px(2.)),
                    BoxShadow::new(px(0.), px(6.), c.shadow(0.10)).blur_radius(px(16.)),
                ])
            })
            .when(!selected, |d| d.hover(|s| s.bg(c.deboss.opacity(0.4))))
            .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
            .child(AppBadge::new(app.clone()).icon(app_icon))
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
                            .child(ui(app, 13., 16., FontWeight::SEMIBOLD, c.ink))
                            .child(mono(kit::clock(e.created_at), 12., c.graphite)),
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
        let app = e.app.as_ref().map(|a| a.name.clone()).unwrap_or_else(|| "Unknown app".into());
        let app_icon = e.app.as_ref().and_then(|a| self.model.read(cx).icons.get(&a.bundle_id));
        let (can_enhance, engine) = {
            let m = self.model.read(cx);
            let style = m.active_style();
            (m.provider_for(&style).is_some_and(|p| !p.needs_model()) && style.uses_ai(), m.model_ready())
        };

        let enhance = {
            let e2 = e.clone();
            // Says what a click does: a second pass if a style already ran.
            let again = matches!(e.enhance, EnhanceOutcome::Applied { .. });
            let label = match (self.busy == Some("enhance"), again) {
                (true, _) => "Enhancing",
                (false, true) => "Re-enhance",
                (false, false) => "Enhance",
            };
            Button::new("enhance", label)
                .disabled(!can_enhance || self.busy.is_some())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.busy = Some("enhance");
                    let entry = e2.clone();
                    this.model.update(cx, |m, cx| m.enhance_entry(entry, cx));
                    cx.notify();
                }))
        };
        let copy = {
            let text = e.final_text.clone();
            Button::new("copy", if self.copied { "Copied" } else { "Copy" }).primary().on_click(cx.listener(move |this, _, _, cx| {
                this.model.read(cx).copy_text(&text);
                this.copied = true;
                cx.notify();
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_millis(1500)).await;
                    let _ = this.update(cx, |p, cx| {
                        p.copied = false;
                        cx.notify();
                    });
                })
                .detach();
            }))
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
                    .child(AppBadge::new(app.clone()).icon(app_icon).size(24.))
                    .child(ui(app.clone(), 14., 18., FontWeight::SEMIBOLD, c.ink).truncate())
                    .child(
                        ui(
                            format!("{} · {}", kit::clock(e.created_at), kit::duration_words(e.duration_ms)),
                            14.,
                            18.,
                            FontWeight::NORMAL,
                            c.graphite,
                        )
                        .flex_none(),
                    ),
            )
            .child(div().flex().flex_none().gap(px(6.)).child(enhance).child(copy));

        let content = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(px(28.))
            .pt(px(8.))
            .pb(px(32.))
            .px(px(48.))
            .child(self.final_text(&e.final_text, &c))
            .child(div().flex_none().child(self.player(&e, cx)))
            .child(self.trail(&e, cx))
            .child(self.actions(&e, engine, cx));
        // Nothing here scrolls but the text: the player, the trail, and the
        // actions always show. On a short window the text box gives up height first.
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .child(div().flex_none().pt(px(40.)).pb(px(20.)).px(px(48.)).child(header))
            .child(content)
            .into_any_element()
    }

    /// The dictation in display type, at most 3.5 lines tall. Longer text
    /// scrolls inside the box, and a fade marks that more follows.
    fn final_text(&self, text: &str, c: &Colors) -> AnyElement {
        let mut body = div().flex().flex_col().gap(px((DETAIL_LINE * 0.5).round()));
        for paragraph in paragraphs(text) {
            body = body.child(div().child(paragraph.to_string()));
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
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.text_scroll)
                    .font_family(DISPLAY)
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
            .track_focus(&self.modal_focus)
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.transcript_open = false;
                    cx.notify();
                }
            }))
            .on_click(close)
            .child(
                div()
                    .id("transcript-modal")
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .w(px(560.))
                    .max_h(relative(0.8))
                    .p(px(28.))
                    .rounded(px(16.))
                    .bg(c.sheet_raised)
                    .shadow(paper::floating(&c))
                    // A click inside the sheet does not close it.
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(caps("Transcript", 12., c.graphite))
                            .child(ui(format!("What {model_name} heard, before your dictionary and style."), 13., 18., FontWeight::NORMAL, c.graphite)),
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
                            .child(Button::new("transcript-copy", "Copy").on_click(cx.listener(move |this, _, _, cx| {
                                this.model.read(cx).copy_text(&transcript);
                            })))
                            .child(Button::new("transcript-close", "Close").primary().on_click(cx.listener(|this, _, _, cx| {
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
        let button = div()
            .id("play")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(44.))
            .rounded_full()
            .bg(c.ink_fill)
            .shadow(vec![
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
        let wave = div()
            .id("wave")
            .flex_1()
            .min_w_0()
            .h(px(44.))
            .when(has_audio, |d| {
                d.cursor_pointer().on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| this.seek(&e2, ev.position().x, cx)))
            })
            .child(bars(e.waveform.clone(), progress, if has_audio { c.ink } else { rest }, rest, bounds).size_full());

        let time = if has_audio { format!("{} / {}", kit::duration(pos), kit::duration(total)) } else { kit::duration(total) };
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
            .child(mono(time, 12., c.graphite).flex_none());

        let m = self.model.read(cx);
        let note = if let Some(n) = &self.notice {
            n.clone()
        } else if !has_audio {
            if m.config.history.save_audio { "The audio of this dictation was not saved or has expired.".into() } else { "Audio was not saved. You can turn on saving in Settings › History.".into() }
        } else if let Some(at) = m.audio_expiry(e) {
            format!("Audio is kept until {}. Change this in Settings › History.", at.format("%B %-d"))
        } else {
            "Audio is kept until you delete it. Change this in Settings › History.".into()
        };
        let model = self.model.clone();
        div().flex().flex_col().flex_none().gap(px(10.)).child(tray).child(
            div()
                .id("audio-note")
                .pl(px(4.))
                .cursor_pointer()
                .on_click(move |_, _, cx| model.update(cx, |m, cx| m.navigate(Route::Settings(SettingsPage::HistoryPrivacy), cx)))
                .child(ui(note, 13., 16., FontWeight::NORMAL, if self.notice.is_some() { c.danger } else { c.graphite })),
        )
    }

    fn trail(&self, e: &HistoryEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let mut steps: Vec<(String, Option<AnyElement>)> = Vec::new();

        let ms = if e.transcribe_ms > 0 { format!(" · {} ms", e.transcribe_ms) } else { String::new() };
        let long = e.transcript.chars().count() > TRANSCRIPT_PREVIEW_CHARS || e.transcript.contains('\n');
        let preview = fraunces(e.transcript.replace('\n', " "), 15., 21., c.graphite).italic().line_clamp(2).text_ellipsis();
        steps.push((
            format!("Transcript · {}{ms}", m.model_name(&e.model)),
            Some(if long {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(preview)
                    .child(
                        div()
                            .id("show-transcript")
                            .cursor_pointer()
                            .child(ui("Show full transcript", 13., 16., FontWeight::SEMIBOLD, c.accent))
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
            steps.push(("Dictionary · no changes".into(), None));
        } else {
            let mut body = div().flex().flex_col().gap(px(2.));
            // Three lines at most, so the trail keeps its height.
            for r in e.replacements.iter().take(3) {
                let times = if r.count > 1 { format!("  ×{}", r.count) } else { String::new() };
                body = body.child(ui(format!("“{}” → “{}”{times}", r.from, r.to.replace("\\n", "↵")), 14., 18., FontWeight::NORMAL, c.graphite));
            }
            if e.replacements.len() > 3 {
                body = body.child(ui(format!("and {} more", e.replacements.len() - 3), 14., 18., FontWeight::NORMAL, c.pencil));
            }
            steps.push((format!("Dictionary · {}", kit::plural(n, "replacement", "replacements")), Some(body.into_any_element())));
        }

        let style_name = m.style_name(&e.style_id);
        let style_desc = m.styles.get(&e.style_id).map(|s| s.description.clone()).unwrap_or_default();
        match &e.enhance {
            EnhanceOutcome::Applied { provider_id, model, elapsed_ms } => steps.push((
                format!(
                    "Style · {style_name} · {}, {} · {:.2} s",
                    m.provider_name(provider_id),
                    super::ext::short_model(model),
                    *elapsed_ms as f64 / 1000.0
                ),
                (!style_desc.is_empty()).then(|| ui(style_desc.clone(), 14., 18., FontWeight::NORMAL, c.graphite).into_any_element()),
            )),
            EnhanceOutcome::Failed { provider_id, reason } => steps.push((
                format!("Style · {style_name} · failed with {}", m.provider_name(provider_id)),
                Some(
                    ui(format!("{reason}. Sayso inserted the text without the style."), 14., 18., FontWeight::NORMAL, c.danger)
                        .into_any_element(),
                ),
            )),
            EnhanceOutcome::NotUsed => steps.push((
                format!("Style · {style_name} · not used"),
                Some(
                    ui(
                        if e.style_id == sayso_core::style::RAW_STYLE_ID { "Raw does not use AI." } else { "AI did not run on this text." },
                        14.,
                        18.,
                        FontWeight::NORMAL,
                        c.graphite,
                    )
                    .into_any_element(),
                ),
            )),
        }

        let app = e.app.as_ref().map(|a| a.name.clone()).unwrap_or_else(|| "the app".into());
        let (last, last_color) = match &e.insert {
            InsertOutcome::Pasted { clipboard_restored } => (
                format!("Inserted · Paste into {app} · {}", if *clipboard_restored { "clipboard restored" } else { "clipboard kept" }),
                c.accent,
            ),
            InsertOutcome::Typed => (format!("Inserted · Typed into {app}"), c.accent),
            InsertOutcome::Failed { reason } => (format!("Not inserted · {reason}"), c.danger),
            InsertOutcome::NotInserted => ("Not inserted · The dictation was cancelled".into(), c.pencil),
        };

        let mut col = div().flex().flex_col().flex_none().pt(px(4.)).child(div().pb(px(14.)).child(caps("How it got here", 12., c.graphite)));
        for (title, body) in steps {
            col = col.child(
                div()
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

    fn actions(&self, e: &HistoryEntry, engine: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let id = e.id;
        let mut row = div().flex().flex_none().items_center().gap(px(8.)).pt(px(4.));
        if self.confirm_delete {
            row = row
                .child(ui("Delete this dictation and its audio?", 13., 16., FontWeight::MEDIUM, cx.paper().colors.ink))
                .child(Button::new("del-yes", "Delete").small().danger().on_click(cx.listener(move |this, _, _, cx| {
                    this.stop_audio();
                    this.confirm_delete = false;
                    this.model.update(cx, |m, cx| m.delete_entry(id, cx));
                })))
                .child(Button::new("del-no", "Cancel").small().ghost().on_click(cx.listener(|this, _, _, cx| {
                    this.confirm_delete = false;
                    cx.notify();
                })));
        } else {
            row = row.child(Button::new("delete", "Delete").small().ghost().icon(Icon::Trash).on_click(cx.listener(|this, _, _, cx| {
                this.confirm_delete = true;
                cx.notify();
            })));
            if e.audio_file.is_some() && engine {
                let e2 = e.clone();
                row = row.child(
                    Button::new("retranscribe", if self.busy == Some("retranscribe") { "Transcribing" } else { "Transcribe again" })
                        .small()
                        .ghost()
                        .icon(Icon::Undo)
                        .disabled(self.busy.is_some())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.busy = Some("retranscribe");
                            let entry = e2.clone();
                            this.model.update(cx, |m, cx| m.retranscribe_entry(entry, cx));
                            cx.notify();
                        })),
                );
            }
        }
        row
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
        if total == 0 {
            let hk = self
                .model
                .read(cx)
                .config
                .hotkeys
                .toggle
                .as_ref()
                .map(|h| h.keycaps().join(" "))
                .unwrap_or_else(|| "your hotkey".into());
            return root.items_center().justify_center().child(kit::empty_state(
                Icon::History,
                "No dictations yet",
                &format!("Press {hk} in any app and speak. Each dictation is kept here with its transcript and audio."),
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

#[cfg(test)]
mod tests {
    // Not a glob: gpui_kit exports its own `test` macro.
    use super::paragraphs;

    #[test]
    fn paragraphs_split_at_blank_lines() {
        assert_eq!(paragraphs("One.\n\nTwo.\n\n\nThree."), ["One.", "Two.", "Three."]);
        assert_eq!(paragraphs("- a\n- b"), ["- a\n- b"]);
        assert_eq!(paragraphs("  "), [""]);
    }
}
