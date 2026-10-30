//! The question that an app asks before it downloads a model
//! (`docs/extensions.md`, "Requests", `download_model`).
//!
//! The host runs on its own threads and waits for the answer. Each question
//! is a small window of its own (`question`), so a question from one app does
//! not wait for the Hub or for a question from another app. Closing the window
//! without a click, or quitting Sayso, answers no. When the app disconnects
//! before the user answers, the window closes, and that also counts as no.

use crate::model::AppModel;
use crate::question::{self, Texts};
use gpui_kit::*;
use sayso_core::models::format_size;
use sayso_host::DownloadQuestion;
use sayso_i18n::t;

/// A question and the way back to the thread of the host that waits for it.
pub struct Ask {
    pub question: DownloadQuestion,
    /// True means download. The host also takes a closed channel as no.
    pub answer: crossbeam_channel::Sender<bool>,
    /// A message here closes the window: the app disconnected, or the
    /// host closed. The host takes that as no.
    pub withdraw: async_channel::Receiver<()>,
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
        app_id: "dev.example.notetaker".into(),
        app_name: "Notetaker".into(),
        model_id: "parakeet-tdt-0.6b-v3".into(),
        model_name: "Parakeet TDT v3".into(),
        size_bytes: 640_000_000,
        reason: "Notetaker needs a speech model to write the text of your meetings on this computer.".into(),
    };
    std::thread::spawn(move || log::info!("download question: the user said {}", if answered.recv().unwrap_or(false) { "yes" } else { "no" }));
    let (_never, withdraw) = async_channel::bounded(1);
    open(model, Ask { question, answer, withdraw }, cx);
}

fn texts(q: &DownloadQuestion) -> Texts {
    Texts {
        window_title: t!("apps.download_ask.window_title"),
        title: t!("apps.download_ask.title", app = q.app_name),
        body: t!("apps.download_ask.body", model = q.model_name, size = format_size(q.size_bytes)),
        details: None,
        // The reason is text from the app: the label says so.
        quote: (!q.reason.is_empty()).then(|| (t!("apps.download_ask.reason_label", app = q.app_name), q.reason.clone())),
        cancel: t!("apps.download_ask.cancel"),
        confirm: t!("apps.download_ask.download"),
        danger: false,
    }
}

fn open(model: &Entity<AppModel>, ask: Ask, cx: &mut App) {
    let Ask { question, answer, withdraw } = ask;
    let opened = question::open(model, "download-ask", size(px(520.), px(380.)), move || texts(&question), move |yes, _| {
        let _ = answer.send(yes);
    }, cx);
    match opened {
        Ok(handle) => {
            // A message closes the window. A closed channel means the
            // question has its answer, and nothing is to do.
            cx.spawn(async move |cx| {
                if withdraw.recv().await.is_ok() {
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                }
            })
            .detach();
        }
        // The window is gone, and so is the sender in it: the host takes it as no.
        Err(e) => log::error!("could not open the download question: {e:#}"),
    }
}
