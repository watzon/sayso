//! The models page. See the Paper board "Hub — Models".
//!
//! The active model as a hero card with real numbers, then the local catalog
//! in groups with a speed meter, the size, and one action per row, then the
//! cloud providers with their models.

use super::kit::{self, MenuItem, caps, mono, ui};
use crate::model::AppModel;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::config::is_local_url;
use sayso_core::config::PreviewChoice;
use sayso_core::models::{EngineKind, Family, ModelId, ModelInfo, format_size};
use sayso_core::speech::{SpeechProvider, SpeechProviderKind};
use sayso_core::stt::ModelStatus;
use sayso_i18n::t;
use sayso_ui::a11y::{A11yExt as _, LiveStatus, Pressable, Role};
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::{ActivePaper, Colors, text};
use std::collections::HashMap;

/// "632 MB", or "macOS" for Apple Speech, whose files macOS downloads and stores.
pub fn size_label(info: &ModelInfo) -> String {
    if info.size_bytes == 0 { "macOS".into() } else { format_size(info.size_bytes) }
}

/// The groups of the local catalog, in display order.
pub const LOCAL_GROUPS: [Family; 3] = [Family::Parakeet, Family::Whisper, Family::Other];

/// The title of a group, in the interface language.
pub fn group_title(family: Family) -> String {
    match family {
        Family::Parakeet => "Parakeet".into(),
        Family::Whisper => "Whisper".into(),
        Family::Other | Family::Cloud => t!("models.group_more"),
    }
}

/// Five bars, `level` of them in ink.
pub fn meter(level: u8, c: &Colors) -> Div {
    let mut meter = div().flex().flex_none().items_center().gap(px(3.));
    for i in 0..5u8 {
        meter = meter.child(div().w(px(14.)).h(px(6.)).rounded(px(2.)).bg(if i < level { c.ink } else { c.deboss_shade }));
    }
    meter
}

/// The speed meter: five bars, `speed` of them in ink.
pub fn speed_meter(speed: u8, c: &Colors) -> Div {
    meter(speed, c)
}

/// The accuracy meter level for a word error rate. The steps are in
/// `docs/model-roles.md`.
pub fn accuracy_level(wer: f32) -> u8 {
    match wer {
        w if w < 6.5 => 5,
        w if w < 7.0 => 4,
        w if w < 8.5 => 3,
        w if w < 10.0 => 2,
        _ => 1,
    }
}

/// "94.1%": the share of words the model gets right.
pub fn accuracy_label(wer: f32) -> String {
    t!("models.accuracy_percent", value = sayso_i18n::numbers::decimal((100. - wer) as f64, 1))
}

/// "160 ms" or "1.1 s".
fn interval_label(ms: u32) -> String {
    if ms >= 1000 { format!("{} s", sayso_i18n::numbers::decimal(ms as f64 / 1000., 1)) } else { format!("{ms} ms") }
}

/// A large number over a small note, in the setup card.
fn stat(value: String, note: String, c: &Colors) -> Div {
    div().flex().flex_col().gap(px(2.)).child(text::figure(value, 22., c).line_height(px(28.))).child(ui(note, 12., 16., FontWeight::NORMAL, c.graphite))
}

/// [`text::title`] without a node, for the inside of a named control.
fn plain_title(name: String, size: f32, c: &Colors) -> Div {
    use sayso_ui::fonts::{DISPLAY, Face, LocaleFonts as _};
    div().font_family(DISPLAY).locale_fonts(Face::Serif).text_size(px(size)).font_weight(FontWeight::SEMIBOLD).text_color(c.ink).child(name)
}

/// What a screen reader hears when the state of a model changes. It has no
/// percent, so it changes only when the state does.
fn status_text(name: &str, status: &ModelStatus) -> String {
    match status {
        ModelStatus::Downloading { .. } => t!("models.a11y.downloading", name = name),
        ModelStatus::Optimizing => t!("models.a11y.optimizing", name = name),
        ModelStatus::Failed { message, reason } => t!("models.a11y.failed", name = name, reason = crate::engine_text::describe(*reason, message)),
        ModelStatus::Ready | ModelStatus::Downloaded => t!("models.a11y.ready", name = name),
        ModelStatus::NotDownloaded => String::new(),
    }
}

/// The tag of a model that has a role.
fn role_pill(label: String, c: &Colors) -> Div {
    div().flex().flex_none().items_center().h(px(28.)).px(px(12.)).rounded_full().bg(c.accent_wash).child(ui(label, 13., 16., FontWeight::SEMIBOLD, c.accent))
}

// The column widths of the lists.
const ACCURACY_W: f32 = 176.;
const SPEED_W: f32 = 84.;
const SIZE_W: f32 = 64.;
const ACTION_W: f32 = 132.;

/// The result of a provider check, shown on the provider's line.
#[derive(Clone)]
enum Test {
    Running,
    Passed(u64),
    Failed(String),
}

/// The "Add a cloud provider" form.
struct AddProvider {
    kind: SpeechProviderKind,
    /// Custom endpoint only.
    name: Entity<InputState>,
    url: Entity<InputState>,
    model_id: Entity<InputState>,
    key: Entity<InputState>,
    error: Option<AddError>,
}

/// Why the "Add a cloud provider" form cannot save. It keeps the reason, not
/// the sentence, so it changes with the language.
enum AddError {
    Name,
    Url,
    ModelId,
    ServerKey,
    Key(String),
}

impl AddError {
    fn text(&self) -> String {
        match self {
            AddError::Name => t!("models.add.error_name"),
            AddError::Url => t!("models.add.error_url"),
            AddError::ModelId => t!("models.add.error_model_id"),
            AddError::ServerKey => t!("models.add.error_server_key"),
            AddError::Key(provider) => t!("models.add.error_key", provider = provider),
        }
    }
}

/// The placeholders of the form. The inputs keep them, so `ModelsPage` sets
/// them again when the language changes.
fn key_placeholder() -> String {
    t!(crate::shell::os_text!("models.add.key_placeholder_mac", "models.add.key_placeholder_windows", "models.add.key_placeholder_other"))
}

/// The order of the model lists.
#[derive(Clone, Copy, PartialEq)]
enum SortKey {
    /// The catalog order, the recommended model first.
    Recommended,
    Accuracy,
    Speed,
    Size,
}

impl SortKey {
    const ALL: [SortKey; 4] = [SortKey::Recommended, SortKey::Accuracy, SortKey::Speed, SortKey::Size];

    fn label(self) -> String {
        match self {
            SortKey::Recommended => t!("models.sort.recommended"),
            SortKey::Accuracy => t!("models.group.accuracy"),
            SortKey::Speed => t!("models.group.speed"),
            SortKey::Size => t!("models.group.size"),
        }
    }
}

/// Sort the best first: the most accurate, the fastest, the smallest. `reversed`
/// turns the order around, but models without an accuracy figure or a size stay last.
fn sort_models(list: &mut [ModelInfo], key: SortKey, reversed: bool) {
    use std::cmp::Ordering;
    list.sort_by(|a, b| {
        let order = match key {
            SortKey::Recommended => b.recommended.cmp(&a.recommended),
            SortKey::Accuracy => match (a.wer_percent, b.wer_percent) {
                (Some(x), Some(y)) => x.total_cmp(&y),
                (Some(_), None) => return Ordering::Less,
                (None, Some(_)) => return Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
            SortKey::Speed => b.speed.cmp(&a.speed),
            // Apple Speech has no size here (macOS keeps its files), so it goes last.
            SortKey::Size => match (a.size_bytes, b.size_bytes) {
                (0, 0) => Ordering::Equal,
                (_, 0) => return Ordering::Less,
                (0, _) => return Ordering::Greater,
                (x, y) => x.cmp(&y),
            },
        };
        if reversed { order.reverse() } else { order }
    });
}

/// True when the model's name, vendor, languages, or description has `query`
/// (lowercase). An empty query matches every model.
fn matches(info: &ModelInfo, query: &str) -> bool {
    query.is_empty()
        || [info.name.as_str(), info.vendor.as_str(), &info.language_label(), &info.description].iter().any(|s| s.to_lowercase().contains(query))
}

/// The final model and the live preview model. The page reads them once per
/// render, not once per row.
struct Roles {
    active: ModelId,
    preview: Option<ModelId>,
}

impl Roles {
    fn of(m: &AppModel) -> Self {
        Self { active: m.active_model().id, preview: m.preview_model() }
    }

    /// "Final text", "Live preview", or "In use" for a model with both roles.
    fn tag(&self, id: &ModelId) -> Option<String> {
        let is_final = &self.active == id;
        let is_preview = self.preview.as_ref() == Some(id);
        match (is_final, is_preview) {
            (true, true) => Some(t!("models.row.in_use")),
            (true, false) => Some(t!("models.setup.final")),
            (false, true) => Some(t!("models.setup.preview")),
            (false, false) => None,
        }
    }
}

/// Which floating menu of the page is open.
#[derive(Clone, PartialEq)]
enum OpenMenu {
    Final,
    Preview,
    Sort,
    /// The "Use" menu in the row of a streaming model.
    Use(ModelId),
}

/// The state of the live preview slot.
enum PreviewSlot {
    Off,
    /// The chosen model does not understand the dictation language.
    Paused(ModelInfo),
    /// The final model also gives the preview.
    Same(ModelInfo),
    Model(ModelInfo),
    /// Automatic, and no streaming model is on disk.
    Unavailable,
}

pub struct ModelsPage {
    model: Entity<AppModel>,
    confirm_delete: Option<String>,
    add: Option<AddProvider>,
    tests: HashMap<String, Test>,
    confirm_remove: Option<String>,
    menu: Option<OpenMenu>,
    /// The model in the details modal.
    details: Option<ModelId>,
    search: Entity<InputState>,
    sort: SortKey,
    reversed: bool,
    focus: FocusHandle,
    /// The page has no room for all columns (set at each render).
    narrow: bool,
    /// The locale of the text in the placeholders of the add form.
    locale: String,
    _observe: Subscription,
    _subs: Vec<Subscription>,
}

impl ModelsPage {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("models.search.placeholder")));
        let subs = vec![cx.subscribe_in(&search, window, |_, _, _: &InputEvent, _, cx| cx.notify())];
        Self {
            model,
            confirm_delete: None,
            add: None,
            tests: HashMap::new(),
            confirm_remove: None,
            menu: None,
            details: None,
            search,
            sort: SortKey::Recommended,
            reversed: false,
            focus: cx.focus_handle(),
            narrow: false,
            locale: sayso_i18n::locale(),
            _observe: observe,
            _subs: subs,
        }
    }

    fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_lowercase()
    }

    /// Sort by `key`. The key that is already active turns its order around.
    fn set_sort(&mut self, key: SortKey, cx: &mut Context<Self>) {
        self.reversed = key == self.sort && key != SortKey::Recommended && !self.reversed;
        self.sort = key;
        self.menu = None;
        cx.notify();
    }

    /// The search field and the sort menu, above the lists.
    fn toolbar(&self, cx: &mut Context<Self>) -> Div {
        let menu = (self.menu == Some(OpenMenu::Sort)).then(|| {
            let items = SortKey::ALL.iter().map(|k| MenuItem::new(k.label()).selected(*k == self.sort)).collect();
            let page = cx.entity().downgrade();
            let pick = {
                let page = page.clone();
                move |i: usize, _: &mut Window, cx: &mut App| {
                    let _ = page.update(cx, |this, cx| {
                        // A pick from the menu always gives the best first.
                        this.reversed = false;
                        this.sort = SortKey::ALL[i];
                        this.menu = None;
                        cx.notify();
                    });
                }
            };
            div().absolute().top_full().right_0().w(px(200.)).child(kit::menu_items("sort-menu", items, pick, Self::close_menu(page), cx)).into_any_element()
        });
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(16.))
            .child(kit::search_well_named(&self.search, Some(t!("models.search.placeholder").into()), 34., None, cx).w(px(300.)))
            .child(
                div()
                    .relative()
                    .flex_none()
                    .child(Button::new("sort-models", t!("models.sort.button", key = self.sort.label())).small().expanded(self.menu == Some(OpenMenu::Sort)).on_click(cx.listener(|this, _, _, cx| this.toggle_menu(OpenMenu::Sort, cx))))
                    .children(menu),
            )
    }

    /// A column caption that sorts the lists. The active one shows its direction.
    fn sort_caption(&self, key: SortKey, width: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let active = self.sort == key;
        let label = if active { format!("{} {}", key.label(), if self.reversed { "↑" } else { "↓" }) } else { key.label() };
        Pressable::new(SharedString::from(format!("sort-{}", key.label())))
            .label(t!("models.a11y.sort_by", key = key.label()))
            .w(px(width))
            .flex_none()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.set_sort(key, cx)))
            .child(kit::plain(label.to_uppercase(), 11., 16., FontWeight::MEDIUM, if active { c.ink } else { c.graphite }))
    }

    fn open_details(&mut self, id: ModelId, window: &mut Window, cx: &mut Context<Self>) {
        self.details = Some(id);
        self.confirm_delete = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn toggle_menu(&mut self, menu: OpenMenu, cx: &mut Context<Self>) {
        self.menu = if self.menu.as_ref() == Some(&menu) { None } else { Some(menu) };
        cx.notify();
    }

    fn close_menu(page: WeakEntity<Self>) -> impl Fn(&mut Window, &mut App) + 'static {
        move |_, cx| {
            let _ = page.update(cx, |this, cx| {
                this.menu = None;
                cx.notify();
            });
        }
    }

    // -----------------------------------------------------------------------
    // Setup card
    // -----------------------------------------------------------------------

    /// The final model and the preview model, side by side.
    fn setup(&self, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let final_slot = self.final_slot(cx);
        let preview_slot = self.preview_slot(cx);
        let narrow = self.narrow;
        div()
            .flex()
            .flex_none()
            .when(narrow, |d| d.flex_col())
            .rounded(px(16.))
            .bg(c.sheet_raised)
            .shadow(vec![
                BoxShadow::new(px(0.), px(1.), c.highlight(1.0)).inset(),
                BoxShadow::new(px(0.), px(1.), c.shadow(0.14)).blur_radius(px(2.)),
                BoxShadow::new(px(0.), px(12.), c.shadow(0.10)).blur_radius(px(28.)),
            ])
            .child(final_slot.flex_grow(1.25).flex_basis(relative(0.)).min_w_0())
            .child(if narrow { div().h(px(1.)).mx(px(28.)).bg(c.rule) } else { div().w(px(1.)).my(px(22.)).bg(c.rule) })
            .child(preview_slot.flex_grow(1.).flex_basis(relative(0.)).min_w_0())
    }

    fn slot_label(&self, dot: Hsla, label: String, c: &Colors) -> Div {
        div().flex().items_center().gap(px(8.)).child(status_dot(dot, 8.)).child(caps(label, 12., c.graphite))
    }

    fn slot_name(&self, name: String, muted: bool, c: &Colors) -> Div {
        let title = text::title(name, 30., c).line_height(px(34.));
        if muted { title.italic().font_weight(FontWeight::MEDIUM).text_color(c.graphite) } else { title }
    }

    fn change_button(&self, id: &'static str, menu: OpenMenu, name: String, cx: &mut Context<Self>) -> Button {
        Button::new(id, t!("models.setup.change")).a11y_label(name).expanded(self.menu.as_ref() == Some(&menu)).small().on_click(cx.listener(move |this, _, _, cx| this.toggle_menu(menu.clone(), cx)))
    }

    fn final_slot(&self, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let info = m.active_model();
        let status = m.status_of(&info.id);
        let engine = m.services.engine.is_some() && !m.engine_starting;
        let measured = m.measured_speed(&info.id);
        let remote = info.is_remote();

        let (dot, state) = match &status {
            // The engine has not said what is on disk yet.
            _ if m.engine_starting && !remote => (c.accent, Some(t!("models.hero.loading"))),
            ModelStatus::Ready => (c.success, None),
            ModelStatus::NotDownloaded if remote => (c.danger, Some(t!("models.hero.provider_not_set_up"))),
            // The idle time unloaded the model. The next dictation loads it.
            ModelStatus::Downloaded if m.model_idle => (c.success, None),
            ModelStatus::Downloaded => (c.accent, Some(t!("models.hero.loading"))),
            ModelStatus::Optimizing => (c.accent, Some(crate::shell::optimizing())),
            ModelStatus::Downloading { fraction, .. } => (c.accent, Some(format!("{:.0}%", fraction * 100.))),
            ModelStatus::NotDownloaded => (c.danger, Some(t!("models.hero.not_downloaded"))),
            ModelStatus::Failed { .. } => (c.danger, Some(t!("models.hero.model_error"))),
        };
        let label = match state {
            Some(s) => format!("{} · {s}", t!("models.setup.final")),
            None => t!("models.setup.final"),
        };

        let mut stats = div().flex().flex_none().items_end().gap(px(24.));
        if let Some(wer) = info.wer_percent {
            stats = stats.child(stat(accuracy_label(wer), t!("models.setup.accuracy"), &c));
        }
        stats = stats.child(match (measured, info.rtfx) {
            // A time from this computer beats the benchmark.
            (Some((ms, dur)), _) => stat(t!("models.hero.ms", ms = ms), t!("models.hero.for_speech", duration = kit::duration_words(dur)), &c),
            (None, Some(x)) => stat(format!("{x}×"), t!("models.setup.faster"), &c),
            (None, None) => stat(format!("{}/5", info.speed), t!("models.hero.speed"), &c),
        });
        stats = stats.child(if remote {
            stat(t!("models.hero.cloud"), t!("models.hero.audio_goes_to", vendor = info.vendor), &c)
        } else {
            stat(size_label(&info), if status.is_on_disk() || info.size_bytes == 0 { t!("models.setup.on_disk") } else { t!("models.hero.to_download") }, &c)
        });

        let id = info.id.clone();
        let action: Option<AnyElement> = match &status {
            _ if m.engine_starting && !remote => None,
            ModelStatus::NotDownloaded if remote => Some(ui(t!("models.hero.add_provider_again"), 13., 18., FontWeight::NORMAL, c.danger).into_any_element()),
            ModelStatus::NotDownloaded => Some(
                Button::new("hero-download", if info.size_bytes == 0 { t!("models.download") } else { t!("models.download_size", size = format_size(info.size_bytes)) })
                    .a11y_label(t!("models.a11y.download", name = info.name))
                    .primary()
                    .icon(Icon::Download)
                    .disabled(!engine)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let id = id.clone();
                        this.model.update(cx, |m, cx| m.download_model(id, cx));
                    }))
                    .into_any_element(),
            ),
            ModelStatus::Failed { message, reason } => Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(ui(crate::engine_text::describe(*reason, message), 13., 18., FontWeight::NORMAL, c.danger))
                    .child(Button::new("hero-retry", t!("models.try_again")).a11y_label(t!("models.a11y.try_again", name = info.name)).small().on_click(cx.listener(move |this, _, _, cx| {
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
                    .max_w(px(360.))
                    .child(Progress::new(*fraction).label(t!("models.a11y.downloading", name = info.name)))
                    .when(*bytes_total > 0, |d| d.child(mono(t!("models.hero.bytes_of", done = format_size(*bytes_done), total = format_size(*bytes_total)), 11., c.graphite)))
                    .into_any_element(),
            ),
            ModelStatus::Optimizing => Some(
                ui(
                    t!(crate::shell::os_text!("models.hero.optimizing_detail_mac", "models.hero.optimizing_detail_windows", "models.hero.optimizing_detail_other")),
                    13.,
                    18.,
                    FontWeight::NORMAL,
                    c.graphite,
                )
                .into_any_element(),
            ),
            _ => None,
        };

        let menu = (self.menu == Some(OpenMenu::Final)).then(|| self.final_menu(cx).into_any_element());
        let heard = status_text(&info.name, &status);
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .pt(px(24.))
            .pb(px(22.))
            .px(px(28.))
            .child(self.slot_label(dot, label, &c))
            .child(self.slot_name(info.name.clone(), false, &c))
            .child(ui(format!("{} · {}", info.vendor, info.language_label()), 14., 20., FontWeight::NORMAL, c.graphite))
            .when_some(action, |d, a| d.child(div().flex().pt(px(4.)).child(a)))
            .child(
                // Download, failure, and finish: a screen reader reads each change.
                // This row is always there, so the finish is read too.
                LiveStatus::new("final-status", heard)
                    .flex()
                    .items_end()
                    .justify_between()
                    .gap(px(16.))
                    .mt_auto()
                    .pt(px(10.))
                    .child(stats)
                    .child(div().relative().flex_none().child(self.change_button("change-final", OpenMenu::Final, t!("models.a11y.change_final"), cx)).children(menu)),
            )
    }

    /// The models that can write the final text now: local models on disk and
    /// set-up cloud models.
    fn final_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let m = self.model.read(cx);
        let active = m.active_model().id;
        let models: Vec<ModelInfo> = m.catalog().into_iter().filter(|i| i.final_pass && m.status_of(&i.id).is_on_disk() || i.is_remote() && m.status_of(&i.id) == ModelStatus::Ready).collect();
        let items = models
            .iter()
            .map(|i| {
                let item = MenuItem::new(i.name.clone()).selected(i.id == active);
                match i.wer_percent {
                    Some(wer) => item.detail(accuracy_label(wer)),
                    None => item,
                }
            })
            .collect();
        let ids: Vec<ModelId> = models.into_iter().map(|i| i.id).collect();
        let page = cx.entity().downgrade();
        let pick = {
            let page = page.clone();
            move |i: usize, _: &mut Window, cx: &mut App| {
                let id = ids[i].clone();
                let _ = page.update(cx, |this, cx| {
                    this.menu = None;
                    this.model.update(cx, |m, cx| m.set_active_model(id, cx));
                });
            }
        };
        div().absolute().top_full().right_0().w(px(300.)).child(kit::menu_items("final-menu", items, pick, Self::close_menu(page), cx))
    }

    fn preview_state(&self, cx: &App) -> PreviewSlot {
        let m = self.model.read(cx);
        let d = &m.config.dictation;
        if !d.live_preview {
            return PreviewSlot::Off;
        }
        if m.preview_language_mismatch()
            && let PreviewChoice::Model(id) = &d.preview_model
            && let Some(info) = sayso_core::models::find(id)
        {
            return PreviewSlot::Paused(info);
        }
        // A chosen model that is not on disk yet shows its download.
        if let PreviewChoice::Model(id) = &d.preview_model
            && !m.status_of(id).is_on_disk()
            && let Some(info) = sayso_core::models::find(id)
        {
            return PreviewSlot::Model(info);
        }
        let active = m.active_model();
        match m.preview_model() {
            Some(id) if id == active.id => PreviewSlot::Same(active),
            Some(id) => sayso_core::models::find(&id).map_or(PreviewSlot::Unavailable, PreviewSlot::Model),
            None => PreviewSlot::Unavailable,
        }
    }

    fn preview_slot(&self, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let state = self.preview_state(cx);
        let m = self.model.read(cx);
        let label = t!("models.setup.preview");
        let (head, name, muted, line) = match &state {
            PreviewSlot::Off => (self.slot_label(c.pencil, label, &c), t!("models.setup.off"), true, None),
            PreviewSlot::Unavailable => (self.slot_label(c.pencil, label, &c), t!("models.setup.unavailable"), true, Some(t!("models.setup.unavailable_note"))),
            PreviewSlot::Paused(info) => (
                self.slot_label(c.danger, format!("{label} · {}", t!("models.setup.paused")), &c),
                info.name.clone(),
                false,
                Some(t!("models.setup.mismatch", language = sayso_core::languages::display(&m.config.dictation.language))),
            ),
            PreviewSlot::Same(_) => (self.slot_label(c.success, label, &c), t!("models.setup.same_model"), true, None),
            PreviewSlot::Model(info) => {
                let (dot, s) = match m.status_of(&info.id) {
                    ModelStatus::Downloading { fraction, .. } => (c.accent, Some(format!("{:.0}%", fraction * 100.))),
                    ModelStatus::Optimizing | ModelStatus::Downloaded => (c.accent, Some(t!("models.hero.loading"))),
                    ModelStatus::Failed { .. } => (c.danger, Some(t!("models.hero.model_error"))),
                    _ => (c.success, None),
                };
                let text = match s {
                    Some(s) => format!("{label} · {s}"),
                    None => label,
                };
                (self.slot_label(dot, text, &c), info.name.clone(), false, Some(format!("{} · {}", info.vendor, info.language_label())))
            }
        };
        let mut stats = div().flex().flex_none().items_end().gap(px(24.));
        if let PreviewSlot::Same(info) | PreviewSlot::Model(info) = &state {
            if let Some(wer) = info.preview_wer_percent.or(info.wer_percent) {
                stats = stats.child(stat(accuracy_label(wer), t!("models.setup.accuracy"), &c));
            }
            if let Some(ms) = info.preview_interval_ms() {
                stats = stats.child(stat(interval_label(ms), t!("models.setup.between_updates"), &c));
            }
        }
        let heard = match &state {
            PreviewSlot::Model(info) => status_text(&info.name, &m.status_of(&info.id)),
            _ => String::new(),
        };
        let button: AnyElement = if let PreviewSlot::Paused(_) = state {
            Button::new("use-automatic", t!("models.setup.use_automatic"))
                .small()
                .on_click(cx.listener(|this, _, _, cx| this.model.update(cx, |m, cx| m.set_preview(Some(PreviewChoice::Auto), cx))))
                .into_any_element()
        } else {
            let menu = (self.menu == Some(OpenMenu::Preview)).then(|| self.preview_menu(cx).into_any_element());
            div().relative().flex_none().child(self.change_button("change-preview", OpenMenu::Preview, t!("models.a11y.change_preview"), cx)).children(menu).into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .pt(px(24.))
            .pb(px(22.))
            .px(px(28.))
            .child(head)
            .child(self.slot_name(name, muted, &c))
            .when_some(line, |d, l| d.child(ui(l, 14., 20., FontWeight::NORMAL, c.graphite)))
            .child(
                LiveStatus::new("preview-status", heard)
                    .flex()
                    .items_end()
                    .justify_between()
                    .gap(px(16.))
                    .mt_auto()
                    .pt(px(10.))
                    .child(stats)
                    .child(button),
            )
    }

    /// Automatic, the streaming models, and off.
    fn preview_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let m = self.model.read(cx);
        let d = &m.config.dictation;
        let streaming: Vec<ModelInfo> = m.catalog().into_iter().filter(|i| i.live_preview).collect();
        let mut items = vec![MenuItem::new(t!("models.setup.automatic")).selected(d.live_preview && d.preview_model == PreviewChoice::Auto).rule_after()];
        let count = streaming.len();
        for (n, i) in streaming.iter().enumerate() {
            let chosen = d.live_preview && d.preview_model == PreviewChoice::Model(i.id.clone());
            let detail = if m.status_of(&i.id).is_on_disk() { i.language_label() } else { format_size(i.size_bytes) };
            let item = MenuItem::new(i.name.clone()).detail(detail).selected(chosen);
            items.push(if n + 1 == count { item.rule_after() } else { item });
        }
        items.push(MenuItem::new(t!("models.setup.no_preview")).selected(!d.live_preview));
        let ids: Vec<ModelId> = streaming.into_iter().map(|i| i.id).collect();
        let page = cx.entity().downgrade();
        let pick = {
            let page = page.clone();
            move |i: usize, _: &mut Window, cx: &mut App| {
                let choice = match i {
                    0 => Some(PreviewChoice::Auto),
                    i if i <= ids.len() => Some(PreviewChoice::Model(ids[i - 1].clone())),
                    _ => None,
                };
                let _ = page.update(cx, |this, cx| {
                    this.menu = None;
                    this.model.update(cx, |m, cx| m.set_preview(choice, cx));
                });
            }
        };
        div().absolute().top_full().right_0().w(px(320.)).child(kit::menu_items("preview-menu", items, pick, Self::close_menu(page), cx))
    }

    // -----------------------------------------------------------------------
    // Lists
    // -----------------------------------------------------------------------

    /// `place` is the row and the size of its list.
    fn row(&self, info: &ModelInfo, place: (usize, usize), roles: &Roles, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let status = m.status_of(&info.id);
        let engine = m.services.engine.is_some() && !m.engine_starting;
        let remote = info.is_remote();
        // A custom endpoint on this Mac is not cloud.
        let place_label = match sayso_core::speech::split_model_id(&info.id).and_then(|(p, _)| m.config.speech.provider(p)) {
            Some(p) if !p.is_cloud() => t!("models.row.server"),
            _ => t!("models.row.cloud"),
        };
        let mut meta = if remote { info.description.trim_end_matches('.').to_string() } else { format!("{} · {}", info.vendor, info.language_label()) };
        if !info.punctuation {
            meta = format!("{meta} · {}", t!("models.row.no_punctuation"));
        }
        let id = info.id.clone();
        let key = info.id.as_str().to_string();
        let tag = roles.tag(&info.id);
        let name = info.name.clone();

        let action: AnyElement = match &status {
            ModelStatus::Downloading { fraction, .. } => div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .w_full()
                .child(div().flex().justify_between().child(ui(t!("models.row.downloading"), 12., 16., FontWeight::SEMIBOLD, c.ink)).child(kit::plain_mono(format!("{:.0}%", fraction * 100.), 11., c.graphite)))
                .child(Progress::new(*fraction).label(t!("models.a11y.downloading", name = name)))
                .into_any_element(),
            ModelStatus::NotDownloaded if !remote => Button::new(SharedString::from(format!("dl-{key}")), t!("models.download"))
                .a11y_label(t!("models.a11y.download", name = name))
                .small()
                .disabled(!engine)
                .on_click(cx.listener(move |this, _, _, cx| {
                    let id = id.clone();
                    this.model.update(cx, |m, cx| m.download_model(id, cx));
                }))
                .into_any_element(),
            ModelStatus::Failed { message, reason } => div()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(4.))
                .child(Button::new(SharedString::from(format!("retry-{key}")), t!("models.try_again")).a11y_label(t!("models.a11y.try_again", name = name)).small().on_click(cx.listener(move |this, _, _, cx| {
                    let id = id.clone();
                    this.model.update(cx, |m, cx| m.download_model(id, cx));
                })))
                .child(ui(crate::engine_text::describe(*reason, message), 11., 14., FontWeight::NORMAL, c.danger).truncate().max_w(px(150.)))
                .into_any_element(),
            ModelStatus::Optimizing => ui(t!("models.row.optimizing"), 12., 16., FontWeight::SEMIBOLD, c.graphite).into_any_element(),
            _ if tag.is_some() => role_pill(tag.clone().unwrap_or_default(), &c).into_any_element(),
            _ if info.live_preview => {
                let open = self.menu == Some(OpenMenu::Use(info.id.clone()));
                let menu_id = info.id.clone();
                let menu = open.then(|| self.use_menu(info.id.clone(), cx).into_any_element());
                div()
                    .relative()
                    .child(
                        Button::new(SharedString::from(format!("use-{key}")), t!("models.row.use"))
                            .a11y_label(t!("models.a11y.use", name = name))
                            .expanded(open)
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_menu(OpenMenu::Use(menu_id.clone()), cx))),
                    )
                    .children(menu)
                    .into_any_element()
            }
            _ => Button::new(SharedString::from(format!("use-{key}")), t!("models.row.use"))
                .a11y_label(t!("models.a11y.use", name = name))
                .small()
                .on_click(cx.listener(move |this, _, _, cx| {
                    let id = id.clone();
                    this.model.update(cx, |m, cx| m.set_active_model(id, cx));
                }))
                .into_any_element(),
        };

        let accuracy = match info.wer_percent {
            Some(wer) => div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(meter(accuracy_level(wer), &c))
                .child(kit::plain(accuracy_label(wer), 13., 16., FontWeight::NORMAL, c.graphite)),
            None => kit::plain(t!("models.row.not_measured"), 13., 16., FontWeight::NORMAL, c.pencil),
        };
        // The details button says all that the row shows: name, maker,
        // languages, size, accuracy, speed, and the role.
        let size = if remote { place_label.clone() } else { size_label(info) };
        let mut said = vec![info.name.clone(), meta.clone(), size.clone()];
        if let Some(wer) = info.wer_percent {
            said.push(t!("models.a11y.accuracy", value = accuracy_label(wer)));
        }
        if !remote {
            said.push(t!("models.a11y.speed", n = info.speed));
        }
        said.extend(tag);
        said.push(t!("models.a11y.show_details"));
        let heard = status_text(&info.name, &status);
        let open_id = info.id.clone();
        div()
            .id(SharedString::from(format!("model-{key}")))
            .role(Role::ListItem)
            .aria_label(info.name.clone())
            .aria_position_in_set(place.0 + 1)
            .aria_size_of_set(place.1)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(16.))
            .h(px(66.))
            .border_b_1()
            .border_color(c.rule)
            .child(
                Pressable::new(SharedString::from(format!("open-{key}")))
                    .label(said.join(", "))
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .items_center()
                    .gap(px(16.))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| this.open_details(open_id.clone(), window, cx)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(3.))
                            .child(plain_title(info.name.clone(), 17., &c).line_height(px(22.)))
                            .child(kit::plain(meta, 13., 16., FontWeight::NORMAL, c.graphite).truncate()),
                    )
                    // A narrow page has no accuracy and speed columns.
                    .when(!self.narrow, |d| d.child(accuracy.w(px(ACCURACY_W)).flex_none()).child(meter(info.speed, &c).w(px(SPEED_W))))
                    .child(kit::plain_mono(size, 12., c.graphite).w(px(SIZE_W)).flex_none()),
            )
            // A change of state (download, failure, ready) is read by a screen reader.
            .child(LiveStatus::new("state", heard).flex().flex_none().justify_end().w(px(ACTION_W)).child(action))
    }

    /// The "Use" menu of a streaming model: for the final text or for the preview.
    fn use_menu(&self, id: ModelId, cx: &mut Context<Self>) -> impl IntoElement {
        let items = vec![MenuItem::new(t!("models.row.for_final")), MenuItem::new(t!("models.row.for_preview"))];
        let page = cx.entity().downgrade();
        let pick = {
            let page = page.clone();
            move |i: usize, _: &mut Window, cx: &mut App| {
                let id = id.clone();
                let _ = page.update(cx, |this, cx| {
                    this.menu = None;
                    this.model.update(cx, |m, cx| if i == 0 { m.set_active_model(id, cx) } else { m.set_preview(Some(PreviewChoice::Model(id)), cx) });
                });
            }
        };
        div().absolute().top_full().right_0().w(px(200.)).child(kit::menu_items("use-menu", items, pick, Self::close_menu(page), cx))
    }

    /// A list of local models: a title, a note, column captions, then the rows.
    fn group(&self, title: String, note: String, models: &[ModelInfo], roles: &Roles, cx: &mut Context<Self>) -> Stateful<Div> {
        let c = cx.paper().colors;
        // The id keeps the captions of each group apart.
        let table = div().id(SharedString::from(format!("group-{title}"))).flex().flex_col().child(
            div()
                .flex()
                .flex_none()
                .items_end()
                .gap(px(16.))
                .pb(px(10.))
                .border_b_1()
                .border_color(c.rule)
                .child(div().flex().flex_col().flex_1().min_w_0().gap(px(3.)).child(text::title(title.clone(), 20., &c).line_height(px(26.))).child(ui(note, 13., 18., FontWeight::NORMAL, c.graphite)))
                .when(!self.narrow, |d| d.child(self.sort_caption(SortKey::Accuracy, ACCURACY_W, cx)).child(self.sort_caption(SortKey::Speed, SPEED_W, cx)))
                .child(self.sort_caption(SortKey::Size, SIZE_W, cx))
                .child(div().w(px(ACTION_W)).flex_none()),
        );
        let mut rows = div().id(SharedString::from(format!("rows-{title}"))).role(Role::List).aria_label(title.clone()).flex().flex_col();
        for (i, info) in models.iter().enumerate() {
            rows = rows.child(self.row(info, (i, models.len()), roles, cx));
        }
        table.child(rows)
    }

    // -----------------------------------------------------------------------
    // Details
    // -----------------------------------------------------------------------

    fn details_modal(&self, id: &ModelId, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let info = m.catalog().into_iter().find(|i| &i.id == id)?;
        let status = m.status_of(&info.id);
        let engine = m.services.engine.is_some() && !m.engine_starting;
        let active = m.active_model();
        let preview = m.preview_model().and_then(|p| sayso_core::models::find(&p));
        let measured = m.measured_speed(&info.id);
        let remote = info.is_remote();
        let key = info.id.as_str().to_string();

        let kind = if remote { t!("models.hero.cloud") } else if info.live_preview { t!("models.details.streaming") } else { t!("models.details.non_streaming") };
        let close = kit::icon_button("close-details", Icon::Close, 10., cx).label(t!("models.a11y.close_details")).on_click(cx.listener(|this, _, _, cx| {
            this.details = None;
            cx.notify();
        }));
        let head = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .pt(px(26.))
            .pb(px(22.))
            .px(px(30.))
            .child(div().flex().items_center().justify_between().child(caps(format!("{kind} · {}", info.vendor), 12., c.graphite)).child(close))
            .child(text::title(info.name.clone(), 34., &c).line_height(px(38.)))
            .child(ui(info.description.clone(), 15., 22., FontWeight::NORMAL, c.graphite))
            .when_some(info.credit.clone(), |d, credit| d.child(ui(credit, 12., 17., FontWeight::NORMAL, c.graphite)));

        // Accuracy, on a scale from 85% to 95%.
        let accuracy = info.wer_percent.map(|wer| {
            const W: f32 = 560.;
            let x = |wer: f32| ((15. - wer) / 10.).clamp(0., 1.) * W;
            let mut scale = div().relative().w(px(W)).h(px(70.)).flex_none()
                .child(mono(accuracy_label(15.), 11., c.graphite).absolute().left_0().top_0())
                .child(mono(accuracy_label(5.), 11., c.graphite).absolute().right_0().top_0())
                .child(div().absolute().left_0().top(px(28.)).w(px(W)).h(px(6.)).rounded_full().bg(c.deboss));
            for other in m.catalog() {
                if let Some(w) = other.wer_percent.filter(|_| other.id != info.id) {
                    scale = scale.child(div().absolute().left(px(x(w) - 1.)).top(px(26.)).w(px(2.)).h(px(10.)).rounded(px(1.)).bg(c.deboss_shade));
                }
            }
            let marker = |at: f32, name: String, ring: bool| {
                div()
                    .child(div().absolute().left(px(at - 6.)).top(px(25.)).size(px(12.)).rounded_full().map(|d| if ring { d.border_3().border_color(c.accent).bg(c.sheet_raised) } else { d.bg(c.accent) }))
                    .child(ui(name, 12., 16., FontWeight::NORMAL, c.accent).absolute().top(px(46.)).left(px((at - 90.).clamp(0., W - 180.))).w(px(180.)).text_center())
            };
            if active.id != info.id && let Some(w) = active.wer_percent {
                scale = scale.child(marker(x(w), active.name.clone(), false));
            }
            if let Some(p) = preview.as_ref().filter(|p| p.id != info.id && p.id != active.id) && let Some(w) = p.preview_wer_percent.or(p.wer_percent) {
                scale = scale.child(marker(x(w), p.name.clone(), true));
            }
            scale = scale
                .child(div().absolute().left(px(x(wer) - 7.)).top(px(24.)).size(px(14.)).rounded_full().bg(c.ink).border_2().border_color(c.sheet_raised))
                .child(ui(t!("models.details.this_model"), 12., 14., FontWeight::SEMIBOLD, c.ink).absolute().top_0().left(px((x(wer) - 40.).clamp(56., W - 136.))).w(px(80.)).text_center());
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .pt(px(20.))
                .pb(px(18.))
                .px(px(30.))
                .border_t_1()
                .border_color(c.rule)
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .justify_between()
                        .child(caps(t!("models.group.accuracy"), 12., c.graphite))
                        .child(text::figure(accuracy_label(wer), 22., &c)),
                )
                .child(scale)
        });

        let fact = |label: String, value: String, muted: bool| {
            div()
                // Two facts can have the same value ("Yes"), so the label is the id.
                .id(SharedString::from(format!("fact-{label}")))
                .flex()
                .justify_between()
                .items_baseline()
                .py(px(10.))
                .border_t_1()
                .border_color(c.rule)
                .child(ui(label, 13., 18., FontWeight::NORMAL, c.graphite))
                .child(ui(value, 14., 18., FontWeight::MEDIUM, if muted { c.pencil } else { c.ink }))
        };
        let yes_no = |v: bool| if v { t!("models.details.yes") } else { t!("models.details.no") };
        let speed = match (info.rtfx, measured) {
            (Some(x), _) => t!("models.details.rtfx", x = x),
            (None, Some((ms, dur))) => format!("{} {}", t!("models.hero.ms", ms = ms), t!("models.hero.for_speech", duration = kit::duration_words(dur))),
            (None, None) => t!("models.details.speed_of_five", n = info.speed),
        };
        let interval = info.preview_interval_ms().map(|ms| t!("models.details.every", interval = interval_label(ms)));
        let left = div()
            .flex()
            .flex_col()
            .flex_1()
            .child(fact(t!("models.group.speed"), speed, false))
            .child(fact(t!("models.setup.preview"), interval.clone().unwrap_or_else(|| yes_no(info.live_preview)), !info.live_preview))
            .child(fact(t!("models.details.languages"), info.language_label(), false).border_b_1());
        let right = div()
            .flex()
            .flex_col()
            .flex_1()
            .child(fact(t!("models.details.punctuation"), yes_no(info.punctuation), !info.punctuation))
            .child(fact(t!("models.details.dictionary"), if info.vocabulary { t!("models.details.used") } else { t!("models.details.not_used") }, !info.vocabulary))
            .child(fact(t!("models.details.size"), if remote { t!("models.hero.cloud") } else { size_label(&info) }, false).border_b_1());
        let facts = div().flex().gap(px(28.)).pt(px(4.)).pb(px(16.)).px(px(30.)).child(left).child(right);
        let source = info.wer_percent.map(|wer| ui(t!("models.details.source", wer = sayso_i18n::numbers::decimal(wer as f64, 2)), 12., 17., FontWeight::NORMAL, c.graphite).px(px(30.)).pb(px(18.)));

        // The action bar.
        let is_final = active.id == info.id;
        let is_preview = preview.as_ref().is_some_and(|p| p.id == info.id);
        let confirm = self.confirm_delete.as_deref() == Some(key.as_str());
        let can_delete = !remote && status.is_on_disk() && info.engine != EngineKind::AppleSpeech;
        // Its own bottom corners: the panel clips to a rectangle, not to its rounded corners.
        let mut bar = div().flex().items_center().gap(px(10.)).py(px(14.)).px(px(30.)).border_t_1().border_color(c.rule).bg(c.sheet).rounded_b(px(18.));
        if can_delete {
            let (id, k) = (info.id.clone(), key.clone());
            bar = bar.child(if confirm {
                Button::new("details-delete-yes", t!("models.row.delete")).a11y_label(t!("models.a11y.delete", name = info.name)).small().danger().on_click(cx.listener(move |this, _, _, cx| {
                    let id = id.clone();
                    this.confirm_delete = None;
                    this.details = None;
                    this.model.update(cx, |m, cx| m.delete_model(id, cx));
                }))
            } else {
                Button::new("details-delete", t!("models.row.delete")).a11y_label(t!("models.a11y.delete", name = info.name)).small().ghost().on_click(cx.listener(move |this, _, _, cx| {
                    this.confirm_delete = Some(k.clone());
                    cx.notify();
                }))
            });
        }
        if let Some(url) = info.card_url.clone() {
            bar = bar.child(kit::link("model-card", t!("models.details.model_card"), 13., cx).on_click(move |_, _, _| AppModel::open_url(&url)));
        }
        bar = bar.child(div().flex_1());
        match &status {
            ModelStatus::NotDownloaded if !remote => {
                let id = info.id.clone();
                bar = bar.child(Button::new("details-download", t!("models.download_size", size = format_size(info.size_bytes))).a11y_label(t!("models.a11y.download", name = info.name)).primary().icon(Icon::Download).disabled(!engine).on_click(cx.listener(move |this, _, _, cx| {
                    let id = id.clone();
                    this.model.update(cx, |m, cx| m.download_model(id, cx));
                })));
            }
            ModelStatus::Downloading { fraction, .. } => bar = bar.child(div().w(px(200.)).child(Progress::new(*fraction).label(t!("models.a11y.downloading", name = info.name)))),
            _ => {
                if info.live_preview {
                    bar = bar.child(if is_preview {
                        role_pill(t!("models.setup.preview"), &c).into_any_element()
                    } else {
                        let id = info.id.clone();
                        Button::new("details-use-preview", t!("models.details.use_preview"))
                            .a11y_label(t!("models.a11y.use_preview", name = info.name))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let id = id.clone();
                                this.model.update(cx, |m, cx| m.set_preview(Some(PreviewChoice::Model(id)), cx));
                            }))
                            .into_any_element()
                    });
                }
                if info.final_pass {
                    bar = bar.child(if is_final {
                        role_pill(t!("models.setup.final"), &c).into_any_element()
                    } else {
                        let id = info.id.clone();
                        Button::new("details-use-final", t!("models.details.use_final"))
                            .a11y_label(t!("models.a11y.use_final", name = info.name))
                            .primary()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let id = id.clone();
                                this.model.update(cx, |m, cx| m.set_active_model(id, cx));
                            }))
                            .into_any_element()
                    });
                }
            }
        }

        let panel = div()
            .id("model-details")
            .role(Role::Dialog)
            .aria_label(info.name.clone())
            // The focus is on the dialog, so a screen reader reads its name.
            .track_focus(&self.focus)
            .w(px(620.))
            .max_h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .rounded(px(18.))
            .bg(c.sheet_raised)
            .shadow(sayso_ui::paper::floating(&c))
            .occlude()
            .child(head)
            .children(accuracy)
            .child(facts)
            .children(source)
            .child(bar);
        let scrim = Pressable::new("details-scrim").label(t!("models.a11y.close_details")).tab_stop(false).absolute().inset_0().bg(c.ground.opacity(0.55)).occlude().on_click(cx.listener(|this, _, _, cx| {
            this.details = None;
            cx.notify();
        }));
        Some(
            div()
                .absolute()
                .inset_0()
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                    if ev.keystroke.key == "escape" {
                        this.details = None;
                        cx.notify();
                    }
                }))
                .flex()
                .items_center()
                .justify_center()
                .p(px(24.))
                .child(scrim)
                .child(panel)
                .into_any_element(),
        )
    }

    // -----------------------------------------------------------------------
    // Cloud providers
    // -----------------------------------------------------------------------

    fn test(&mut self, id: String, cx: &mut Context<Self>) {
        self.tests.insert(id.clone(), Test::Running);
        let page = cx.entity().downgrade();
        self.model.update(cx, |m, cx| {
            let key = id.clone();
            m.test_speech_provider(&id, cx, move |_, result, cx| {
                let _ = page.update(cx, |this, cx| {
                    this.tests.insert(key, match result {
                        Ok(ms) => Test::Passed(ms),
                        Err(e) => Test::Failed(e),
                    });
                    cx.notify();
                });
            });
        });
        cx.notify();
    }

    /// A provider's line: name, where the audio goes, the last check, and its actions.
    fn provider_head(&self, p: &SpeechProvider, cx: &mut Context<Self>) -> Stateful<Div> {
        let c = cx.paper().colors;
        let cloud = p.is_cloud();
        let host = p.base_url().map(|u| u.split("://").nth(1).unwrap_or(&u).to_string()).unwrap_or_default();
        let (line, line_color) = match self.tests.get(&p.id) {
            Some(Test::Running) => (t!("models.provider.checking"), c.graphite),
            Some(Test::Passed(ms)) => (t!("models.provider.connected", ms = ms), c.success),
            Some(Test::Failed(e)) => (e.clone(), c.danger),
            None if cloud => (t!("models.provider.sends_audio", host = host), c.graphite),
            None => (t!("models.provider.own_server", host = host), c.graphite),
        };
        let confirm = self.confirm_remove.as_deref() == Some(p.id.as_str());
        let (tid, rid, cid) = (p.id.clone(), p.id.clone(), p.id.clone());
        let running = matches!(self.tests.get(&p.id), Some(Test::Running));
        let actions = if confirm {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(4.))
                .child({
                    // The question appears after a click, so a screen reader says it at once.
                    let question = t!("models.provider.confirm_remove");
                    div().id("confirm-remove").role(Role::Status).a11y_live(question.clone(), true).child(kit::plain(question, 12., 16., FontWeight::NORMAL, c.graphite))
                })
                .child(Button::new(SharedString::from(format!("sp-rm-yes-{}", p.id)), t!("models.provider.remove")).a11y_label(t!("models.a11y.remove_provider", name = p.name)).small().danger().on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.confirm_remove = None;
                        this.tests.remove(&rid);
                        let id = rid.clone();
                        this.model.update(cx, |m, cx| m.remove_speech_provider(&id, cx));
                    },
                )))
                .child(Button::new(SharedString::from(format!("sp-rm-no-{}", p.id)), t!("models.cancel")).small().ghost().on_click(cx.listener(
                    |this, _, _, cx| {
                        this.confirm_remove = None;
                        cx.notify();
                    },
                )))
        } else {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(4.))
                .child(Button::new(SharedString::from(format!("sp-test-{}", p.id)), t!("models.provider.test")).a11y_label(t!("models.a11y.test_provider", name = p.name)).small().disabled(running).on_click(cx.listener(
                    move |this, _, _, cx| this.test(tid.clone(), cx),
                )))
                .child(Button::new(SharedString::from(format!("sp-rm-{}", p.id)), t!("models.provider.remove")).a11y_label(t!("models.a11y.remove_provider", name = p.name)).small().ghost().on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.confirm_remove = Some(cid.clone());
                        cx.notify();
                    },
                )))
        };
        div()
            .id(SharedString::from(format!("provider-{}", p.id)))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(12.))
            .pt(px(18.))
            .pb(px(10.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(text::title(p.name.clone(), 20., &c).line_height(px(24.)))
                            .child(kit::place_badge(cloud, cx)),
                    )
                    // The result of a check is read when it comes.
                    .child(div().id("provider-line").role(Role::Status).a11y_live(line.clone(), false).child(kit::plain(line, 13., 16., FontWeight::NORMAL, line_color).truncate())),
            )
            .child(actions)
    }

    fn open_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.add.is_some() {
            self.add = None;
            cx.notify();
            return;
        }
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(t!("models.add.name_placeholder")));
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("http://localhost:8000/v1"));
        let model_id = cx.new(|cx| InputState::new(window, cx).placeholder(t!("models.add.model_placeholder")));
        let key = cx.new(|cx| InputState::new(window, cx).placeholder(key_placeholder()).masked(true));
        self._subs.push(cx.subscribe_in(&url, window, |_, _, _: &InputEvent, _, cx| cx.notify()));
        key.update(cx, |s, cx| s.focus(window, cx));
        self.add = Some(AddProvider { kind: SpeechProviderKind::OpenAi, name, url, model_id, key, error: None });
        cx.notify();
    }

    fn save_add(&mut self, cx: &mut Context<Self>) {
        let Some(add) = self.add.as_mut() else { return };
        let key = add.key.read(cx).value().trim().to_string();
        let custom = add.kind == SpeechProviderKind::OpenAiCompatible;
        let (name, base_url, models) = if custom {
            let name = add.name.read(cx).value().trim().to_string();
            let url = add.url.read(cx).value().trim().trim_end_matches('/').to_string();
            let model = add.model_id.read(cx).value().trim().to_string();
            add.error = if name.is_empty() {
                Some(AddError::Name)
            } else if !(url.starts_with("http://") || url.starts_with("https://")) {
                Some(AddError::Url)
            } else if model.is_empty() {
                Some(AddError::ModelId)
            } else if key.is_empty() && !is_local_url(&url) {
                Some(AddError::ServerKey)
            } else {
                None
            };
            (name, Some(url), vec![model])
        } else {
            add.error = key.is_empty().then(|| AddError::Key(add.kind.label().to_string()));
            (add.kind.label().to_string(), None, Vec::new())
        };
        if add.error.is_some() {
            cx.notify();
            return;
        }
        let kind = add.kind;
        let id = self.model.read(cx).new_speech_provider_id(&name);
        let key = (!key.is_empty()).then_some(key);
        let provider = SpeechProvider { api_key_account: key.as_ref().map(|_| format!("speech.{id}")), id: id.clone(), name, kind, base_url, models };
        self.add = None;
        self.model.update(cx, |m, cx| m.save_speech_provider(provider, key, cx));
        self.test(id, cx);
    }

    fn add_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = cx.paper().colors;
        let add = self.add.as_ref()?;
        let count = SpeechProviderKind::ALL.len();
        let current = SpeechProviderKind::ALL.iter().position(|k| *k == add.kind).unwrap_or(0);
        // A group of radio buttons: the arrow keys change the kind and move the focus.
        let mut kinds = div()
            .id("speech-kinds")
            .role(Role::RadioGroup)
            .aria_label(t!("models.add.provider"))
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, w, cx| {
                let next = match ev.keystroke.key.as_str() {
                    "right" | "down" if current + 1 < count => current + 1,
                    "left" | "up" if current > 0 => current - 1,
                    _ => return,
                };
                cx.stop_propagation();
                if let Some(a) = this.add.as_mut() {
                    a.kind = SpeechProviderKind::ALL[next];
                    a.error = None;
                }
                if next > current { w.focus_next(cx) } else { w.focus_prev(cx) }
                cx.notify();
            }));
        for (i, kind) in SpeechProviderKind::ALL.into_iter().enumerate() {
            kinds = kinds.child(Chip::new(("speech-kind", i), kind.label(), add.kind == kind).role(ChipRole::Choice { index: i, count }).on_click(cx.listener(move |this, _, _, cx| {
                if let Some(a) = this.add.as_mut() {
                    a.kind = kind;
                    a.error = None;
                }
                cx.notify();
            })));
        }
        let custom = add.kind == SpeechProviderKind::OpenAiCompatible;
        let url = add.url.read(cx).value().to_string();
        let cloud = !custom || (!url.trim().is_empty() && !is_local_url(&url));
        let mut body = div().flex().flex_col().gap(px(16.)).child(kit::field(&t!("models.add.provider"), kinds, None, cx));
        if custom {
            body = body
                .child(
                    div()
                        .flex()
                        .gap(px(12.))
                        .child(kit::field(&t!("models.add.name"), kit::input_well_named(&add.name, Some(t!("models.add.name").into()), cx), None, cx).w(px(220.)))
                        .child(
                            kit::field(
                                &t!("models.add.base_url"),
                                kit::input_well_named(&add.url, Some(t!("models.add.base_url").into()), cx),
                                Some(&t!("models.add.base_url_hint")),
                                cx,
                            )
                            .flex_1(),
                        ),
                )
                .child(kit::field(&t!("models.add.model"), kit::input_well_named(&add.model_id, Some(t!("models.add.model").into()), cx), None, cx))
                .child(kit::field(&t!("models.add.api_key"), kit::secret_well_named(&add.key, t!("models.add.api_key").into(), cx), Some(&t!("models.add.api_key_hint", computer = crate::shell::computer())), cx));
        } else {
            let names: Vec<&str> = add.kind.known_models().iter().map(|m| m.name).collect();
            let key_link = add.kind.key_url().map(|url| {
                kit::link("speech-key-link", t!("models.add.get_key", provider = add.kind.label()), 12., cx)
                    .on_click(move |_, _, _| AppModel::open_url(url))
                    .into_any_element()
            });
            body = body
                .child(kit::field(&t!("models.add.api_key"), kit::secret_well_named(&add.key, t!("models.add.api_key").into(), cx), None, cx).when_some(key_link, |d, l| d.child(div().flex().child(l))))
                .child(ui(t!("models.add.adds", names = names.join(", ")), 13., 18., FontWeight::NORMAL, c.graphite));
        }
        if cloud {
            body = body.child(Banner::new(
                BannerKind::Info,
                t!("models.add.cloud_notice", computer = crate::shell::computer()),
            ));
        }
        let panel = div()
            .id("add-speech-provider-form")
            .role(Role::Group)
            .aria_label(t!("models.add.title"))
            .flex()
            .flex_col()
            .gap(px(18.))
            .mt(px(14.))
            .p(px(22.))
            .rounded(px(14.))
            .raised(&c)
            .child(text::title(t!("models.add.title"), 22., &c))
            .child(body)
            .when_some(add.error.as_ref(), |d, e| d.child(Banner::new(BannerKind::Warning, e.text())))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(Button::new("save-speech-provider", t!("models.add.save")).primary().on_click(cx.listener(|this, _, _, cx| this.save_add(cx))))
                    .child(Button::new("cancel-speech-provider", t!("models.cancel")).ghost().on_click(cx.listener(|this, _, _, cx| {
                        this.add = None;
                        cx.notify();
                    }))),
            );
        Some(panel.into_any_element())
    }

    /// The cloud section: every provider with its models, then the add form.
    fn cloud(&mut self, roles: &Roles, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let providers = self.model.read(cx).config.speech.providers.clone();
        let query = self.query(cx);
        let note = ui(t!("models.cloud.note", computer = crate::shell::computer()), 13., 16., FontWeight::NORMAL, c.graphite).into_any_element();
        let mut col = div().flex().flex_col().child(kit::section_head(&t!("models.cloud.title"), Some(note), cx));
        if providers.is_empty() && self.add.is_none() {
            col = col.child(
                ui(
                    t!("models.cloud.empty"),
                    14.,
                    20.,
                    FontWeight::NORMAL,
                    c.graphite,
                )
                .max_w(px(620.))
                .pt(px(14.)),
            );
        }
        for p in &providers {
            let models: Vec<ModelInfo> = p.catalog().into_iter().filter(|m| m.id != roles.active && matches(m, &query)).collect();
            if models.is_empty() && !query.is_empty() {
                continue;
            }
            col = col.child(self.provider_head(p, cx));
            let mut rows = div().id(SharedString::from(format!("provider-models-{}", p.id))).role(Role::List).aria_label(p.name.clone()).flex().flex_col();
            for (i, info) in models.iter().enumerate() {
                rows = rows.child(self.row(info, (i, models.len()), roles, cx));
            }
            col = col.child(rows).child(div().h(px(1.)).bg(c.rule));
        }
        match self.add_panel(cx) {
            Some(panel) => col.child(panel),
            None => col.child(
                div().flex().pt(px(16.)).child(
                    kit::dashed("add-speech-provider", cx)
                        .label(t!("models.add.title"))
                        .h(px(36.))
                        .px(px(14.))
                        .rounded(px(9.))
                        .child(icon(Icon::Plus, 12., c.graphite))
                        .child(kit::plain(t!("models.add.title"), 13., 16., FontWeight::MEDIUM, c.graphite))
                        .on_click(cx.listener(|this, _, window, cx| this.open_add(window, cx))),
                ),
            ),
        }
    }
}

impl Render for ModelsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c: Colors = cx.paper().colors;
        // The inputs of the add form keep their placeholders, so they change with the language.
        if self.locale != sayso_i18n::locale() {
            self.locale = sayso_i18n::locale();
            if let Some(add) = &self.add {
                add.name.update(cx, |s, cx| s.set_placeholder(t!("models.add.name_placeholder"), window, cx));
                add.model_id.update(cx, |s, cx| s.set_placeholder(t!("models.add.model_placeholder"), window, cx));
                add.key.update(cx, |s, cx| s.set_placeholder(key_placeholder(), window, cx));
            }
        }
        let pad = crate::layout::page_pad(window, &self.model.read(cx).config);
        self.narrow = crate::layout::page_narrow(window, &self.model.read(cx).config);
        let (catalog, roles, used, dir, engine_down) = {
            let m = self.model.read(cx);
            (m.catalog(), Roles::of(m), m.models_disk_bytes(), m.models_dir_display(), m.services.engine.is_none())
        };
        let model = self.model.clone();
        let used_label = if used == 0 { t!("models.page.nothing_downloaded") } else { t!("models.page.used", size = format_size(used)) };
        let disk = Pressable::new("models-dir")
            .label(format!("{used_label} · {dir}"))
            .cursor_pointer()
            .pb(px(4.))
            .on_click(move |_, _, cx| AppModel::open_path(&model.read(cx).paths.models_dir()))
            .child(kit::plain(format!("{used_label} · {dir}"), 13., 16., FontWeight::NORMAL, c.graphite))
            .into_any_element();
        let head = kit::page_head(
            &t!("models.page.title"),
            Some(kit::intro(
                t!(crate::shell::os_text!("models.page.intro_mac", "models.page.intro_windows", "models.page.intro_other")),
                600.,
                cx,
            )),
            Some(disk),
            cx,
        );
        let query = self.query(cx);
        let (sort, reversed) = (self.sort, self.reversed);
        let list = |streaming: bool| -> Vec<ModelInfo> {
            let mut list: Vec<ModelInfo> = catalog.iter().filter(|i| !i.is_remote() && i.live_preview == streaming && matches(i, &query)).cloned().collect();
            sort_models(&mut list, sort, reversed);
            list
        };
        let (non_streaming, streaming) = (list(false), list(true));
        let mut tables = div().flex().flex_col().gap(px(36.)).child(self.toolbar(cx));
        if non_streaming.is_empty() && streaming.is_empty() {
            tables = tables.child(ui(t!("models.search.empty", query = self.search.read(cx).value().trim().to_string()), 14., 20., FontWeight::NORMAL, c.graphite));
        }
        if !non_streaming.is_empty() {
            tables = tables.child(self.group(t!("models.group.non_streaming"), t!("models.group.non_streaming_note"), &non_streaming, &roles, cx));
        }
        if !streaming.is_empty() {
            tables = tables.child(self.group(t!("models.group.streaming"), t!("models.group.streaming_note"), &streaming, &roles, cx));
        }
        let cloud = self.cloud(&roles, cx);
        let setup = self.setup(cx);
        let details = self.details.clone().and_then(|id| self.details_modal(&id, cx));
        div().absolute().inset_0().child(div().id("models").absolute().inset_0().overflow_y_scroll().text_color(c.ink).child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap(px(28.))
                .py(px(pad.min(44.)))
                .px(px(pad))
                .child(head)
                .when(engine_down, |d| {
                    d.child(Banner::new(
                        BannerKind::Warning,
                        t!("models.page.engine_down"),
                    ))
                })
                .child(setup)
                .child(div().pt(px(8.)).child(tables))
                .child(div().pt(px(8.)).child(cloud)),
        )).children(details)
    }
}

#[cfg(test)]
mod tests {
    use super::{SortKey, matches, sort_models};
    use sayso_core::models::{ModelId, ModelInfo, catalog};

    fn ids(list: &[sayso_core::models::ModelInfo]) -> Vec<&str> {
        list.iter().map(|m| m.id.as_str()).collect()
    }

    /// Models with and without a WER and a size, the same on every system.
    fn fixture() -> Vec<ModelInfo> {
        let base = catalog()[0].clone();
        let model = |id: &str, wer_percent: Option<f32>, size_bytes: u64, speed: u8| ModelInfo {
            id: ModelId::new(id),
            wer_percent,
            size_bytes,
            speed,
            ..base.clone()
        };
        vec![
            model("unmeasured-big", None, 900, 2),
            model("good", Some(5.5), 600, 3),
            model("no-size", Some(9.0), 0, 5),
            model("unmeasured-small", None, 100, 4),
            model("best", Some(4.0), 300, 1),
            model("weak", Some(12.0), 200, 3),
        ]
    }

    #[test]
    fn the_best_comes_first_and_unmeasured_models_stay_last() {
        let mut list = fixture();
        sort_models(&mut list, SortKey::Accuracy, false);
        assert_eq!(ids(&list)[..4], ["best", "good", "no-size", "weak"]);
        let wers: Vec<Option<f32>> = list.iter().map(|m| m.wer_percent).collect();
        let measured = wers.iter().take_while(|w| w.is_some()).count();
        assert!(wers[measured..].iter().all(Option::is_none), "{:?}", ids(&list));
        assert!(wers[..measured].windows(2).all(|w| w[0] <= w[1]));

        sort_models(&mut list, SortKey::Accuracy, true);
        assert!(list.last().unwrap().wer_percent.is_none(), "reversed, unmeasured models still go last");
        assert!(list[0].wer_percent >= list[1].wer_percent);

        sort_models(&mut list, SortKey::Size, false);
        let sized: Vec<u64> = list.iter().map(|m| m.size_bytes).take_while(|&b| b > 0).collect();
        assert!(sized.windows(2).all(|w| w[0] <= w[1]));
        assert!(list[sized.len()..].iter().all(|m| m.size_bytes == 0), "no size goes last");
        sort_models(&mut list, SortKey::Speed, false);
        assert!(list.windows(2).all(|w| w[0].speed >= w[1].speed));
    }

    #[test]
    fn recommended_keeps_the_catalog_order_with_the_recommended_model_first() {
        let mut list = catalog().to_vec();
        list.reverse();
        sort_models(&mut list, SortKey::Recommended, false);
        assert!(list[0].recommended);
        let mut expected = catalog().to_vec();
        expected.reverse();
        expected.retain(|m| !m.recommended);
        assert_eq!(ids(&list[1..]), ids(&expected), "the sort is stable");
    }

    #[test]
    fn search_looks_at_name_vendor_and_languages() {
        let m = catalog()[0].clone();
        assert!(matches(&m, ""));
        assert!(matches(&m, &m.name.to_lowercase()));
        assert!(matches(&m, &m.vendor.to_lowercase()));
        assert!(!matches(&m, "no model has this text"));
    }
}
