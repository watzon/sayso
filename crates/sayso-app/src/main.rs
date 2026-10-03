//! Sayso: local dictation for macOS and Linux.

mod app;
mod chrome;
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
mod shell;
mod widgets;

fn main() {
    app::run();
}
