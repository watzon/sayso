//! Static pieces: keycaps, tags, badges, seals, banners, app badges, dots.

use crate::ActivePaper;
use crate::assets::Icon;
use crate::fonts::{MONO, UI};
use crate::paper::{self, PaperStyled};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::hotkey::Hotkey;

pub fn icon(icon: Icon, size: f32, color: Hsla) -> Svg {
    svg().path(icon.path()).size(px(size)).flex_none().text_color(color)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeySize {
    Small,
    #[default]
    Medium,
    Large,
}

/// A row of keycaps, for example ⌥ + Space.
#[derive(IntoElement)]
pub struct Keycaps {
    caps: Vec<SharedString>,
    size: KeySize,
    /// Show the key that is held down now as pressed ink.
    pressed: Vec<bool>,
    plus: bool,
}

impl Keycaps {
    pub fn new(caps: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        let caps: Vec<SharedString> = caps.into_iter().map(Into::into).collect();
        let pressed = vec![false; caps.len()];
        Self { caps, size: KeySize::Medium, pressed, plus: false }
    }
    pub fn hotkey(hotkey: &Hotkey) -> Self {
        Self::new(hotkey.keycaps())
    }
    pub fn size(mut self, size: KeySize) -> Self {
        self.size = size;
        self
    }
    /// Draw "+" between keys (the large onboarding style).
    pub fn with_plus(mut self) -> Self {
        self.plus = true;
        self
    }
    pub fn pressed(mut self, pressed: Vec<bool>) -> Self {
        self.pressed = pressed;
        self
    }
}

impl RenderOnce for Keycaps {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.paper().colors;
        let (h, pad, text, radius, gap) = match self.size {
            KeySize::Small => (20., 6., 11., 5., 3.),
            KeySize::Medium => (26., 8., 12., 6., 4.),
            KeySize::Large => (52., 18., 18., 10., 10.),
        };
        let n = self.caps.len();
        let mut row = div().flex().items_center().gap(px(gap));
        for (i, cap) in self.caps.into_iter().enumerate() {
            let is_pressed = self.pressed.get(i).copied().unwrap_or(false);
            let wide = cap.chars().count() > 1;
            let mut key = div()
                .flex()
                .items_center()
                .justify_center()
                .h(px(h))
                .min_w(px(h))
                .px(px(if wide { pad + 2. } else { pad }))
                .rounded(px(radius))
                .font_family(MONO)
                .text_size(px(text))
                .font_weight(FontWeight::MEDIUM)
                .child(cap);
            key = if is_pressed {
                key.bg(c.ink_fill).text_color(c.on_ink).shadow(paper::ink_pressed(&c))
            } else {
                key.keycap(&c).text_color(c.ink)
            };
            if self.size == KeySize::Large && wide {
                key = key.min_w(px(150.));
            }
            row = row.child(key);
            if self.plus && i + 1 < n {
                row = row.child(div().text_size(px(16.)).text_color(c.pencil).child("+"));
            }
        }
        row
    }
}

/// A debossed tag with a colored dot: the style tag in rows.
#[derive(IntoElement)]
pub struct StyleTag {
    label: SharedString,
    dot: Option<Hsla>,
    width: Option<f32>,
}

impl StyleTag {
    pub fn new(label: impl Into<SharedString>, dot: Option<Hsla>) -> Self {
        Self { label: label.into(), dot, width: None }
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }
}

impl RenderOnce for StyleTag {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.paper().colors;
        let mut d = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(22.))
            .px(px(9.))
            .rounded_full()
            .debossed(&c)
            .when_some(self.dot, |d, dot| d.child(div().size(px(6.)).rounded_full().bg(dot)))
            .when(self.dot.is_none(), |d| {
                d.child(div().size(px(6.)).rounded_full().border_1().border_color(c.pencil))
            })
            .child(div().font_family(UI).text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(c.graphite).child(self.label));
        if let Some(w) = self.width {
            d = d.w(px(w));
        }
        d
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeTone {
    Accent,
    Ink,
    Muted,
}

/// A small caps badge: "CLOUD", "RECOMMENDED", "ACTIVE".
#[derive(IntoElement)]
pub struct Badge {
    label: SharedString,
    tone: BadgeTone,
    icon: Option<Icon>,
}

impl Badge {
    pub fn new(label: impl Into<SharedString>, tone: BadgeTone) -> Self {
        Self { label: label.into(), tone, icon: None }
    }
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
}

impl RenderOnce for Badge {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.paper().colors;
        let (bg, fg) = match self.tone {
            BadgeTone::Accent => (c.accent_wash, c.accent),
            BadgeTone::Ink => (c.ink_fill, c.on_ink),
            BadgeTone::Muted => (c.deboss, c.graphite),
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .h(px(22.))
            .px(px(8.))
            .rounded_full()
            .bg(bg)
            .when_some(self.icon, |d, i| d.child(icon(i, 11., fg)))
            .child(
                div()
                    .font_family(UI)
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(fg)
                    .child(SharedString::from(self.label.to_uppercase())),
            )
    }
}

/// The domed seal with a check. Indigo by default (the "Inserted" stamp).
#[derive(IntoElement)]
pub struct Seal {
    size: f32,
    color: Option<Hsla>,
    ring: bool,
}

impl Seal {
    pub fn new(size: f32) -> Self {
        Self { size, color: None, ring: false }
    }
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
    /// The large onboarding seal with an inner ring and halo.
    pub fn ringed(mut self) -> Self {
        self.ring = true;
        self
    }
}

impl RenderOnce for Seal {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.paper().colors;
        let color = self.color.unwrap_or(c.accent);
        let shadows = paper::seal(&c);
        let check = icon(Icon::Check, self.size * 0.5, c.on_ink);
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .relative()
            .size(px(self.size))
            .rounded_full()
            .bg(color)
            .shadow(shadows)
            .map(|d| {
                if self.ring {
                    d.child(halo(self.size, 6., color.opacity(0.10))).child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(self.size * 0.76))
                            .rounded_full()
                            .border_1()
                            .border_color(c.on_ink.opacity(0.35))
                            .child(check),
                    )
                } else {
                    d.child(check)
                }
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerKind {
    Warning,
    Info,
}

/// A notice row: a hotkey conflict, a permission hint.
#[derive(IntoElement)]
pub struct Banner {
    kind: BannerKind,
    text: SharedString,
}

impl Banner {
    pub fn new(kind: BannerKind, text: impl Into<SharedString>) -> Self {
        Self { kind, text: text.into() }
    }
}

impl RenderOnce for Banner {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.paper().colors;
        let (bg, fg, border, ic) = match self.kind {
            BannerKind::Warning => (c.danger_wash, c.danger, c.danger.opacity(0.18), Icon::Warning),
            BannerKind::Info => (gpui_kit::transparent_black(), c.graphite, c.rule, Icon::Info),
        };
        div()
            .flex()
            .items_start()
            .gap(px(12.))
            .px(px(14.))
            .py(px(12.))
            .rounded(px(10.))
            .bg(bg)
            .border_1()
            .border_color(border)
            .child(icon(ic, 16., fg).mt(px(1.)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(UI)
                    .text_size(px(13.))
                    .line_height(px(19.))
                    .text_color(if self.kind == BannerKind::Warning && !c.is_dark() { hsla(0.02, 0.6, 0.23, 1.0) } else { fg })
                    .child(self.text),
            )
    }
}

/// A rounded square with an app's initial or icon.
#[derive(IntoElement)]
pub struct AppBadge {
    name: SharedString,
    icon_png: Option<std::sync::Arc<Image>>,
    site_png: Option<std::sync::Arc<Image>>,
    size: f32,
}

impl AppBadge {
    pub fn new(name: impl Into<SharedString>) -> Self {
        Self { name: name.into(), icon_png: None, site_png: None, size: 28. }
    }
    pub fn icon(mut self, png: Option<std::sync::Arc<Image>>) -> Self {
        self.icon_png = png;
        self
    }
    /// The icon of a site, on a small plate at the bottom right corner.
    pub fn site(mut self, png: Option<std::sync::Arc<Image>>) -> Self {
        self.site_png = png;
        self
    }
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
}

/// A stable color per app name, from muted tones that suit the paper.
pub fn app_color(name: &str) -> Hsla {
    const TONES: [u32; 8] = [0x4A154B, 0x2B6CB0, 0x1F1F1F, 0xE3B341, 0x5E6AD2, 0x3F6B5C, 0x7A4A2B, 0x6E2A24];
    let h = name.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    rgb(TONES[(h % TONES.len() as u32) as usize]).into()
}

impl RenderOnce for AppBadge {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let size = px(self.size);
        // The plate is white in each theme: a site makes its icon for a light tab.
        let site = self.site_png.map(|png| {
            let plate = (self.size * 0.54).round();
            let c = cx.paper().colors;
            div()
                .absolute()
                .right(px(-plate * 0.3))
                .bottom(px(-plate * 0.3))
                .flex()
                .items_center()
                .justify_center()
                .size(px(plate))
                .rounded(px(plate * 0.28))
                .bg(gpui_kit::white())
                .shadow(vec![BoxShadow::new(px(0.), px(1.), c.shadow(0.28)).blur_radius(px(2.))])
                .child(img(png).size(px(plate - 3.)).rounded(px(plate * 0.2)))
        });
        if let Some(png) = self.icon_png {
            return div().relative().flex_none().size(size).child(img(png).size(size)).children(site).into_any_element();
        }
        let initial: SharedString = self.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default().into();
        div()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(size)
            .rounded(px(self.size * 0.25))
            .bg(app_color(&self.name))
            .shadow(vec![BoxShadow::new(px(0.), px(-1.), gpui_kit::black().opacity(0.25)).inset()])
            .font_family(UI)
            .text_size(px(self.size * 0.46))
            .font_weight(FontWeight::BOLD)
            .text_color(gpui_kit::white())
            .child(initial)
            .children(site)
            .into_any_element()
    }
}

/// A status dot with a soft halo. The halo is a second circle, because GPUI
/// does not round the spread of a shadow.
pub fn status_dot(color: Hsla, size: f32) -> Div {
    let halo = size + 6.;
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(halo))
        .rounded_full()
        .bg(color.opacity(0.16))
        .child(div().size(px(size)).rounded_full().bg(color))
}

/// A round halo around a circle of `size`. Add it as the first child of a
/// `.relative()` circle. A spread box shadow is not round: GPUI keeps the
/// element's corner radius when it grows the shadow, so it draws a rounded square.
pub fn halo(size: f32, spread: f32, color: Hsla) -> Div {
    div()
        .absolute()
        .top(px(-spread))
        .left(px(-spread))
        .size(px(size + spread * 2.))
        .rounded_full()
        .bg(color)
}

/// A thin horizontal rule.
pub fn rule(c: &crate::Colors) -> Div {
    div().h(px(1.)).w_full().bg(c.rule)
}
