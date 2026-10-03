//! The home page. See the Paper board "Hub — Home".

use super::ext::StyleRoute;
use super::kit::{self, caps, fraunces, mono, ui};
use crate::hub::{Route, SettingsPage};
use crate::model::AppModel;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::stats::format_count;
use sayso_core::stt::ModelStatus;
use sayso_platform::Permission;
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::{ActivePaper, Colors, text};

/// A fix action for a row in the "All set" list.
type Fix = Box<dyn Fn(&mut AppModel, &mut Context<AppModel>)>;

pub struct HomePage {
    model: Entity<AppModel>,
    _observe: Subscription,
}

impl HomePage {
    pub fn new(model: Entity<AppModel>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self { model, _observe: observe }
    }

    fn header(&self, cx: &App) -> Div {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let now = chrono::Local::now();
        let hour = chrono::Timelike::hour(&now);
        let part = if hour < 12 {
            "morning"
        } else if hour < 18 {
            "afternoon"
        } else {
            "evening"
        };
        // The name from Settings › General, or the Mac account's first name.
        let name = m.config.general.name().map(str::to_string).or_else(kit::first_name);
        let greeting = match name {
            Some(name) => format!("Good {part}, {name}."),
            None => format!("Good {part}."),
        };
        let hint = match &m.config.hotkeys.toggle {
            Some(hk) => div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(ui("Press", 14., 18., FontWeight::NORMAL, c.graphite))
                .child(Keycaps::hotkey(hk))
                .child(ui("anywhere to dictate", 14., 18., FontWeight::NORMAL, c.graphite)),
            None => {
                let model = self.model.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(ui("No hotkey is set.", 14., 18., FontWeight::NORMAL, c.graphite))
                    .child(kit::link("set-hotkey", "Set a hotkey", 14., cx).on_click(move |_, _, cx| {
                        model.update(cx, |m, cx| m.navigate(Route::Settings(SettingsPage::Dictation), cx))
                    }))
            }
        };
        div()
            .flex()
            .flex_none()
            .items_end()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(caps(now.format("%A, %B %-d").to_string(), 13., c.graphite))
                    .child(text::display(greeting, 44., &c)),
            )
            .child(hint.pb(px(6.)))
    }

    fn stats(&self, cx: &App) -> Div {
        let c = cx.paper().colors;
        let s = &self.model.read(cx).stats;
        let stat = |label: &str, value: Option<String>, first: bool| {
            let v = match value {
                Some(v) => text::display(v, 40., &c),
                None => text::display("None yet", 32., &c).opacity(0.45),
            };
            div()
                .flex()
                .flex_col()
                .flex_1()
                .gap(px(6.))
                .when(!first, |d| d.pl(px(24.)).border_l_1().border_color(c.rule))
                .child(ui(label.to_string(), 13., 16., FontWeight::MEDIUM, c.graphite))
                .child(v)
        };
        let max = s.week_by_day.iter().copied().max().unwrap_or(0).max(1) as f32;
        let mut bars = div().flex().flex_none().items_end().gap(px(6.)).h(px(44.));
        for (i, words) in s.week_by_day.iter().enumerate() {
            let (h, color) = if i > s.today_index || *words == 0 {
                (6., c.rule)
            } else {
                let h = (6. + 34. * (*words as f32 / max)).round();
                (h, if i == s.today_index { c.ink } else { c.deboss_shade })
            };
            bars = bars.child(div().flex_none().w(px(8.)).h(px(h)).rounded(px(3.)).bg(color));
        }
        let mut week = div()
            .flex()
            .items_end()
            .justify_between()
            .pl(px(24.))
            .border_l_1()
            .border_color(c.rule)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(ui("This week", 13., 16., FontWeight::MEDIUM, c.graphite))
                    .child(text::display(format_count(s.words_this_week), 40., &c)),
            )
            .child(bars);
        week.style().flex_grow = Some(1.3);
        week.style().flex_shrink = Some(1.);
        week.style().flex_basis = Some(relative(0.).into());
        div()
            .flex()
            .flex_none()
            .py(px(24.))
            .border_t_1()
            .border_b_1()
            .border_color(c.rule)
            .child(stat("Words today", Some(format_count(s.words_today)), true))
            .child(stat("Time saved", Some(format!("{} min", s.minutes_saved_today)), false))
            .child(stat("Speaking pace", s.pace_wpm.map(|p| format!("{p} wpm")), false))
            .child(week)
    }

    fn recent(&self, cx: &App) -> Div {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let open = {
            let model = self.model.clone();
            kit::link("open-history", "Open History", 14., cx)
                .on_click(move |_, _, cx| model.update(cx, |m, cx| m.navigate(Route::History, cx)))
        };
        let mut col = div().flex().flex_col().flex_1().min_w_0().gap(px(6.)).child(
            div()
                .flex()
                .items_end()
                .justify_between()
                .pb(px(8.))
                .child(text::title("Recent", 22., &c).line_height(px(28.)))
                .when(!m.recent.is_empty(), |d| d.child(open)),
        );
        if m.recent.is_empty() {
            let hk = m.config.hotkeys.toggle.as_ref().map(|h| h.keycaps().join(" ")).unwrap_or_else(|| "your hotkey".into());
            return col.child(
                div().rounded(px(14.)).well(&c).child(kit::empty_state(
                    sayso_ui::assets::Icon::Mic,
                    "No dictations yet",
                    &format!("Press {hk} in any app and speak. Your dictations show here."),
                    None,
                    cx,
                )),
            );
        }
        for e in m.recent.iter().take(6) {
            let style = m.styles.get(&e.style_id);
            let dot = style.map(|s| kit::ink_color(&s.ink, &c));
            let name = m.style_name(&e.style_id);
            let app = e.app.as_ref().map(|a| a.name.clone()).unwrap_or_else(|| "Unknown app".into());
            let app_icon = e.app.as_ref().and_then(|a| m.icons.get(&a.bundle_id));
            let model = self.model.clone();
            let id = e.id;
            col = col.child(
                div()
                    .id(("recent", e.id as u64))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(14.))
                    .h(px(56.))
                    .px(px(4.))
                    .border_b_1()
                    .border_color(c.rule)
                    .cursor_pointer()
                    .hover(|s| s.bg(c.deboss.opacity(0.35)))
                    .on_click(move |_, _, cx| {
                        kit::select_history_entry(id, cx);
                        model.update(cx, |m, cx| m.navigate(Route::History, cx));
                    })
                    .child(mono(kit::clock(e.created_at), 12., c.graphite).w(px(44.)).flex_none())
                    .child(AppBadge::new(app).icon(app_icon))
                    .child(fraunces(e.final_text.replace('\n', " "), 15., 18., c.ink).flex_1().min_w_0().truncate())
                    .child(StyleTag::new(name, dot).width(84.))
                    .child(mono(kit::duration(e.duration_ms), 12., c.graphite).w(px(36.)).flex_none().text_right()),
            );
        }
        col
    }

    fn active_style(&self, cx: &App) -> Div {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let style = m.active_style();
        let route = m.style_route(&style);
        let detail = match &route {
            StyleRoute::NoAi => format!("Nothing leaves your {}.", crate::os::COMPUTER),
            StyleRoute::NeedsProvider => "AI is off, so Sayso inserts the transcript. Add a provider in Styles.".into(),
            StyleRoute::NeedsModel { provider } => format!("No model is chosen for {provider}, so Sayso inserts the transcript. Choose one in Styles."),
            StyleRoute::Local { provider, model } => format!("Runs on your {} with {provider} · {model}.", crate::os::COMPUTER),
            StyleRoute::Cloud { provider, model } => format!("Sent to {provider} · {model}."),
        };
        let description = if style.description.is_empty() { detail } else { format!("{} {}", style.description, detail) };
        let badge = match &route {
            StyleRoute::Cloud { .. } => Some(kit::place_badge(true, cx)),
            StyleRoute::Local { .. } | StyleRoute::NoAi => Some(kit::place_badge(false, cx)),
            StyleRoute::NeedsProvider | StyleRoute::NeedsModel { .. } => None,
        };
        let model = self.model.clone();
        let change = Button::new("change-style", "Change style")
            .small()
            .on_click(move |_, _, cx| model.update(cx, |m, cx| m.navigate(Route::Styles, cx)));
        let cycle = m.config.hotkeys.cycle_style.as_ref().map(|hk| {
            div().flex().items_center().h(px(30.)).px(px(4.)).child(
                div()
                    .flex()
                    .items_center()
                    .h(px(22.))
                    .px(px(6.))
                    .rounded(px(5.))
                    .keycap(&c)
                    .child(mono(hk.keycaps().join(""), 11., c.ink).line_height(px(14.))),
            )
        });
        let under = c.sheet.blend(c.deboss.opacity(0.5));
        div()
            .relative()
            .flex()
            .flex_col()
            .pt(px(10.))
            .child(div().absolute().top_0().left(px(14.)).right(px(14.)).h(px(40.)).rounded(px(12.)).bg(c.deboss).shadow(vec![
                BoxShadow::new(px(0.), px(1.), c.shadow(0.10)).blur_radius(px(2.)),
            ]))
            .child(div().absolute().top(px(5.)).left(px(7.)).right(px(7.)).h(px(40.)).rounded(px(12.)).bg(under).shadow(vec![
                BoxShadow::new(px(0.), px(1.), c.shadow(0.10)).blur_radius(px(2.)),
            ]))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .py(px(18.))
                    .px(px(20.))
                    .rounded(px(12.))
                    .bg(c.sheet_raised)
                    .shadow(vec![
                        BoxShadow::new(px(0.), px(1.), c.highlight(1.0)).inset(),
                        BoxShadow::new(px(0.), px(1.), c.shadow(0.14)).blur_radius(px(2.)),
                        BoxShadow::new(px(0.), px(10.), c.shadow(0.12)).blur_radius(px(26.)),
                    ])
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .h(px(22.))
                            .child(caps("Active style", 12., c.graphite))
                            .when_some(badge, |d, b| d.child(b)),
                    )
                    .child(text::display(style.name.clone(), 28., &c))
                    .child(ui(description, 14., 20., FontWeight::NORMAL, c.graphite))
                    .child(div().flex().gap(px(6.)).pt(px(4.)).child(change).when_some(cycle, |d, k| d.child(k))),
            )
    }

    fn checklist(&self, cx: &App) -> Div {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let mic_ok = m.granted(Permission::Microphone);
        let ax_ok = m.granted(Permission::Accessibility);
        let active = m.active_model();
        let status = m.status_of(&active.id);
        let model_ok = m.model_ready();
        let all_ok = mic_ok && ax_ok && model_ok;

        let fix = |id: &'static str, label: &str, f: Fix| {
            let model = self.model.clone();
            kit::link(id, label.to_string(), 13., cx).on_click(move |_, _, cx| model.update(cx, |m, cx| f(m, cx)))
        };
        let row = |ok: bool, label: &str, value: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(kit::check_dot(ok, cx))
                .child(ui(label.to_string(), 14., 18., FontWeight::NORMAL, c.ink).flex_1())
                .child(value)
        };
        let plain = |s: String| ui(s, 13., 16., FontWeight::NORMAL, c.graphite).truncate().max_w(px(170.)).into_any_element();

        let mic = if mic_ok {
            plain(m.microphone_name().unwrap_or_else(|| "Allowed".into()))
        } else {
            fix("fix-mic", "Allow", Box::new(|m, _| m.fix_permission(Permission::Microphone))).into_any_element()
        };
        let ax = if ax_ok {
            plain("Allowed".into())
        } else {
            fix("fix-ax", "Allow", Box::new(|m, _| m.fix_permission(Permission::Accessibility))).into_any_element()
        };
        let model_value = match &status {
            ModelStatus::NotDownloaded if active.is_remote() => {
                fix("fix-model", "Open Models", Box::new(|m, cx| m.navigate(Route::Models, cx))).into_any_element()
            }
            ModelStatus::Ready | ModelStatus::Downloaded => plain(active.name.clone()),
            ModelStatus::Optimizing => plain(crate::os::OPTIMIZING.into()),
            ModelStatus::Downloading { fraction, .. } => plain(format!("Downloading {:.0}%", fraction * 100.)),
            ModelStatus::NotDownloaded => {
                fix("fix-model", "Download", Box::new(|m, cx| m.navigate(Route::Models, cx))).into_any_element()
            }
            ModelStatus::Failed { .. } => fix("fix-model", "Open Models", Box::new(|m, cx| m.navigate(Route::Models, cx))).into_any_element(),
        };
        let words = m.words.len();
        let rules = m.replacements.len();
        let dict = if words + rules == 0 {
            fix("fix-dict", "Add words", Box::new(|m, cx| m.navigate(Route::Dictionary, cx))).into_any_element()
        } else {
            plain(format!("{} · {}", kit::plural(words, "word", "words"), kit::plural(rules, "rule", "rules")))
        };
        let mut list = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(4.))
            .child(caps(if all_ok { "All set" } else { "Needs attention" }, 12., c.graphite))
            .child(row(mic_ok, "Microphone", mic))
            .when(crate::os::HAS_INPUT_PERMISSIONS, |d| d.child(row(ax_ok, "Accessibility", ax)))
            .child(row(model_ok, "Model", model_value))
            .child(row(true, "Dictionary", dict));
        // A cloud model does not need the engine for its final pass.
        if !active.is_remote() && (m.services.engine.is_none() || m.engine_down.is_some()) {
            list = list.child(row(false, "Engine", plain("Stopped. Restarting".into())));
        }
        list
    }
}

impl Render for HomePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c: Colors = cx.paper().colors;
        div()
            .id("home")
            .absolute()
            .inset_0()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .gap(px(40.))
                    .py(px(44.))
                    .px(px(52.))
                    .text_color(c.ink)
                    .child(self.header(cx))
                    .child(self.stats(cx))
                    .child(
                        div()
                            .flex()
                            .w_full()
                            .gap(px(40.))
                            .child(self.recent(cx))
                            .child(div().flex().flex_col().flex_none().w(px(300.)).gap(px(28.)).child(self.active_style(cx)).child(self.checklist(cx))),
                    ),
            )
    }
}
