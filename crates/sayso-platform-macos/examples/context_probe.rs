//! Prints the installed apps and the page address of every running app.
//!
//! Run: `cargo run -p sayso-platform-macos --example context_probe`
//!
//! `page_url` needs the Accessibility permission for the host process. Without it
//! every address is `None`. The example never requests the permission.

// The crate is empty on other systems, so the example is too.
#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use sayso_platform::ContextProvider;
    use sayso_platform_macos::context::MacContext;
    use std::time::Instant;

    let context = MacContext;
    let installed = context.installed_apps();
    println!("installed apps: {}", installed.len());
    for app in installed.iter().take(10) {
        println!("  {} ({})", app.name, app.bundle_id);
    }

    println!("reads_page_url: {}", context.reads_page_url());
    for app in context.running_apps() {
        let start = Instant::now();
        let url = context.page_url(&app);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        println!("{:>8.1} ms  {} ({}, pid {}): {:?}", ms, app.name, app.bundle_id, app.pid, url);
    }
}
