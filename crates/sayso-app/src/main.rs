//! Sayso: local dictation.

// A release build on Windows is a GUI program: no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod caption;
mod dev;
mod dictation;
mod hub;
mod icons;
mod live;
mod model;
mod model_picker;
mod onboarding;
mod os;
mod overlay;
mod pindrop_import;
mod popover;
mod services;
mod widgets;

fn main() {
    app::run();
}
