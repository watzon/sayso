//! Paste "hello from sayso" into the frontmost app after 1.5 s.
use sayso_platform::InsertMethod;
fn main() {
    let platform = sayso_platform_macos::MacPlatform::new(std::env::temp_dir());
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let method = if std::env::args().any(|a| a == "--type") { InsertMethod::Type } else {
        InsertMethod::Paste { restore_clipboard: true, restore_delay: std::time::Duration::from_millis(450) } };
    println!("{:?}", platform.inserter.insert("hello from sayso ", method));
}
