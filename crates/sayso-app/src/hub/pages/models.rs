//! The models page. See the Paper board "Hub — Models".
//!
//! The active model as a hero card with real numbers, then the catalog with
//! a speed meter, the size, and one action per row.

use super::kit::{self, caps, mono, ui};
use crate::model::AppModel;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::models::{EngineKind, ModelInfo, format_size};
use sayso_core::stt::ModelStatus;
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::{ActivePaper, Colors, text};

pub struct ModelsPage {
    model: Entity<AppModel>,
    confirm_delete: Option<String>,
    _observe: Subscription,
}

impl ModelsPage {
    pub fn new(model: Entity<AppModel>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self { model, confirm_delete: None, _observe: observe }
    }

    fn hero(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let info = m.active_model();
        let status = m.status_of(&info.id);
        let engine = m.services.engine.is_some();
        let measured = m.measured_speed(&info.id);

        let (dot, label) = match &status {
            ModelStatus::Ready => (c.success, "In use".to_string()),
            ModelStatus::Downloaded => (c.accent, "Loading".to_string()),
            ModelStatus::Optimizing => (c.accent, "Optimizing for your Mac".to_string()),
            ModelStatus::Downloading { fraction, .. } => (c.accent, format!("Downloading {:.0}%", fraction * 100.)),
            ModelStatus::NotDownloaded => (c.danger, "Not downloaded".to_string()),
            ModelStatus::Failed { .. } => (c.danger, "Model error".to_string()),
        };

        let stat = |value: String, note: String| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(text::title(value, 28., &c).line_height(px(34.)))
                .child(ui(note, 12., 16., FontWeight::NORMAL, c.graphite))
        };
        let mut stats = div().flex().flex_none().items_end().gap(px(28.)).pb(px(4.));
        match measured {
            Some((ms, dur)) => stats = stats.child(stat(format!("{ms} ms"), format!("for {} of speech", kit::duration_words(dur)))),
            None => stats = stats.child(stat(format!("{}/5", info.speed), "speed".into())),
        }
        if let Some(wer) = info.wer_percent {
            stats = stats.child(stat(format!("{wer}%"), "word error rate".into()));
        }
        let parts = match info.engine {
            EngineKind::ParakeetUnified { .. } => "3 parts",
            _ => "1 part",
        };
        stats = stats.child(stat(
            format_size(info.size_bytes),
            if status.is_on_disk() { format!("{parts} on disk") } else { "to download".into() },
        ));

        let id = info.id.clone();
        let action: Option<AnyElement> = match &status {
            ModelStatus::NotDownloaded => Some(
                Button::new("hero-download", format!("Download {}", format_size(info.size_bytes)))
                    .primary()
                    .icon(Icon::Download)
                    .disabled(!engine)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let id = id.clone();
                        this.model.update(cx, |m, cx| m.download_model(id, cx));
                    }))
                    .into_any_element(),
            ),
            ModelStatus::Failed { message } => Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(ui(format!("{message}."), 13., 18., FontWeight::NORMAL, c.danger))
                    .child(Button::new("hero-retry", "Try again").small().on_click(cx.listener(move |this, _, _, cx| {
                        let id = id.clone();
                        this.model.update(cx, |m, cx| m.download_model(id, cx));
                    })))
                    .into_any_element(),
            ),
            ModelStatus::Downloading { fraction, bytes_done, bytes_total } => Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .w(px(360.))
                    .child(Progress::new(*fraction))
                    .when(*bytes_total > 0, |d| {
                        d.child(mono(format!("{} of {}", format_size(*bytes_done), format_size(*bytes_total)), 11., c.graphite))
                    })
                    .into_any_element(),
            ),
            ModelStatus::Optimizing => Some(
                ui("The first start compiles the model for the Neural Engine. This takes a minute or two.", 13., 18., FontWeight::NORMAL, c.graphite)
                    .into_any_element(),
            ),
            _ => None,
        };

        div()
            .flex()
            .flex_none()
            .gap(px(36.))
            .py(px(24.))
            .px(px(28.))
            .rounded(px(16.))
            .bg(c.sheet_raised)
            .shadow(vec![
                BoxShadow::new(px(0.), px(1.), c.highlight(1.0)).inset(),
                BoxShadow::new(px(0.), px(1.), c.shadow(0.14)).blur_radius(px(2.)),
                BoxShadow::new(px(0.), px(12.), c.shadow(0.10)).blur_radius(px(28.)),
            ])
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(10.))
                    .child(div().flex().items_center().gap(px(8.)).child(status_dot(dot, 8.)).child(caps(label, 12., c.graphite)))
                    .child(text::title(info.name.clone(), 32., &c).line_height(px(35.)))
                    .child(ui(format!("{} · {}", info.vendor, info.description), 14., 20., FontWeight::NORMAL, c.graphite))
                    .when_some(action, |d, a| d.child(div().flex().pt(px(6.)).child(a))),
            )
            .child(stats)
    }

    fn row(&self, info: &ModelInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let status = m.status_of(&info.id);
        let engine = m.services.engine.is_some();
        let is_preview = m.preview_model().as_ref() == Some(&info.id);
        let engine_name = match info.engine {
            EngineKind::Whisper { .. } => "WhisperKit",
            _ => "FluidAudio",
        };
        let langs = if info.languages.iter().any(|l| l == "multilingual") { "99 languages" } else { "English" };
        let desc = info.description.trim_end_matches('.').to_string();
        let mut speed = div().flex().flex_none().items_center().gap(px(3.)).w(px(120.));
        for i in 0..5u8 {
            speed = speed.child(div().w(px(14.)).h(px(6.)).rounded(px(2.)).bg(if i < info.speed { c.ink } else { c.deboss_shade }));
        }
        let id = info.id.clone();
        let key = info.id.as_str().to_string();
        let confirm = self.confirm_delete.as_deref() == Some(key.as_str());

        let delete = {
            let id = id.clone();
            let key = key.clone();
            if confirm {
                Button::new(SharedString::from(format!("del-yes-{key}")), "Delete").small().danger().on_click(cx.listener(
                    move |this, _, _, cx| {
                        let id = id.clone();
                        this.confirm_delete = None;
                        this.model.update(cx, |m, cx| m.delete_model(id, cx));
                    },
                ))
            } else {
                Button::new(SharedString::from(format!("del-{key}")), "Delete").small().ghost().on_click(cx.listener(move |this, _, _, cx| {
                    this.confirm_delete = Some(key.clone());
                    cx.notify();
                }))
            }
        };

        let action: AnyElement = match &status {
            ModelStatus::Downloading { fraction, .. } => div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .w_full()
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(ui("Downloading", 12., 16., FontWeight::SEMIBOLD, c.ink))
                        .child(mono(format!("{:.0}%", fraction * 100.), 11., c.graphite)),
                )
                .child(Progress::new(*fraction))
                .into_any_element(),
            ModelStatus::NotDownloaded => Button::new(SharedString::from(format!("dl-{key}")), "Download")
                .disabled(!engine)
                .on_click(cx.listener(move |this, _, _, cx| {
                    let id = id.clone();
                    this.model.update(cx, |m, cx| m.download_model(id, cx));
                }))
                .into_any_element(),
            ModelStatus::Failed { message } => div()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(4.))
                .child(Button::new(SharedString::from(format!("retry-{key}")), "Try again").small().on_click(cx.listener(
                    move |this, _, _, cx| {
                        let id = id.clone();
                        this.model.update(cx, |m, cx| m.download_model(id, cx));
                    },
                )))
                .child(ui(message.clone(), 11., 14., FontWeight::NORMAL, c.danger).truncate().max_w(px(150.)))
                .into_any_element(),
            ModelStatus::Optimizing => ui("Optimizing", 12., 16., FontWeight::SEMIBOLD, c.graphite).into_any_element(),
            _ if !info.final_pass => div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(ui(if is_preview { "Live preview" } else { "Preview only" }, 12., 16., FontWeight::MEDIUM, c.graphite))
                .child(delete)
                .into_any_element(),
            _ => div()
                .flex()
                .items_center()
                .gap(px(4.))
                .when(!confirm, |d| {
                    d.child(Button::new(SharedString::from(format!("use-{key}")), "Use").small().on_click(cx.listener(move |this, _, _, cx| {
                        let id = id.clone();
                        this.model.update(cx, |m, cx| m.set_active_model(id, cx));
                    })))
                })
                .child(delete)
                .into_any_element(),
        };

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(16.))
            .h(px(64.))
            .border_t_1()
            .border_color(c.rule)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(text::title(info.name.clone(), 17., &c).line_height(px(22.)))
                    .child(ui(format!("{} · {engine_name} · {langs} · {desc}", info.vendor), 13., 16., FontWeight::NORMAL, c.graphite).truncate()),
            )
            .child(speed)
            .child(mono(format_size(info.size_bytes), 12., c.graphite).w(px(80.)).flex_none())
            .child(div().flex().flex_none().justify_end().w(px(150.)).child(action))
    }
}

impl Render for ModelsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c: Colors = cx.paper().colors;
        let (catalog, active, used, dir, engine_down) = {
            let m = self.model.read(cx);
            (m.catalog(), m.config.dictation.model.clone(), m.models_disk_bytes(), m.models_dir_display(), m.services.engine.is_none())
        };
        let model = self.model.clone();
        let used_label = if used == 0 { "Nothing downloaded".to_string() } else { format!("{} used", format_size(used)) };
        let disk = div()
            .id("models-dir")
            .cursor_pointer()
            .pb(px(4.))
            .on_click(move |_, _, cx| AppModel::open_path(&model.read(cx).paths.models_dir()))
            .child(ui(format!("{used_label} · {dir}"), 13., 16., FontWeight::NORMAL, c.graphite))
            .into_any_element();
        let head = kit::page_head(
            "Models",
            Some(kit::intro("Every model runs on your Mac’s Neural Engine. Nothing is sent anywhere.", 560., cx)),
            Some(disk),
            cx,
        );
        let more: Vec<ModelInfo> = catalog.into_iter().filter(|i| i.id != active).collect();
        let mut table = div().flex().flex_col().child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(16.))
                .h(px(36.))
                .child(caps("More models", 12., c.graphite).flex_1())
                .child(caps("Speed", 12., c.graphite).w(px(120.)).flex_none())
                .child(caps("Size", 12., c.graphite).w(px(80.)).flex_none())
                .child(div().w(px(150.)).flex_none()),
        );
        for info in &more {
            table = table.child(self.row(info, cx));
        }
        table = table.child(div().h(px(1.)).bg(c.rule));
        let hero = self.hero(cx);
        div().id("models").absolute().inset_0().overflow_y_scroll().text_color(c.ink).child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap(px(28.))
                .py(px(44.))
                .px(px(52.))
                .child(head)
                .when(engine_down, |d| {
                    d.child(Banner::new(
                        BannerKind::Warning,
                        "The speech engine is not running, so Sayso cannot download or load models. Restart Sayso. If this continues, check the engine path in Settings › Advanced.",
                    ))
                })
                .child(hero)
                .child(div().pt(px(8.)).child(table)),
        )
    }
}
