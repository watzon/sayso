//! Settings › Advanced: engine path, log level, folders.

use super::kit::{self, group, row};
use crate::model::AppModel;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::*;
use sayso_ui::ActivePaper;
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;

const LEVELS: [&str; 5] = ["error", "warn", "info", "debug", "trace"];

pub struct AdvancedSettings {
    model: Entity<AppModel>,
    engine: Entity<InputState>,
    engine_saved: bool,
    _engine_sub: Subscription,
}

impl AdvancedSettings {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let current = model.read(cx).config.advanced.engine_path.clone().unwrap_or_default();
        let placeholder = sayso_core::paths::Paths::display(
            &sayso_engine_client::EngineClient::default_engine_path(),
            &sayso_core::paths::PathEnv::home(&sayso_core::paths::SystemEnv),
        );
        let engine = crate::widgets::input_state(&placeholder, &current, window, cx);
        let sub = cx.subscribe_in(&engine, window, |this, state, ev: &InputEvent, _, cx| {
            if !matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                return;
            }
            let value = state.read(cx).value().trim().to_string();
            let v = (!value.is_empty()).then_some(value);
            let before = this.model.read(cx).config.advanced.engine_path.clone();
            if before != v {
                this.model.update(cx, |m, cx| m.edit_config(cx, |c| c.advanced.engine_path = v));
                this.engine_saved = true;
                cx.notify();
            }
        });
        Self { model, engine, engine_saved: false, _engine_sub: sub }
    }
}

impl Render for AdvancedSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let adv = m.config.advanced.clone();
        let paths = m.paths.clone();
        let engine_missing = adv.engine_path.as_ref().is_some_and(|p| !std::path::Path::new(p).exists());

        let level = adv.log_level.as_deref().unwrap_or("info");
        let level_index = LEVELS.iter().position(|l| *l == level).unwrap_or(2);
        let m1 = self.model.clone();
        let levels = Segmented::new("log-level", ["Error", "Warn", "Info", "Debug", "Trace"], level_index).on_select(move |i, _, cx| {
            let v = if i == 2 { None } else { Some(LEVELS[i].to_string()) };
            m1.update(cx, |m, cx| m.edit_config(cx, |c| c.advanced.log_level = v));
        });

        let field = div()
            .flex()
            .items_center()
            .w(px(420.))
            .h(px(36.))
            .px(px(12.))
            .rounded(px(9.))
            .debossed(&c)
            .font_family(sayso_ui::fonts::MONO)
            .text_size(px(12.))
            .child(Input::new(&self.engine).appearance(false).w_full());

        let mut engine = group("Engine", cx).child(row(
            "Engine path",
            "A custom SaysoEngine binary. Leave empty to use the one in the app. Takes effect when Sayso starts again.",
            field,
            cx,
        ));
        if engine_missing {
            engine = engine.child(kit::banner(BannerKind::Warning, "No file exists at this path. Check the path, or clear the field to use the bundled engine.", cx));
        } else if self.engine_saved {
            engine = engine.child(kit::banner(BannerKind::Info, "Saved. Quit Sayso and open it again to use this engine.", cx));
        }
        engine = engine.child(row("Log level", "How much Sayso writes to its log. Takes effect when Sayso starts again.", levels, cx));

        let log_dir = paths.log_dir();
        let folders = group("Folders", cx)
            .child(kit::path_row("reveal-config", "Config", &paths.config_dir, cx))
            .child(kit::path_row("reveal-data", "Data (history, audio, models)", &paths.data_dir, cx))
            .child(kit::path_row("reveal-cache", "Cache", &paths.cache_dir, cx))
            .child(row(
                "Logs",
                "Attach sayso.log when you report a problem.",
                Button::new("open-logs", "Open logs folder").small().on_click(move |_, _, _| {
                    let _ = std::fs::create_dir_all(&log_dir);
                    AppModel::open_path(&log_dir);
                }),
                cx,
            ));

        kit::page(
            "advanced-page",
            "Advanced",
            "Engine, logs, experiments, and where Sayso keeps its files.",
            kit::body().child(engine).child(folders),
            cx,
        )
    }
}
