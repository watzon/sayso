//! Tests against a real X server and real input devices. They do not run by
//! default.
//!
//! ```sh
//! # X11: XGrabKey chords and XInput2 raw keys, driven by xdotool.
//! xvfb-run -a cargo test -p sayso-platform-linux live_x11 -- --ignored --nocapture
//! # evdev: a uinput virtual keyboard. Needs write access to /dev/uinput and
//! # read access to /dev/input (run the test binary with sudo).
//! cargo test -p sayso-platform-linux live_evdev -- --ignored --nocapture
//! ```

use super::*;
use crate::session::{Desktop, UiBackend};
use std::collections::BTreeSet;
use std::process::Command;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(3);

fn session(kind: SessionKind) -> &'static Session {
    Box::leak(Box::new(Session {
        kind,
        ui: UiBackend::Headless,
        desktop: Desktop::Other(String::new()),
        wayland_display: None,
        x11_display: std::env::var_os("DISPLAY"),
        wayland_globals: BTreeSet::new(),
    }))
}

fn bindings(toggle: &str, ptt: &str) -> HotkeyBindings {
    HotkeyBindings {
        toggle: Some(toggle.parse().unwrap()),
        push_to_talk: Some(ptt.parse().unwrap()),
        paste_last: None,
        cycle_style: None,
        single_escape: false,
        double_escape_window: Duration::from_millis(400),
    }
}

fn expect(rx: &Receiver<HotkeyEvent>, want: HotkeyEvent) {
    match rx.recv_timeout(WAIT) {
        Ok(got) => assert_eq!(got, want),
        Err(e) => panic!("no {want:?}: {e}"),
    }
    println!("got {want:?}");
}

fn expect_quiet(rx: &Receiver<HotkeyEvent>) {
    if let Ok(event) = rx.recv_timeout(Duration::from_millis(300)) {
        panic!("unexpected {event:?}");
    }
}

fn xdotool(args: &[&str]) {
    let status = Command::new("xdotool").args(args).status().expect("xdotool runs");
    assert!(status.success(), "xdotool {args:?}");
}

#[test]
#[ignore = "needs an X server (xvfb-run) and xdotool"]
fn live_x11() {
    let _ = env_logger::builder().is_test(true).try_init();
    let hotkeys = LinuxHotkeys::new(session(SessionKind::X11));
    let rx = hotkeys.events();
    let reg = hotkeys.register(&bindings("ctrl+opt+space", "right_control"));
    println!("registration: {reg:?}");
    assert!(reg.failed.is_empty() && !reg.needs_input_monitoring);

    // A second client cannot grab the same chord.
    let other = LinuxHotkeys::new(session(SessionKind::X11));
    let reg2 = other.register(&bindings("ctrl+opt+space", "right_shift"));
    println!("second client: {reg2:?}");
    assert_eq!(reg2.failed.len(), 1);
    assert!(reg2.failed[0].1.contains("another app"));
    drop(other);

    // The toggle chord through XGrabKey, also with CapsLock on.
    xdotool(&["key", "ctrl+alt+space"]);
    expect(&rx, HotkeyEvent::Toggle);
    xdotool(&["key", "Caps_Lock"]);
    xdotool(&["key", "ctrl+alt+space"]);
    expect(&rx, HotkeyEvent::Toggle);
    xdotool(&["key", "Caps_Lock"]);
    // A held chord fires once, not once per auto-repeat.
    xdotool(&["keydown", "ctrl+alt+space"]);
    std::thread::sleep(Duration::from_millis(900));
    xdotool(&["keyup", "ctrl+alt+space"]);
    expect(&rx, HotkeyEvent::Toggle);
    expect_quiet(&rx);

    // Solo push-to-talk through XInput2 raw keys. xdotool presses Left Ctrl
    // too when it gets the name Control_R, so it gets the X key code.
    let control_r = (keymap::KEY_RIGHTCTRL + keymap::X11_OFFSET).to_string();
    xdotool(&["keydown", &control_r]);
    expect(&rx, HotkeyEvent::PushToTalkDown);
    xdotool(&["keyup", &control_r]);
    expect(&rx, HotkeyEvent::PushToTalkUp);

    // Esc counts only while recording.
    xdotool(&["key", "--delay", "100", "Escape", "Escape"]);
    expect_quiet(&rx);
    hotkeys.set_recording(true);
    xdotool(&["key", "--delay", "100", "Escape", "Escape"]);
    expect(&rx, HotkeyEvent::Cancel);
    hotkeys.set_recording(false);

    // The key recorder takes the next chord, and the grab does not also fire.
    let captured = hotkeys.capture_next();
    xdotool(&["key", "ctrl+alt+space"]);
    let hotkey = captured.recv_timeout(WAIT).expect("a captured hotkey");
    println!("captured {hotkey}");
    assert_eq!(hotkey, "ctrl+opt+space".parse().unwrap());
    expect_quiet(&rx);
    let captured = hotkeys.capture_next();
    let shift_r = (keymap::KEY_RIGHTSHIFT + keymap::X11_OFFSET).to_string();
    xdotool(&["keydown", &shift_r]);
    xdotool(&["keyup", &shift_r]);
    let hotkey = captured.recv_timeout(WAIT).expect("a captured solo modifier");
    println!("captured {hotkey}");
    assert_eq!(hotkey, "right_shift".parse().unwrap());

    // Chord push-to-talk through the grab. Wait until the mute after the
    // capture ends.
    std::thread::sleep(Duration::from_millis(600));
    let reg = hotkeys.register(&bindings("ctrl+opt+space", "ctrl+shift+d"));
    assert!(reg.failed.is_empty());
    xdotool(&["keydown", "ctrl+shift+d"]);
    expect(&rx, HotkeyEvent::PushToTalkDown);
    xdotool(&["keyup", "d"]);
    expect(&rx, HotkeyEvent::PushToTalkUp);
    xdotool(&["keyup", "ctrl+shift"]);
    expect_quiet(&rx);
    drop(hotkeys);
    println!("dropped cleanly");
}

#[test]
#[ignore = "needs /dev/uinput and read access to /dev/input (sudo)"]
fn live_evdev() {
    use ::evdev::uinput::VirtualDevice;
    use ::evdev::{AttributeSet, EventType, InputEvent, KeyCode};

    let _ = env_logger::builder().is_test(true).try_init();
    let mut keys = AttributeSet::<KeyCode>::new();
    for code in 1..=127u16 {
        keys.insert(KeyCode::new(code));
    }
    let new_keyboard = |name: &str| {
        VirtualDevice::builder()
            .expect("/dev/uinput opens")
            .name(name)
            .with_keys(&keys)
            .unwrap()
            .build()
            .expect("the virtual keyboard is created")
    };
    let send = |keyboard: &mut VirtualDevice, code: u16, value: i32| {
        keyboard
            .emit(&[InputEvent::new(EventType::KEY.0, code, value)])
            .expect("the key event is sent");
        std::thread::sleep(Duration::from_millis(30));
    };
    let mut keyboard = new_keyboard("Sayso test keyboard");
    let mut press = |code: u16, value: i32| send(&mut keyboard, code, value);
    // Give udev time to create the device node.
    std::thread::sleep(Duration::from_millis(500));

    // No display: the evdev stream also detects the chords.
    let hotkeys = LinuxHotkeys::new(session(SessionKind::None));
    let rx = hotkeys.events();
    let reg = hotkeys.register(&bindings("ctrl+opt+space", "right_control"));
    println!("registration: {reg:?}");
    assert!(reg.failed.is_empty() && !reg.needs_input_monitoring);
    assert_eq!(input_monitoring_state(session(SessionKind::None)), PermissionState::Granted);

    use keymap::*;
    press(KEY_LEFTCTRL, 1);
    press(KEY_LEFTALT, 1);
    press(KEY_SPACE, 1);
    press(KEY_SPACE, 2);
    press(KEY_SPACE, 0);
    press(KEY_LEFTALT, 0);
    press(KEY_LEFTCTRL, 0);
    expect(&rx, HotkeyEvent::Toggle);
    expect_quiet(&rx);

    press(KEY_RIGHTCTRL, 1);
    expect(&rx, HotkeyEvent::PushToTalkDown);
    press(KEY_RIGHTCTRL, 2);
    press(KEY_RIGHTCTRL, 0);
    expect(&rx, HotkeyEvent::PushToTalkUp);

    hotkeys.set_recording(true);
    for value in [1, 0, 1, 0] {
        press(KEY_ESC, value);
    }
    expect(&rx, HotkeyEvent::Cancel);
    hotkeys.set_recording(false);

    let captured = hotkeys.capture_next();
    press(KEY_LEFTMETA, 1);
    press(code(sayso_core::hotkey::Key::Letter('k')).unwrap(), 1);
    let hotkey = captured.recv_timeout(WAIT).expect("a captured hotkey");
    println!("captured {hotkey}");
    assert_eq!(hotkey, "cmd+k".parse().unwrap());
    press(code(sayso_core::hotkey::Key::Letter('k')).unwrap(), 0);
    press(KEY_LEFTMETA, 0);

    // A keyboard plugged in later is picked up.
    let mut second = new_keyboard("Sayso second test keyboard");
    std::thread::sleep(Duration::from_millis(500));
    send(&mut second, KEY_RIGHTCTRL, 1);
    expect(&rx, HotkeyEvent::PushToTalkDown);
    send(&mut second, KEY_RIGHTCTRL, 0);
    expect(&rx, HotkeyEvent::PushToTalkUp);
    drop(hotkeys);
    println!("dropped cleanly");
}
