//! The question that an extension asks before it downloads a model
//! (`docs/extensions.md`, "Requests", `download_model`).
//!
//! The host runs on its own threads and waits for the answer. Each question
//! is a small window of its own, like "What is new". It does not need the Hub,
//! so it shows also when the Hub is closed, and a question from one extension
//! does not wait for the Hub or for a question from another extension.
//! Closing the window without a click, or quitting Sayso, answers no.

use crate::model::AppModel;
use gpui_kit::*;
use sayso_core::models::format_size;
use sayso_host::DownloadQuestion;
use sayso_i18n::t;
use sayso_ui::fonts::{Face, LocaleFonts as _};
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::texture::{Grain, grain};
use sayso_ui::{ActivePaper, text};

/// A question and the way back to the thread of the host that waits for it.
pub struct Ask {
    pub question: DownloadQuestion,
    /// True means download. The host also takes a closed channel as no.
    pub answer: crossbeam_channel::Sender<bool>,
}

/// Show each question that the host sends, as it arrives.
pub fn listen(model: &Entity<AppModel>, questions: async_channel::Receiver<Ask>, cx: &mut App) {
    let model = model.clone();
    cx.spawn(async move |cx| {
        while let Ok(ask) = questions.recv().await {
            cx.update(|cx| open(&model, ask, cx));
        }
    })
    .detach();
}

/// `--ask-download`: the question with sample text, to see the window. The
/// answer goes to the log.
pub fn open_sample(model: &Entity<AppModel>, cx: &mut App) {
    let (answer, answered) = crossbeam_channel::bounded(1);
    let question = DownloadQuestion {
        extension_id: "dev.example.notetaker".into(),
        extension_name: "dev.example.notetaker".into(),
        model_id: "parakeet-tdt-0.6b-v3".into(),
        model_name: "Parakeet TDT v3".into(),
        size_bytes: 640_000_000,
        reason: "Notetaker needs a speech model to write the text of your meetings on this computer.".into(),
    };
    std::thread::spawn(move || log::info!("download question: the user said {}", if answered.recv().unwrap_or(false) { "yes" } else { "no" }));
    open(model, Ask { question, answer }, cx);
}

fn open(model: &Entity<AppModel>, ask: Ask, cx: &mut App) {
    let Ask { question, answer } = ask;
    let bounds = Bounds::centered(None, size(px(520.), px(380.)), cx);
    let on_close = answer.clone();
    let title = t!("extensions.download_ask.window_title");
    let opened = gpui_kit::open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some(title.into()), appears_transparent: true, traffic_light_position: Some(point(px(18.), px(18.))) }),
            is_resizable: false,
            focus: true,
            // The window class on Linux, which matches the desktop entry.
            app_id: Some(crate::shell::APP_ID.into()),
            window_decorations: crate::chrome::decorations(&model.read(cx).config),
            window_background: crate::chrome::background(),
            show: true,
            ..Default::default()
        },
        cx,
        |window, cx| {
            // The close button of the window is a no.
            window.on_window_should_close(cx, move |_, _| {
                let _ = on_close.send(false);
                true
            });
            cx.new(|_| DownloadAskView { question, answer })
        },
    );
    match opened {
        Ok(_) => crate::app::appkit_later(cx, crate::shell::show_app_and_activate),
        // The window is gone, and so is the sender in it: the host takes it as no.
        Err(e) => log::error!("could not open the download question: {e:#}"),
    }
}

struct DownloadAskView {
    question: DownloadQuestion,
    answer: crossbeam_channel::Sender<bool>,
}

impl DownloadAskView {
    fn respond(&self, yes: bool, window: &mut Window) {
        let _ = self.answer.send(yes);
        window.remove_window();
    }

    fn top_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let header = div().id("download-ask-title").flex().flex_none().items_center().justify_end().h(px(52.)).px(px(18.));
        if crate::chrome::is_custom(window) {
            // The custom title bar (Linux): the header moves the window.
            let answer = self.answer.clone();
            return crate::chrome::drag_area(header)
                .child(crate::chrome::window_controls(window, cx, move |window, _| {
                    let _ = answer.send(false);
                    window.remove_window();
                }))
                .into_any_element();
        }
        header.into_any_element()
    }
}

impl Render for DownloadAskView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        // The title follows a change of language.
        window.set_window_title(&t!("extensions.download_ask.window_title"));
        let q = &self.question;
        let heading = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(text::title(t!("extensions.download_ask.title", extension = q.extension_name), 28., &c).line_height(px(34.)))
            .child(text::ui(t!("extensions.download_ask.body", model = q.model_name, size = format_size(q.size_bytes)), 15., FontWeight::NORMAL, c.graphite).line_height(px(22.)));
        // The reason is text from the extension: the label says so.
        let reason = (!q.reason.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(text::ui(t!("extensions.download_ask.reason_label", extension = q.extension_name), 12., FontWeight::SEMIBOLD, c.graphite).line_height(px(16.)))
                .child(
                    div()
                        .py(px(10.))
                        .px(px(14.))
                        .rounded(px(10.))
                        .debossed(&c)
                        .child(text::ui(q.reason.clone(), 14., FontWeight::NORMAL, c.ink).line_height(px(20.))),
                )
        });
        let footer = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_end()
            .gap(px(10.))
            .h(px(68.))
            .px(px(24.))
            .border_t_1()
            .border_color(c.rule)
            .child(Button::new("download-ask-cancel", t!("extensions.download_ask.cancel")).large().on_click(cx.listener(|this, _, window, _| this.respond(false, window))))
            .child(Button::new("download-ask-download", t!("extensions.download_ask.download")).primary().large().on_click(cx.listener(|this, _, window, _| this.respond(true, window))));
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .p(px(10.))
            .pt(px(crate::caption::top_margin(10.)))
            .bg(c.ground)
            .font_family(sayso_ui::fonts::UI)
            .locale_fonts(Face::Sans)
            .text_color(c.ink)
            // Esc is a no.
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.respond(false, window);
                }
            }))
            .child(grain(Grain::Board, px(0.), cx))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .rounded(px(12.))
                    .bg(c.sheet)
                    .shadow(sayso_ui::paper::sheet(&c))
                    .child(grain(Grain::Sheet, px(12.), cx))
                    .child(self.top_bar(window, cx))
                    .child(div().flex().flex_col().flex_1().min_h_0().gap(px(20.)).px(px(44.)).pt(px(8.)).pb(px(24.)).child(heading).children(reason))
                    .child(footer),
            )
            .children(crate::caption::caption_bar(false, cx))
    }
}
