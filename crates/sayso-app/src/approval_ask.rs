//! The approval question (`docs/sdk.md`, "The question"): an app asks to use
//! Sayso. The host holds the reply of `hello` and shows one such question at a
//! time. **Don't allow**, Esc, and the close button answer no, and the host
//! keeps that. When each connection of the app closes, the window closes, and
//! the host keeps no answer.

use crate::model::AppModel;
use crate::question::{self, Texts};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_host::{ApprovalQuestion, Identity};
use sayso_i18n::t;
use sayso_protocol::Capability;
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::text;
use sayso_ui::theme::Colors;

/// A question and the way back to the thread of the host that waits for it.
pub struct Ask {
    pub question: ApprovalQuestion,
    /// True is **Allow**. A closed channel means that the question had no answer.
    pub answer: crossbeam_channel::Sender<bool>,
    /// A message here closes the window: the app closed, or the host closed.
    pub withdraw: async_channel::Receiver<()>,
}

/// Show each question that the host sends.
pub fn listen(model: &Entity<AppModel>, questions: async_channel::Receiver<Ask>, cx: &mut App) {
    let model = model.clone();
    cx.spawn(async move |cx| {
        while let Ok(ask) = questions.recv().await {
            cx.update(|cx| open(&model, ask, cx));
        }
    })
    .detach();
}

/// `--ask-approval`: the question with sample text, to see the window. The
/// answer goes to the log. `--ask-approval=interpreter` shows a script interpreter.
pub fn open_sample(model: &Entity<AppModel>, cx: &mut App) {
    let (answer, answered) = crossbeam_channel::bounded(1);
    let question = if crate::dev::arg("ask-approval").as_deref() == Some("interpreter") {
        ApprovalQuestion {
            app_id: Identity::Path("/usr/local/bin/python3".into()).id(),
            identity: Identity::Path("/usr/local/bin/python3".into()),
            name: "python3".into(),
            signer: None,
            claimed_name: Some("Meeting Recorder".into()),
            interpreter: true,
            capabilities: vec![Capability::AudioMic, Capability::TranscribeFile],
            approved: false,
        }
    } else {
        ApprovalQuestion {
            app_id: Identity::Signed { team_id: "MB5789APU7".into(), identifier: "app.justsayso.notes".into() }.id(),
            identity: Identity::Signed { team_id: "MB5789APU7".into(), identifier: "app.justsayso.notes".into() },
            name: "Sayso Notes".into(),
            signer: Some("Watzon Ventures LLC".into()),
            claimed_name: None,
            interpreter: false,
            capabilities: vec![Capability::AudioMic, Capability::AudioSystem, Capability::TranscribeFile],
            approved: false,
        }
    };
    std::thread::spawn(move || log::info!("approval question: {:?}", answered.recv().ok()));
    let (_never, withdraw) = async_channel::bounded(1);
    open(model, Ask { question, answer, withdraw }, cx);
}

/// The capabilities in the order that the question and Settings show them.
pub fn ordered(capabilities: &[Capability]) -> Vec<Capability> {
    const ORDER: [Capability; 7] = [Capability::AudioMic, Capability::AudioSystem, Capability::TranscribeFile, Capability::SessionLive, Capability::ModelsDownload, Capability::AiEnhance, Capability::DictationObserve];
    ORDER.into_iter().filter(|capability| capabilities.contains(capability)).collect()
}

/// A capability in plain words.
pub fn capability_text(capability: &Capability) -> String {
    match capability {
        Capability::ModelsRead => t!("apps.capability.models_read"),
        Capability::ModelsDownload => t!("apps.capability.models_download"),
        Capability::TranscribeFile => t!("apps.capability.transcribe_file"),
        Capability::SessionLive => t!("apps.capability.session_live"),
        Capability::AudioMic => t!("apps.capability.audio_mic"),
        Capability::AudioSystem => t!("apps.capability.audio_system"),
        Capability::AiEnhance => t!("apps.capability.ai_enhance"),
        Capability::DictationObserve => t!("apps.capability.dictation_observe"),
        // The host does not ask for a name that it does not know.
        Capability::Other(name) => name.clone(),
    }
}

fn capability_icon(capability: &Capability) -> Icon {
    match capability {
        Capability::AudioMic => Icon::Mic,
        Capability::AudioSystem => Icon::Speaker,
        Capability::TranscribeFile => Icon::File,
        Capability::DictationObserve | Capability::ModelsRead | Capability::Other(_) => Icon::Eye,
        Capability::SessionLive => Icon::Activity,
        Capability::ModelsDownload => Icon::Download,
        Capability::AiEnhance => Icon::Sparkle,
    }
}

/// The team and the identifier, or the path, as the question and Settings show them.
pub fn identity_text(identity: &Identity) -> String {
    match identity {
        Identity::Signed { team_id, identifier } => format!("{identifier} · {team_id}"),
        Identity::Path(path) => sayso_host::visible_path(path),
    }
}

fn identity_box(q: &ApprovalQuestion, c: &Colors) -> AnyElement {
    let signed = matches!(q.identity, Identity::Signed { .. });
    let (mark, mark_color, name) = if signed {
        let name = q.signer.as_ref().map_or_else(|| t!("apps.approval.signed"), |org| t!("apps.approval.signed_by", org = org));
        (Icon::ShieldCheck, c.success, name)
    } else {
        (Icon::Warning, c.danger, t!("apps.approval.not_signed"))
    };
    let head = div().flex().items_center().gap(px(12.)).child(icon(mark, 18., mark_color)).child(
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(2.))
            .child(text::ui(name, 14., FontWeight::SEMIBOLD, c.ink).line_height(px(18.)))
            .child(text::mono(identity_text(&q.identity), 12., c.graphite).line_height(px(16.))),
    );
    let note = |text_color: Hsla, line: String| div().pl(px(30.)).child(text::ui(line, 13., FontWeight::NORMAL, text_color).line_height(px(19.)));
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .py(px(12.))
        .px(px(14.))
        .rounded(px(10.))
        .debossed(c)
        .child(head)
        .when(q.interpreter, |d| d.child(note(c.ink, t!("apps.approval.interpreter"))))
        .when_some(q.claimed_name.clone(), |d, name| d.child(note(c.graphite, t!("apps.approval.claimed", name = name))))
        .into_any_element()
}

fn capability_list(q: &ApprovalQuestion, c: &Colors) -> AnyElement {
    let mut shown = ordered(&q.capabilities);
    // An app that asks for nothing still gets the list of the models.
    if shown.is_empty() {
        shown.push(Capability::ModelsRead);
    }
    let rows = shown.iter().map(|capability| {
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(icon(capability_icon(capability), 16., c.ink))
            .child(text::ui(capability_text(capability), 15., FontWeight::NORMAL, c.ink).line_height(px(20.)))
    });
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(text::ui(t!("apps.approval.asks_to"), 12., FontWeight::SEMIBOLD, c.graphite).line_height(px(16.)))
        .child(div().flex().flex_col().gap(px(9.)).children(rows))
        .into_any_element()
}

fn texts(q: &ApprovalQuestion) -> Texts {
    let details = q.clone();
    Texts {
        window_title: t!("apps.approval.window_title"),
        title: if q.approved { t!("apps.approval.title_more", app = q.name) } else { t!("apps.approval.title", app = q.name) },
        body: if q.approved { t!("apps.approval.body_more") } else { t!("apps.approval.body") },
        details: Some(Box::new(move |c| div().flex().flex_col().gap(px(20.)).child(identity_box(&details, c)).child(capability_list(&details, c)).into_any_element())),
        quote: None,
        cancel: t!("apps.approval.dont_allow"),
        confirm: t!("apps.approval.allow"),
        danger: false,
        guarded: true,
    }
}

/// The lines of `text` in a column that is `width` wide. An estimate: Latin
/// letters are about half as wide as their size (the display face of a title
/// is narrower), and CJK letters are as wide as their size.
fn lines(text: &str, size: f32, width: f32) -> f32 {
    let latin = if size > 20. { 0.48 } else { 0.56 };
    let total: f32 = text.chars().map(|c| if u32::from(c) >= 0x1100 { size } else { size * latin }).sum();
    (total / width).ceil().max(1.)
}

/// The height of the window, so all of its text shows in each language. The
/// content scrolls when the estimate is short.
fn window_height(q: &ApprovalQuestion, t: &Texts) -> f32 {
    // The column inside the window: 520 wide, 10 of margin, and 44 of padding.
    const COLUMN: f32 = 412.;
    // The margins, the top bar, the padding, and the buttons.
    const FRAME: f32 = 20. + 52. + 8. + 24. + 68.;
    let heading = lines(&t.title, 28., COLUMN) * 34. + 10. + lines(&t.body, 15., COLUMN) * 22.;
    let identity = 24. + 36. + if q.interpreter { 10. + lines(&t!("apps.approval.interpreter"), 13., COLUMN - 58.) * 19. } else { 0. } + q.claimed_name.as_ref().map_or(0., |name| 10. + lines(&t!("apps.approval.claimed", name = name), 13., COLUMN - 58.) * 19.);
    let path_extra = (lines(&identity_text(&q.identity), 12., COLUMN - 58.) - 1.) * 16.;
    let shown = ordered(&q.capabilities);
    let rows: f32 = shown.iter().map(|capability| lines(&capability_text(capability), 15., COLUMN - 26.) * 20.).sum::<f32>().max(20.);
    let list = 16. + 8. + rows + 9. * (shown.len().max(1) as f32 - 1.);
    (FRAME + heading + 20. + identity + path_extra + 20. + list + crate::caption::top_margin(10.) - 10.).ceil()
}

fn open(model: &Entity<AppModel>, ask: Ask, cx: &mut App) {
    let Ask { question, answer, withdraw } = ask;
    let height = window_height(&question, &texts(&question));
    let opened = question::open(model, "approval-ask", size(px(520.), px(height)), move || texts(&question), move |yes, _| {
        let _ = answer.send(yes);
    }, cx);
    match opened {
        Ok(handle) => {
            // A message closes the window. A closed channel means that the
            // question has its answer.
            cx.spawn(async move |cx| {
                if withdraw.recv().await.is_ok() {
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                }
            })
            .detach();
        }
        // The window is gone, and so is the sender in it: the host keeps no answer.
        Err(e) => log::error!("could not open the approval question: {e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{ApprovalQuestion, Capability, Identity, capability_text, identity_text, ordered, texts, window_height};

    fn signed(capabilities: Vec<Capability>) -> ApprovalQuestion {
        ApprovalQuestion {
            app_id: "T/i".into(),
            identity: Identity::Signed { team_id: "MB5789APU7".into(), identifier: "app.justsayso.notes".into() },
            name: "Sayso Notes".into(),
            signer: Some("Watzon Ventures LLC".into()),
            claimed_name: None,
            interpreter: false,
            capabilities,
            approved: false,
        }
    }

    #[test]
    fn each_capability_has_words_and_the_order_is_the_same() {
        sayso_i18n::set_locale("en");
        assert_eq!(capability_text(&Capability::AudioMic), "Record from your microphone");
        assert_eq!(capability_text(&Capability::AudioSystem), "Record the sound of other apps and calls");
        assert_eq!(capability_text(&Capability::DictationObserve), "See each dictation after it is inserted");
        assert_eq!(ordered(&[Capability::DictationObserve, Capability::ModelsRead, Capability::AudioMic]), vec![Capability::AudioMic, Capability::DictationObserve]);
    }

    #[test]
    fn the_path_shows_each_character_that_can_mislead_and_the_identity_keeps_it() {
        let identity = Identity::Path("/opt/ev\u{202E}il\nname".into());
        assert_eq!(identity_text(&identity), "/opt/ev\u{FFFD}il\u{FFFD}name");
        assert_eq!(identity, Identity::Path("/opt/ev\u{202E}il\nname".into()));
    }

    #[test]
    fn the_window_height_is_near_the_design_and_grows_with_the_text() {
        sayso_i18n::set_locale("en");
        let q = signed(vec![Capability::AudioMic, Capability::AudioSystem, Capability::TranscribeFile]);
        let height = window_height(&q, &texts(&q));
        assert!((440. ..=510.).contains(&height), "{height}");
        let mut python = q.clone();
        python.identity = Identity::Path("/usr/local/bin/python3".into());
        python.interpreter = true;
        python.claimed_name = Some("Meeting Recorder".into());
        python.capabilities.truncate(2);
        let taller = window_height(&python, &texts(&python));
        assert!(taller > height, "{taller} {height}");
    }
}
