//! Accessibility on top of GPUI's AccessKit support. See `docs/accessibility.md`.
//!
//! GPUI puts an element in the accessibility tree only when it has an id and a
//! role. [`Pressable`] is the focusable base of every control. [`A11yText`] is
//! text with a node. [`A11yExt`] sets the properties that GPUI has no builder
//! for: the disabled state and live regions.

use crate::ActivePaper as _;
use gpui_kit::accesskit::{self, Live};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::hash::{Hash, Hasher};
use std::panic::Location;

pub use gpui_kit::{Role, Toggled};

/// Properties that GPUI has no builder for.
pub trait A11yExt: StatefulInteractiveElement {
    /// Change the node of this element after GPUI writes it. An element has
    /// one such closure: a second call replaces the first. It runs only when
    /// the element has an id and a role.
    fn a11y_node(self, f: impl FnOnce(&mut accesskit::Node) + 'static) -> Self {
        self.a11y_synthetic_children(move |b| f(b.parent_node()))
    }

    /// A live node. A screen reader reads `value` when the node appears and
    /// each time the value changes, also when the window is not in front.
    /// macOS reads the value, not the label. Use it for a message that shows
    /// after an event: an error, a question, "Copied". For a state that is
    /// on the page from the start, use [`LiveStatus`].
    fn a11y_live(self, value: impl Into<SharedString>, assertive: bool) -> Self {
        let live = if assertive { Live::Assertive } else { Live::Polite };
        self.aria_value(value).a11y_node(move |n| n.set_live(live))
    }

    /// A node that a screen reader reads as dimmed and does not press.
    fn a11y_disabled(self, disabled: bool) -> Self {
        if disabled { self.a11y_node(|n| n.set_disabled()) } else { self }
    }

    /// A radio button: `index` of `count` in its group.
    fn a11y_radio(self, label: impl Into<SharedString>, checked: bool, index: usize, count: usize) -> Self {
        self.role(Role::RadioButton)
            .aria_label(label)
            .aria_toggled(if checked { Toggled::True } else { Toggled::False })
            .aria_selected(checked)
            .aria_position_in_set(index + 1)
            .aria_size_of_set(count)
    }
}

impl<T: StatefulInteractiveElement> A11yExt for T {}

/// An element id from the call site and a value. Text helpers use it, so the
/// same helper with the same text under one parent id gives one id twice. In a
/// list, give each row an id from its data.
#[track_caller]
pub fn call_site_id(value: &str) -> ElementId {
    let at = Location::caller();
    let mut h = std::hash::DefaultHasher::new();
    (at.file(), at.line(), at.column(), value).hash(&mut h);
    ElementId::Integer(h.finish())
}

/// Text with an accessibility node: a label, or a heading with a level.
pub struct A11yText {
    id: ElementId,
    text: SharedString,
    heading: Option<usize>,
}

impl A11yText {
    #[track_caller]
    pub fn new(text: impl Into<SharedString>) -> Self {
        let text = text.into();
        Self { id: call_site_id(&text), text, heading: None }
    }

    /// The same text with a heading role at `level` (1 is the page title).
    pub fn heading(mut self, level: usize) -> Self {
        self.heading = Some(level);
        self
    }

    pub fn with_id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }
}

impl IntoElement for A11yText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for A11yText {
    type RequestLayoutState = <SharedString as Element>::RequestLayoutState;
    type PrepaintState = <SharedString as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn a11y_role(&self) -> Option<Role> {
        Some(if self.heading.is_some() { Role::Heading } else { Role::Label })
    }

    fn write_a11y_info(&self, node: &mut accesskit::Node) {
        // Text inside a live region is not live itself: the region carries
        // the words to announce.
        node.set_live(Live::Off);
        match self.heading {
            Some(level) => {
                node.set_label(self.text.to_string());
                node.set_level(level);
            }
            None => node.set_value(self.text.to_string()),
        }
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.text.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.text.prepaint(id, inspector_id, bounds, layout, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.text.paint(id, inspector_id, bounds, layout, prepaint, window, cx)
    }
}

/// The focus ring: two rings of ink outside the control, on top of its own
/// shadows. It shows only when the focus came from the keyboard.
pub fn focus_ring(base: &[BoxShadow], cx: &App) -> Vec<BoxShadow> {
    let c = cx.paper().colors;
    let mut shadows = base.to_vec();
    shadows.push(BoxShadow::new(px(0.), px(0.), c.sheet).spread_radius(px(2.)));
    shadows.push(BoxShadow::new(px(0.), px(0.), c.ink.opacity(0.85)).spread_radius(px(4.)));
    shadows
}

/// A focusable element that a click, Enter, Space, or an assistive client
/// presses. The default role is [`Role::Button`]. Unlike `base::Button`, it
/// adds no layout, so it can replace a clickable `div` without a change in
/// the look.
#[derive(IntoElement)]
pub struct Pressable {
    id: ElementId,
    base: Stateful<Div>,
    label: Option<SharedString>,
    disabled: bool,
    tab_stop: bool,
    shadows: Vec<BoxShadow>,
    on_click: Option<crate::components::OnClick>,
    focus: Option<FocusHandle>,
}

impl Pressable {
    pub fn new(id: impl Into<ElementId>) -> Self {
        let id = id.into();
        Self { base: div().id(id.clone()).role(Role::Button), id, label: None, disabled: false, tab_stop: true, shadows: Vec::new(), on_click: None, focus: None }
    }

    /// The name that a screen reader reads. Without it, the reader uses the
    /// text inside, if that text has nodes.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The same as `.role(..)`: the last call wins.
    pub fn a11y_role(self, role: Role) -> Self {
        self.role(role)
    }

    /// A disabled control ignores clicks, is not a tab stop, and reads as dimmed.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// False for a control that the keyboard reaches through a sibling, for
    /// example the − and + of a stepper.
    pub fn tab_stop(mut self, tab_stop: bool) -> Self {
        self.tab_stop = tab_stop;
        self
    }

    /// The action for a click, Enter, Space, and the press of an assistive
    /// client. A disabled control drops it.
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(std::rc::Rc::new(f));
        self
    }

    /// Use a focus handle that the view owns, so the view can move the focus
    /// here, for example back to the trigger when a dialog closes.
    pub fn track_focus_handle(mut self, handle: &FocusHandle) -> Self {
        self.focus = Some(handle.clone());
        self
    }

    /// The shadows of the control, so that the focus ring adds to them.
    pub fn shadows(mut self, shadows: Vec<BoxShadow>) -> Self {
        self.shadows = shadows;
        self
    }
}

impl Styled for Pressable {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl ParentElement for Pressable {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.base.extend(elements);
    }
}

impl InteractiveElement for Pressable {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for Pressable {}

impl RenderOnce for Pressable {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = match self.focus {
            Some(handle) => handle,
            None => window.use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle()).read(cx).clone(),
        };
        let ring = focus_ring(&self.shadows, cx);
        let mut el = self.base.when_some(self.label, |el, label| el.aria_label(label));
        if !self.shadows.is_empty() {
            el = el.shadow(self.shadows);
        }
        if self.disabled {
            // Clicks stop here, so a disabled control inside a clickable row
            // does not press the row.
            el.a11y_disabled(true).on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        } else {
            el.track_focus(&focus.tab_stop(self.tab_stop))
                .focus_visible(move |s| s.shadow(ring))
                .when_some(self.on_click, |el, f| {
                    // GPUI answers the Click of an assistive client with a
                    // mouse click at the center of the node, which misses
                    // when the control is scrolled out of view. Call the
                    // handler directly instead.
                    let g = f.clone();
                    el.on_click(move |e, w, cx| f(e, w, cx)).on_a11y_action(AccessibleAction::Click, move |_, w, cx| {
                        let e = ClickEvent::Keyboard(KeyboardClickEvent { button: KeyboardButton::Enter, bounds: Bounds::default() });
                        g(&e, w, cx)
                    })
                })
        }
    }
}

/// Shorthand for [`Pressable::new`].
pub fn pressable(id: impl Into<ElementId>) -> Pressable {
    Pressable::new(id)
}

/// The keycaps of a hotkey as words for a screen reader: "Option Space",
/// not "⌥ Space".
pub fn spoken_keys<S: AsRef<str>>(caps: &[S]) -> String {
    let word = |cap: &str| -> String {
        let named = |sym: &str| -> Option<String> {
            Some(match sym {
                "⌃" => crate::l10n::text("a11y.key.control"),
                "⌥" => crate::l10n::text("a11y.key.option"),
                "⇧" => crate::l10n::text("a11y.key.shift"),
                "⌘" => crate::l10n::text("a11y.key.command"),
                "↵" => crate::l10n::text("a11y.key.return"),
                "⇥" => crate::l10n::text("a11y.key.tab"),
                "⌫" => crate::l10n::text("a11y.key.delete"),
                _ => return None,
            })
        };
        if let Some(w) = named(cap) {
            return w;
        }
        // "Right ⌥" and the like.
        cap.split(' ').map(|part| named(part).unwrap_or_else(|| part.to_string())).collect::<Vec<_>>().join(" ")
    };
    caps.iter().map(|c| word(c.as_ref())).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::{call_site_id, spoken_keys};

    #[test]
    fn keycaps_read_as_words() {
        assert_eq!(spoken_keys(&["⌥", "Space"]), "Option Space");
        assert_eq!(spoken_keys(&["⌃", "⇧", "⌘", "K"]), "Control Shift Command K");
        assert_eq!(spoken_keys(&["Right ⌥"]), "Right Option");
        assert_eq!(spoken_keys(&["Ctrl", "Alt", "Space"]), "Ctrl Alt Space");
    }

    #[test]
    fn call_site_ids_differ_by_place_and_text() {
        let at = |s: &str| call_site_id(s);
        // One call site: the text decides.
        assert_eq!(at("Home"), at("Home"));
        assert_ne!(at("Home"), at("History"));
        // Two call sites with the same text differ.
        assert_ne!(call_site_id("Home"), call_site_id("Home"));
    }
}

/// A status that a screen reader reads when its value changes, but not when
/// it first shows: the state of a model, the engine, a permission. A page
/// with many of them stays quiet when it opens. The default role is
/// [`Role::Status`].
#[derive(IntoElement)]
pub struct LiveStatus {
    id: ElementId,
    base: Stateful<Div>,
    value: SharedString,
}

impl LiveStatus {
    pub fn new(id: impl Into<ElementId>, value: impl Into<SharedString>) -> Self {
        let id = id.into();
        let value = value.into();
        Self { base: div().id(id.clone()).role(Role::Status).aria_label(value.clone()), id, value }
    }
}

impl Styled for LiveStatus {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl ParentElement for LiveStatus {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.base.extend(elements);
    }
}

impl InteractiveElement for LiveStatus {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for LiveStatus {}

impl RenderOnce for LiveStatus {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // The first value of this element, or `None` after the first change.
        // Element state goes away when the element is not drawn, so a status
        // that shows again is quiet again.
        let first = window.use_keyed_state(self.id.clone(), cx, |_, _| Some(self.value.clone()));
        let changed = match first.read(cx).clone() {
            None => true,
            Some(v) if v != self.value => {
                first.update(cx, |f, _| *f = None);
                true
            }
            Some(_) => false,
        };
        let live = if changed { Live::Polite } else { Live::Off };
        self.base.aria_value(self.value).a11y_node(move |n| n.set_live(live))
    }
}
