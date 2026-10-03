//! Drive the Linux inserter by hand, for end-to-end checks in a real or
//! virtual session.
//!
//! Each argument is one step, run in order:
//! - `paste:TEXT` pastes TEXT and restores the clipboard after 200 ms.
//! - `type:TEXT` types TEXT.
//! - `copy:TEXT` puts TEXT on the clipboard.
//! - `run:COMMAND` runs COMMAND with `sh -c` while Sayso still serves the
//!   clipboard (for example `xclip -o` or `wl-paste`).
//! - `sleep:MS` waits.
//! - `state` prints the permission state of key injection.
//! - `hold-keyboard` adds a virtual keyboard that stays until the probe
//!   ends. A headless compositor has no keyboard, so its apps have none
//!   either; this one plays the user's physical keyboard.
//! - `mutter-session` starts a Mutter remote desktop session (the backend
//!   of the GNOME portal) and keeps it until the probe ends. It gives a
//!   headless Mutter a keyboard.
//! - `mutter-keys:TEXT` sends TEXT as keysyms through that session, as the
//!   portal would; `mutter-keys:shift+insert` (or another paste key) sends
//!   that key.
//!
//! `\n` in TEXT is a line break.
//!
//! ```sh
//! cargo run -p sayso-platform-linux --example insert_probe -- 'paste:Hello' 'type: Grüße\n'
//! ```

#[cfg(target_os = "linux")]
fn main() {
    use sayso_platform::{InsertMethod, TextInserter};
    use sayso_platform_linux::{inserter, session};
    use std::time::Duration;

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let session = session::prepare();
    let inserter = inserter::LinuxInserter::new(session);
    for step in std::env::args().skip(1) {
        let (verb, arg) = step.split_once(':').unwrap_or((step.as_str(), ""));
        let text = arg.replace("\\n", "\n");
        match verb {
            "paste" => {
                let method = InsertMethod::Paste { restore_clipboard: true, restore_delay: Duration::from_millis(200) };
                println!("paste {text:?}: {:?}", inserter.insert(&text, method));
            }
            "type" => println!("type {text:?}: {:?}", inserter.insert(&text, InsertMethod::Type)),
            "copy" => {
                inserter.copy_to_clipboard(&text);
                println!("copy {text:?}: done");
            }
            "run" => {
                let output = std::process::Command::new("sh").arg("-c").arg(arg).output();
                match output {
                    Ok(o) => println!("run {arg:?}: {:?}", String::from_utf8_lossy(&o.stdout)),
                    Err(e) => println!("run {arg:?}: {e}"),
                }
            }
            "sleep" => std::thread::sleep(Duration::from_millis(arg.parse().unwrap_or(500))),
            "state" => println!("injection state: {:?}", inserter::injection_state(session)),
            "hold-keyboard" => {
                std::thread::spawn(hold::keyboard);
                std::thread::sleep(Duration::from_millis(300));
            }
            "mutter-session" => println!("mutter session: {:?}", mutter::start()),
            "mutter-keys" => println!("mutter keys {text:?}: {:?}", mutter::keys(&text)),
            other => println!("unknown step {other:?}"),
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {}

/// A virtual keyboard that only exists, for headless compositors.
#[cfg(target_os = "linux")]
mod hold {
    use std::io::Write as _;
    use std::os::fd::{AsFd, FromRawFd, OwnedFd};
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::{wl_registry::WlRegistry, wl_seat::WlSeat};
    use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
    use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
        zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
    };

    struct Ignore;
    impl Dispatch<WlRegistry, GlobalListContents> for Ignore {
        fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
    }
    delegate_noop!(Ignore: ignore WlSeat);
    delegate_noop!(Ignore: ZwpVirtualKeyboardManagerV1);
    delegate_noop!(Ignore: ZwpVirtualKeyboardV1);

    const KEYMAP: &str = "xkb_keymap { xkb_keycodes { minimum = 8; maximum = 9; <K1> = 9; }; \
        xkb_types { include \"complete\" }; xkb_compatibility { include \"complete\" }; \
        xkb_symbols { key <K1> {[ a ]}; }; };";

    pub fn keyboard() {
        let conn = Connection::connect_to_env().expect("no Wayland display");
        let (globals, mut queue) = registry_queue_init::<Ignore>(&conn).expect("no registry");
        let qh = queue.handle();
        let seat: WlSeat = globals.bind(&qh, 1..=1, ()).expect("no seat");
        let manager: ZwpVirtualKeyboardManagerV1 = globals.bind(&qh, 1..=1, ()).expect("no virtual keyboard");
        let keyboard = manager.create_virtual_keyboard(&seat, &qh, ());
        // SAFETY: plain system call; the new descriptor is owned here.
        let fd = unsafe { OwnedFd::from_raw_fd(libc::memfd_create(c"keymap".as_ptr(), 0)) };
        let mut file = std::fs::File::from(fd);
        file.write_all(KEYMAP.as_bytes()).and_then(|()| file.write_all(&[0])).expect("cannot write the keymap");
        keyboard.keymap(1, file.as_fd(), KEYMAP.len() as u32 + 1);
        loop {
            if queue.blocking_dispatch(&mut Ignore).is_err() {
                return;
            }
        }
    }
}

/// A Mutter remote desktop session, for tests under a headless Mutter.
#[cfg(target_os = "linux")]
mod mutter {
    use sayso_platform_linux::inserter::keys;
    use std::sync::OnceLock;
    use zbus::blocking::Connection;
    use zbus::zvariant::OwnedObjectPath;

    const DEST: &str = "org.gnome.Mutter.RemoteDesktop";
    static SESSION: OnceLock<(Connection, OwnedObjectPath)> = OnceLock::new();

    pub fn start() -> zbus::Result<String> {
        let conn = Connection::session()?;
        let reply = conn.call_method(Some(DEST), "/org/gnome/Mutter/RemoteDesktop", Some(DEST), "CreateSession", &())?;
        let path: OwnedObjectPath = reply.body().deserialize()?;
        conn.call_method(Some(DEST), path.as_str(), Some("org.gnome.Mutter.RemoteDesktop.Session"), "Start", &())?;
        let name = path.to_string();
        let _ = SESSION.set((conn, path));
        Ok(name)
    }

    fn key(sym: u32, down: bool) -> zbus::Result<()> {
        let (conn, path) = SESSION.get().ok_or_else(|| zbus::Error::Failure("no session".into()))?;
        let iface = Some("org.gnome.Mutter.RemoteDesktop.Session");
        conn.call_method(Some(DEST), path.as_str(), iface, "NotifyKeyboardKeysym", &(sym, down))?;
        Ok(())
    }

    pub fn keys(text: &str) -> zbus::Result<()> {
        if let Some(paste) = keys::PasteKey::parse(text) {
            let mut presses: Vec<(u32, bool)> = paste.modifiers().iter().map(|m| (m.keysym, true)).collect();
            presses.extend([(paste.key().keysym, true), (paste.key().keysym, false)]);
            presses.extend(paste.modifiers().iter().rev().map(|m| (m.keysym, false)));
            return presses.into_iter().try_for_each(|(sym, down)| key(sym, down));
        }
        for sym in keys::text_keysyms(text).map_err(|c| zbus::Error::Failure(format!("{c:?}")))? {
            key(sym, true)?;
            key(sym, false)?;
        }
        Ok(())
    }
}
