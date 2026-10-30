//! A small window that asks the user one question with two buttons: the
//! download question of an app (`download_ask`), the approval question
//! (`approval_ask`), and the quit question (`quit`). It is a window of its own, like "What is new": it does not need
//! the Hub, so it shows also when the Hub is closed.
//!
//! Each answer comes once. Cancel, Esc, and the close button of the window
//! answer no.

use crate::model::AppModel;
use gpui_kit::*;
use sayso_ui::fonts::{Face, LocaleFonts as _};
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::theme::Colors;
use sayso_ui::texture::{Grain, grain};
use sayso_ui::{ActivePaper, text};
use std::cell::RefCell;
use std::rc::Rc;

/// The text of a question. The window reads it at each render, so it follows
/// a change of language.
pub struct Texts {
    pub window_title: String,
    pub title: String,
    pub body: String,
    /// More of the question, under its text: the identity of an app and what it asks for.
    pub details: Option<Details>,
    /// Text that the question quotes, with a label above it: for example the
    /// reason that an app gives.
    pub quote: Option<(String, String)>,
    pub cancel: String,
    pub confirm: String,
    /// The confirm button stops work of the user: it shows as a danger.
    pub danger: bool,
}

pub type Details = Box<dyn FnOnce(&Colors) -> AnyElement>;

type AnswerFn = Box<dyn FnOnce(bool, &mut App)>;

/// The answer goes out once: the first click, Esc, or close counts.
#[derive(Clone)]
struct Answer(Rc<RefCell<Option<AnswerFn>>>);

impl Answer {
    fn give(&self, yes: bool, cx: &mut App) {
        if let Some(answer) = self.0.borrow_mut().take() {
            answer(yes, cx);
        }
    }
}

pub struct QuestionView {
    id: &'static str,
    texts: Box<dyn Fn() -> Texts>,
    answer: Answer,
}

/// Open the window of a question. `answer` gets true for the confirm
/// button, and false for Cancel, Esc, and the close button.
pub fn open(
    model: &Entity<AppModel>,
    id: &'static str,
    window_size: Size<Pixels>,
    texts: impl Fn() -> Texts + 'static,
    answer: impl FnOnce(bool, &mut App) + 'static,
    cx: &mut App,
) -> anyhow::Result<AnyWindowHandle> {
    let bounds = Bounds::centered(None, window_size, cx);
    let window_title = texts().window_title;
    let answer = Answer(Rc::new(RefCell::new(Some(Box::new(answer)))));
    let on_close = answer.clone();
    let (handle, _) = gpui_kit::open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some(window_title.into()), appears_transparent: true, traffic_light_position: Some(point(px(18.), px(18.))) }),
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
            window.on_window_should_close(cx, move |_, cx| {
                on_close.give(false, cx);
                true
            });
            cx.new(|_| QuestionView { id, texts: Box::new(texts), answer })
        },
    )?;
    crate::app::appkit_later(cx, crate::shell::show_app_and_activate);
    Ok(handle)
}

impl QuestionView {
    fn respond(&self, yes: bool, window: &mut Window, cx: &mut App) {
        self.answer.give(yes, cx);
        window.remove_window();
    }

    fn top_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let header = div().id(SharedString::from(format!("{}-title", self.id))).flex().flex_none().items_center().justify_end().h(px(52.)).px(px(18.));
        if crate::chrome::is_custom(window) {
            // The custom title bar (Linux): the header moves the window.
            let answer = self.answer.clone();
            return crate::chrome::drag_area(header)
                .child(crate::chrome::window_controls(window, cx, move |window, cx| {
                    answer.give(false, cx);
                    window.remove_window();
                }))
                .into_any_element();
        }
        header.into_any_element()
    }
}

impl Render for QuestionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let texts = (self.texts)();
        // The title follows a change of language.
        window.set_window_title(&texts.window_title);
        let heading = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(text::title(texts.title, 28., &c).line_height(px(34.)))
            .child(text::ui(texts.body, 15., FontWeight::NORMAL, c.graphite).line_height(px(22.)));
        let details = texts.details.map(|details| details(&c));
        let quote = texts.quote.map(|(label, quoted)| {
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(text::ui(label, 12., FontWeight::SEMIBOLD, c.graphite).line_height(px(16.)))
                .child(
                    div()
                        .py(px(10.))
                        .px(px(14.))
                        .rounded(px(10.))
                        .debossed(&c)
                        .child(text::ui(quoted, 14., FontWeight::NORMAL, c.ink).line_height(px(20.))),
                )
        });
        let confirm = Button::new(SharedString::from(format!("{}-confirm", self.id)), texts.confirm).large();
        let confirm = if texts.danger { confirm.danger() } else { confirm.primary() };
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
            .child(Button::new(SharedString::from(format!("{}-cancel", self.id)), texts.cancel).large().on_click(cx.listener(|this, _, window, cx| this.respond(false, window, cx))))
            .child(confirm.on_click(cx.listener(|this, _, window, cx| this.respond(true, window, cx))));
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
                    this.respond(false, window, cx);
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
                    .child(div().id(SharedString::from(format!("{}-content", self.id))).flex().flex_col().flex_1().min_h_0().gap(px(20.)).px(px(44.)).pt(px(8.)).pb(px(24.)).overflow_y_scroll().child(heading).children(details).children(quote))
                    .child(footer),
            )
            .children(crate::caption::caption_bar(false, cx))
    }
}
