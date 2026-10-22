//! Sayso: local dictation.

// A release build on Windows is a GUI program: no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod a11y;
mod app;
mod caption;
mod chrome;
mod cleanup_models;
mod dev;
mod devices;
mod diagnostics;
mod dictation;
mod engine_text;
mod hub;
mod icons;
mod layout;
mod live;
mod media;
mod memory;
mod model;
mod model_picker;
mod onboarding;
mod overlay;
mod pindrop_import;
mod popover;
mod services;
mod shell;
mod site_icons;
mod style_files;
mod update_ui;
mod whats_new;
mod widgets;

fn main() {
    app::run();
}
