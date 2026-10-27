# sayso-ui

The design system of [Sayso](https://github.com/watzon/sayso): tactile paper and ink on [GPUI](https://www.gpui.rs) and gpui-kit. Extensions for Sayso use it to look like Sayso, and any GPUI app can use it.

- `theme`: the palette of [`sayso-ink`](https://crates.io/crates/sayso-ink) as a GPUI global, synced into gpui-kit.
- `paper`: material shadows (raised sheets, debossed wells, ink buttons).
- `texture`: tiled rag-paper grain.
- `text`: the type scale (Fraunces, Instrument Sans, IBM Plex Mono).
- `components`: controls built from those pieces.
- `l10n`: the few texts of the components in English, and hooks for an app that has its own translations.

## Setup

```rust,no_run
use sayso_ink::{Appearance, Ink};
use sayso_ui::PaperTheme;

fn main() {
    gpui_kit::application().with_assets(sayso_ui::assets::Assets).run(|cx| {
        gpui_kit::init(cx);
        sayso_ui::init(cx);
        sayso_ui::theme::set(cx, PaperTheme::new(Ink::default(), Appearance::Light, false, true));
        // Open your windows here.
    });
}
```

`examples/gallery.rs` shows each component: `cargo run -p sayso-ui --example gallery` in the Sayso repository (add `dark` for the dark appearance).

## License

The code is MIT (see `LICENSE`). The bundled fonts keep their own SIL Open Font License 1.1, in `assets/fonts/licenses`. So the license of the crate is `MIT AND OFL-1.1`.
