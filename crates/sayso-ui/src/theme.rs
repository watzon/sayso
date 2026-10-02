//! The paper palette as a GPUI global.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Global, Hsla, px, rgb};
use sayso_core::ink::{Appearance, Ink, Palette, Rgb};

/// Palette colors as GPUI colors. Field meanings match [`Palette`].
#[derive(Debug, Clone, Copy)]
pub struct Colors {
    pub ground: Hsla,
    pub sheet: Hsla,
    pub sheet_raised: Hsla,
    pub deboss: Hsla,
    pub deboss_shade: Hsla,
    pub rule: Hsla,
    pub ink: Hsla,
    pub ink_fill: Hsla,
    pub on_ink: Hsla,
    pub graphite: Hsla,
    pub pencil: Hsla,
    pub accent: Hsla,
    pub accent_wash: Hsla,
    pub danger: Hsla,
    pub danger_wash: Hsla,
    pub success: Hsla,
    /// Shadow tint. Use with alpha through [`Colors::shadow`].
    pub shadow_base: Hsla,
    /// Bevel highlight: white in light mode, faint white in dark mode.
    pub highlight_base: Hsla,
    dark: bool,
}

pub fn hsla(c: Rgb) -> Hsla {
    rgb(c.0).into()
}

impl Colors {
    fn from_palette(p: &Palette) -> Self {
        let dark = p.appearance == Appearance::Dark;
        Colors {
            ground: hsla(p.ground),
            sheet: hsla(p.sheet),
            sheet_raised: hsla(p.sheet_raised),
            deboss: hsla(p.deboss),
            deboss_shade: hsla(p.deboss_shade),
            rule: hsla(p.rule),
            ink: hsla(p.ink),
            ink_fill: hsla(p.ink_fill),
            on_ink: hsla(p.on_ink),
            graphite: hsla(p.graphite),
            pencil: hsla(p.pencil),
            accent: hsla(p.accent),
            accent_wash: hsla(p.accent_wash),
            danger: hsla(p.danger),
            danger_wash: hsla(p.danger_wash),
            success: hsla(p.success),
            shadow_base: hsla(p.shadow),
            highlight_base: gpui_kit::white(),
            dark,
        }
    }

    /// The shadow color at an opacity tuned for each mode.
    /// `light_alpha` is the value from the design; dark mode uses a stronger black.
    pub fn shadow(&self, light_alpha: f32) -> Hsla {
        if self.dark { self.shadow_base.opacity((light_alpha * 3.0).min(0.7)) } else { self.shadow_base.opacity(light_alpha) }
    }

    /// The top-edge highlight of a raised surface.
    pub fn highlight(&self, light_alpha: f32) -> Hsla {
        if self.dark { self.highlight_base.opacity(light_alpha * 0.07) } else { self.highlight_base.opacity(light_alpha) }
    }

    pub fn is_dark(&self) -> bool {
        self.dark
    }
}

#[derive(Debug, Clone)]
pub struct PaperTheme {
    pub colors: Colors,
    pub palette: Palette,
    pub ink: Ink,
    pub appearance: Appearance,
    pub reduce_motion: bool,
    pub texture: bool,
}

impl Global for PaperTheme {}

impl PaperTheme {
    pub fn new(ink: Ink, appearance: Appearance, reduce_motion: bool, texture: bool) -> Self {
        let palette = Palette::new(ink, appearance);
        PaperTheme { colors: Colors::from_palette(&palette), palette, ink, appearance, reduce_motion, texture }
    }

    pub fn is_dark(&self) -> bool {
        self.appearance == Appearance::Dark
    }

    /// The colors of the opposite appearance, for previews such as the ink specimens.
    pub fn colors_for(&self, appearance: Appearance) -> Colors {
        Colors::from_palette(&Palette::new(self.ink, appearance))
    }
}

pub trait ActivePaper {
    fn paper(&self) -> &PaperTheme;
}

impl ActivePaper for App {
    fn paper(&self) -> &PaperTheme {
        self.global::<PaperTheme>()
    }
}

pub(crate) fn init(cx: &mut App) {
    set(cx, PaperTheme::new(Ink::default(), Appearance::Light, false, true));
}

/// Install a theme, sync gpui-kit's colors, and redraw every window.
pub fn set(cx: &mut App, theme: PaperTheme) {
    let c = theme.colors;
    let mode = if theme.is_dark() { ThemeMode::Dark } else { ThemeMode::Light };
    Theme::change(mode, None, cx);
    Theme::update(cx, |t| {
        t.font_family = crate::fonts::UI.into();
        t.font_size = px(14.);
        t.mono_font_family = crate::fonts::MONO.into();
        t.mono_font_size = px(13.);
        t.radius = px(8.);
        t.radius_lg = px(12.);
        t.shadow = false;
        let k = &mut t.colors;
        k.background = c.sheet;
        k.foreground = c.ink;
        k.border = c.rule;
        k.input = c.rule;
        k.caret = c.accent;
        k.ring = c.accent;
        k.selection = c.accent_wash;
        k.primary = c.ink_fill;
        k.primary_hover = c.ink_fill.opacity(0.9);
        k.primary_active = c.ink_fill.opacity(0.8);
        k.primary_foreground = c.on_ink;
        k.secondary = c.sheet_raised;
        k.secondary_hover = c.deboss;
        k.secondary_active = c.deboss_shade;
        k.secondary_foreground = c.ink;
        k.accent = c.deboss;
        k.accent_foreground = c.ink;
        k.muted = c.deboss;
        k.muted_foreground = c.graphite;
        k.popover = c.sheet_raised;
        k.popover_foreground = c.ink;
        k.list = c.sheet;
        k.list_hover = c.deboss;
        k.list_active = c.accent_wash;
        k.list_active_border = c.accent;
        k.scrollbar = gpui_kit::transparent_black();
        k.scrollbar_thumb = c.ink.opacity(0.18);
        k.scrollbar_thumb_hover = c.ink.opacity(0.3);
        k.danger = c.danger;
        k.danger_foreground = c.on_ink;
        k.success = c.success;
        k.link = c.accent;
        k.link_hover = c.accent;
        k.link_active = c.accent;
        k.window_border = c.rule;
        k.overlay = c.shadow(0.2);
    });
    cx.set_global(theme);
    cx.refresh_windows();
}
