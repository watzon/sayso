//! Sayso: local dictation for macOS.

mod app;
mod dev;
mod dictation;
mod hub;
mod icons;
mod live;
mod model;
mod model_picker;
mod onboarding;
mod overlay;
mod pindrop_import;
mod popover;
mod services;
mod widgets;

fn main() {
    app::run();
}
