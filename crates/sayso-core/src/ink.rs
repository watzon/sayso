//! Inks and the paper palette.
//!
//! The user picks one ink. Sayso derives the accent (focus, recording) and the
//! dark-mode values from it, and checks contrast (plan §3, "Design").

use serde::{Deserialize, Serialize};

/// An sRGB color as `0xRRGGBB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgb(pub u32);

impl Rgb {
    pub const fn new(hex: u32) -> Self {
        Rgb(hex)
    }
    pub fn r(self) -> u8 {
        (self.0 >> 16) as u8
    }
    pub fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }
    pub fn b(self) -> u8 {
        self.0 as u8
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 {
            return None;
        }
        u32::from_str_radix(s, 16).ok().map(Rgb)
    }

    pub fn to_hex(self) -> String {
        format!("#{:06X}", self.0)
    }

    /// WCAG relative luminance.
    pub fn luminance(self) -> f64 {
        fn channel(c: u8) -> f64 {
            let c = c as f64 / 255.0;
            if c <= 0.039_28 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        }
        0.2126 * channel(self.r()) + 0.7152 * channel(self.g()) + 0.0722 * channel(self.b())
    }

    /// WCAG contrast ratio, 1.0 to 21.0.
    pub fn contrast(self, other: Rgb) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }

    fn to_hsl(self) -> (f64, f64, f64) {
        let r = self.r() as f64 / 255.0;
        let g = self.g() as f64 / 255.0;
        let b = self.b() as f64 / 255.0;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        if (max - min).abs() < f64::EPSILON {
            return (0.0, 0.0, l);
        }
        let d = max - min;
        let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
        let h = if max == r {
            ((g - b) / d + if g < b { 6.0 } else { 0.0 }) * 60.0
        } else if max == g {
            ((b - r) / d + 2.0) * 60.0
        } else {
            ((r - g) / d + 4.0) * 60.0
        };
        (h, s, l)
    }

    fn from_hsl(h: f64, s: f64, l: f64) -> Self {
        let h = h.rem_euclid(360.0) / 360.0;
        let hue = |p: f64, q: f64, mut t: f64| {
            if t < 0.0 {
                t += 1.0;
            }
            if t > 1.0 {
                t -= 1.0;
            }
            if t < 1.0 / 6.0 {
                p + (q - p) * 6.0 * t
            } else if t < 0.5 {
                q
            } else if t < 2.0 / 3.0 {
                p + (q - p) * (2.0 / 3.0 - t) * 6.0
            } else {
                p
            }
        };
        let (r, g, b) = if s == 0.0 {
            (l, l, l)
        } else {
            let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
            let p = 2.0 * l - q;
            (hue(p, q, h + 1.0 / 3.0), hue(p, q, h), hue(p, q, h - 1.0 / 3.0))
        };
        let to = |c: f64| (c.clamp(0.0, 1.0) * 255.0).round() as u32;
        Rgb((to(r) << 16) | (to(g) << 8) | to(b))
    }

    /// Mix toward `other`. `t = 0` keeps self, `t = 1` gives `other`.
    pub fn mix(self, other: Rgb, t: f64) -> Rgb {
        let m = |a: u8, b: u8| ((a as f64) + (b as f64 - a as f64) * t).round() as u32;
        Rgb((m(self.r(), other.r()) << 16) | (m(self.g(), other.g()) << 8) | m(self.b(), other.b()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NamedInk {
    #[default]
    Sumi,
    Indigo,
    IronGall,
    Sepia,
    Verdigris,
    Oxblood,
}

impl NamedInk {
    pub const ALL: [NamedInk; 6] =
        [NamedInk::Sumi, NamedInk::Indigo, NamedInk::IronGall, NamedInk::Sepia, NamedInk::Verdigris, NamedInk::Oxblood];

    pub fn name(self) -> &'static str {
        match self {
            NamedInk::Sumi => "Sumi",
            NamedInk::Indigo => "Indigo",
            NamedInk::IronGall => "Iron Gall",
            NamedInk::Sepia => "Sepia",
            NamedInk::Verdigris => "Verdigris",
            NamedInk::Oxblood => "Oxblood",
        }
    }

    pub fn color(self) -> Rgb {
        Rgb(match self {
            NamedInk::Sumi => 0x1D1B18,
            NamedInk::Indigo => 0x2D3C8C,
            NamedInk::IronGall => 0x2F3A4F,
            NamedInk::Sepia => 0x6B4A2E,
            NamedInk::Verdigris => 0x2F5E52,
            NamedInk::Oxblood => 0x6E2A24,
        })
    }
}

/// The ink setting as stored in config: a named ink or a custom hex color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Ink {
    Named(NamedInk),
    Custom(#[serde(with = "hex_rgb")] Rgb),
}

impl Default for Ink {
    fn default() -> Self {
        Ink::Named(NamedInk::Sumi)
    }
}

mod hex_rgb {
    use super::Rgb;
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(c: &Rgb, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&c.to_hex())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Rgb, D::Error> {
        let s = String::deserialize(d)?;
        Rgb::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("\"{s}\" is not a #RRGGBB color")))
    }
}

impl Ink {
    pub fn color(self) -> Rgb {
        match self {
            Ink::Named(n) => n.color(),
            Ink::Custom(c) => c,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    Light,
    Dark,
}

/// Every color the UI uses. Field names match the Paper design tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub appearance: Appearance,
    /// Window background and sidebar (the "board").
    pub ground: Rgb,
    /// Raised content sheet.
    pub sheet: Rgb,
    /// Top-most sheet: cards, popovers, the selected row.
    pub sheet_raised: Rgb,
    /// Inside of a pressed impression: fields, wells, tracks.
    pub deboss: Rgb,
    pub deboss_shade: Rgb,
    pub rule: Rgb,
    /// Primary text and the waveform.
    pub ink: Rgb,
    /// Fill of primary buttons. Text on it uses `on_ink`.
    pub ink_fill: Rgb,
    pub on_ink: Rgb,
    pub graphite: Rgb,
    pub pencil: Rgb,
    pub accent: Rgb,
    pub accent_wash: Rgb,
    pub danger: Rgb,
    pub danger_wash: Rgb,
    pub success: Rgb,
    /// Color of drop shadows (used with alpha).
    pub shadow: Rgb,
}

/// Where a custom ink fails contrast. Settings shows this as a warning.
#[derive(Debug, Clone, PartialEq)]
pub struct ContrastReport {
    pub light_ratio: f64,
    pub dark_ratio: f64,
}

impl ContrastReport {
    /// WCAG AA for body text.
    pub fn passes(&self) -> bool {
        self.light_ratio >= 4.5 && self.dark_ratio >= 4.5
    }
}

const SUMI_ACCENT_LIGHT: Rgb = Rgb(0x2D3C8C);
const SUMI_ACCENT_DARK: Rgb = Rgb(0x8D9BE6);

fn derived_accent(ink: Rgb, appearance: Appearance) -> Rgb {
    let (h, s, _) = ink.to_hsl();
    if s < 0.15 {
        // Near-black or gray inks have no hue to work from. Use indigo.
        return match appearance {
            Appearance::Light => SUMI_ACCENT_LIGHT,
            Appearance::Dark => SUMI_ACCENT_DARK,
        };
    }
    if (h - 230.0).abs() < 20.0 && s > 0.3 {
        // Indigo ink: the accent moves to a warm ochre so recording still stands out.
        return match appearance {
            Appearance::Light => Rgb(0x8A5A12),
            Appearance::Dark => Rgb(0xE0B464),
        };
    }
    // Other hues: the complementary hue, at a fixed lightness per mode.
    match appearance {
        Appearance::Light => Rgb::from_hsl(h + 180.0, 0.48, 0.36),
        Appearance::Dark => Rgb::from_hsl(h + 180.0, 0.6, 0.72),
    }
}

impl Palette {
    pub fn new(ink: Ink, appearance: Appearance) -> Self {
        let base = ink.color();
        match appearance {
            Appearance::Light => {
                let accent = derived_accent(base, appearance);
                let sheet = Rgb(0xF7F4EE);
                Palette {
                    appearance,
                    ground: Rgb(0xE9E4DA),
                    sheet,
                    sheet_raised: Rgb(0xFCFAF6),
                    deboss: Rgb(0xE2DCD0),
                    deboss_shade: Rgb(0xCFC8BB),
                    rule: Rgb(0xD8D1C4),
                    ink: base,
                    ink_fill: base,
                    on_ink: sheet,
                    graphite: Rgb(0x6A655C),
                    pencil: Rgb(0x8E887D),
                    accent,
                    accent_wash: accent.mix(sheet, 0.84),
                    danger: Rgb(0x8E2F22),
                    danger_wash: Rgb(0xF3E7DC),
                    success: Rgb(0x4E7A4A),
                    shadow: Rgb(0x3C2E1C),
                }
            }
            Appearance::Dark => {
                let accent = derived_accent(base, appearance);
                let sheet = Rgb(0x201F1C);
                // Pale ink, tinted a little toward the chosen ink.
                let pale = Rgb(0xECE6DA).mix(Rgb::from_hsl(base.to_hsl().0, 0.4, 0.85), 0.12);
                Palette {
                    appearance,
                    ground: Rgb(0x171614),
                    sheet,
                    sheet_raised: Rgb(0x292825),
                    deboss: Rgb(0x141311),
                    deboss_shade: Rgb(0x0E0D0C),
                    rule: Rgb(0x34322E),
                    ink: pale,
                    ink_fill: pale,
                    on_ink: sheet,
                    graphite: Rgb(0xA39D91),
                    pencil: Rgb(0x7D776C),
                    accent,
                    accent_wash: accent.mix(sheet, 0.82),
                    danger: Rgb(0xE08A7C),
                    danger_wash: Rgb(0x3A2420),
                    success: Rgb(0x8FB98A),
                    shadow: Rgb(0x000000),
                }
            }
        }
    }

    pub fn contrast_report(ink: Ink) -> ContrastReport {
        let light = Palette::new(ink, Appearance::Light);
        let dark = Palette::new(ink, Appearance::Dark);
        ContrastReport { light_ratio: light.ink.contrast(light.sheet), dark_ratio: dark.ink.contrast(dark.sheet) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_matches_wcag_reference() {
        let r = Rgb(0x000000).contrast(Rgb(0xFFFFFF));
        assert!((r - 21.0).abs() < 0.01);
        assert!((Rgb(0x777777).contrast(Rgb(0xFFFFFF)) - 4.48).abs() < 0.05);
    }

    #[test]
    fn every_named_ink_passes_aa_in_both_modes() {
        for ink in NamedInk::ALL {
            let report = Palette::contrast_report(Ink::Named(ink));
            assert!(report.passes(), "{} fails: {report:?}", ink.name());
            for appearance in [Appearance::Light, Appearance::Dark] {
                let p = Palette::new(Ink::Named(ink), appearance);
                assert!(p.graphite.contrast(p.sheet) >= 4.5, "{} graphite {appearance:?}", ink.name());
                assert!(p.accent.contrast(p.sheet) >= 3.0, "{} accent {appearance:?}", ink.name());
                assert!(p.on_ink.contrast(p.ink_fill) >= 4.5, "{} button text {appearance:?}", ink.name());
            }
        }
    }

    #[test]
    fn sumi_uses_indigo_accent() {
        assert_eq!(Palette::new(Ink::default(), Appearance::Light).accent, Rgb(0x2D3C8C));
    }

    #[test]
    fn a_pale_custom_ink_fails_contrast() {
        assert!(!Palette::contrast_report(Ink::Custom(Rgb(0xD0D0D0))).passes());
    }

    #[test]
    fn ink_config_round_trips() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct W {
            ink: Ink,
        }
        let named: W = toml::from_str("ink = \"iron_gall\"").unwrap();
        assert_eq!(named.ink, Ink::Named(NamedInk::IronGall));
        let custom: W = toml::from_str("ink = \"#123456\"").unwrap();
        assert_eq!(custom.ink, Ink::Custom(Rgb(0x123456)));
        assert_eq!(toml::to_string(&custom).unwrap().trim(), "ink = \"#123456\"");
    }
}
