# sayso-ink

The inks, the palette, and the appearance types of [Sayso](https://github.com/watzon/sayso).

The user picks one ink: a named ink (`NamedInk`) or a custom color. `Palette::new(ink, appearance)` makes every color of the paper-and-ink interface from it, for the light or the dark appearance. `Palette::contrast_report(ink)` tells whether a custom ink has enough contrast for body text (WCAG AA) in both appearances.

```rust
use sayso_ink::{Appearance, Ink, Palette};

let palette = Palette::new(Ink::default(), Appearance::Dark);
println!("text {} on {}", palette.ink.to_hex(), palette.sheet.to_hex());
assert!(Palette::contrast_report(Ink::default()).passes());
```

`Ink` and `Appearance` have serde support, so an app can keep them in its settings. The only dependency is serde. [`sayso-ui`](https://crates.io/crates/sayso-ui) draws the interface with these colors on GPUI.

## License

MIT. See `LICENSE`.
