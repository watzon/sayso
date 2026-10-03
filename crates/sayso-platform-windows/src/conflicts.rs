//! Hotkey conflict detection.
//!
//! Three sources:
//! 1. A table of Windows shortcuts. Windows has no list of its own shortcuts
//!    to read, and most Win+letter chords are taken by Windows, so a chord
//!    with only Win and a letter always counts.
//! 2. A table of apps that use a hotkey by default, checked against the
//!    running processes. Their settings are not read, so these are hints.
//! 3. A real probe: `RegisterHotKey` for the chord on a scratch thread,
//!    released at once. "Already registered" means another program holds it.
//!    Programs that watch keys with a hook (PowerToys does) do not show up
//!    here, which is why the app table stays.

use crate::context;
use crate::keymap;
use sayso_core::hotkey::{Hotkey, Key, Modifiers};
use sayso_platform::{ConflictSource, HotkeyConflict};
use windows::Win32::Foundation::ERROR_HOTKEY_ALREADY_REGISTERED;
use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, RegisterHotKey, UnregisterHotKey};

/// Windows shortcuts by hotkey. The names follow the Windows keyboard shortcut list.
pub const SYSTEM_SHORTCUTS: &[(&str, &str)] = &[
    ("opt+space", "Window menu"),
    ("opt+tab", "Switch apps"),
    ("opt+shift+tab", "Switch apps"),
    ("opt+f4", "Close window"),
    ("opt+esc", "Cycle through windows"),
    ("ctrl+esc", "Start menu"),
    ("ctrl+shift+esc", "Task Manager"),
    ("cmd+space", "Switch input language"),
    ("shift+cmd+space", "Switch input language"),
    ("ctrl+cmd+space", "Previous input language"),
    ("cmd+tab", "Task view"),
    ("cmd+esc", "Close Magnifier"),
    ("ctrl+cmd+v", "Sound output"),
    ("ctrl+cmd+c", "Color filters"),
    ("ctrl+cmd+d", "New virtual desktop"),
    ("ctrl+cmd+f4", "Close virtual desktop"),
    ("ctrl+cmd+n", "Narrator settings"),
    ("ctrl+cmd+o", "On-Screen Keyboard"),
    ("ctrl+cmd+q", "Quick Assist"),
    ("ctrl+cmd+return", "Narrator"),
    ("ctrl+shift+cmd+b", "Restart the graphics driver"),
    ("shift+cmd+m", "Restore minimized windows"),
    ("shift+cmd+r", "Snipping Tool screen recording"),
    ("shift+cmd+s", "Snipping Tool"),
    ("opt+cmd+b", "HDR on or off"),
    ("opt+cmd+d", "Date and time"),
    ("opt+cmd+k", "Mute the microphone in a call"),
    ("opt+cmd+r", "Game Bar recording"),
    ("f12", "Reserved for debuggers"),
];

/// Names for Win+letter, `a` to `z`. Windows reserves every one of them.
const WIN_LETTERS: [&str; 26] = [
    "Quick Settings",
    "Notification area",
    "Copilot",
    "Show the desktop",
    "File Explorer",
    "Feedback Hub",
    "Game Bar",
    "Voice typing",
    "Settings",
    "A Windows shortcut",
    "Cast",
    "Lock the PC",
    "Minimize all windows",
    "Notification center",
    "Lock the screen rotation",
    "Project to a screen",
    "Search",
    "Run",
    "Search",
    "Taskbar apps",
    "Accessibility settings",
    "Clipboard history",
    "Widgets",
    "Quick Link menu",
    "A Windows shortcut",
    "Snap layouts",
];

/// An app that is known to use a hotkey by default.
#[derive(Debug, Clone, Copy)]
pub struct KnownApp {
    /// Every exe name the app runs under, lowercase.
    pub exe_names: &'static [&'static str],
    pub name: &'static str,
    /// The default hotkey.
    pub hotkey: &'static str,
}

pub const KNOWN_APPS: &[KnownApp] = &[
    KnownApp { exe_names: &["powertoys.powerlauncher.exe"], name: "PowerToys Run", hotkey: "opt+space" },
    KnownApp { exe_names: &["microsoft.cmdpal.ui.exe"], name: "Command Palette", hotkey: "opt+cmd+space" },
    KnownApp { exe_names: &["copilot.exe", "microsoft.copilot.exe"], name: "Microsoft Copilot", hotkey: "opt+space" },
    KnownApp { exe_names: &["chatgpt.exe"], name: "ChatGPT", hotkey: "opt+space" },
    KnownApp { exe_names: &["raycast.exe"], name: "Raycast", hotkey: "opt+space" },
    KnownApp { exe_names: &["flow.launcher.exe"], name: "Flow Launcher", hotkey: "opt+space" },
    KnownApp { exe_names: &["wox.exe"], name: "Wox", hotkey: "opt+space" },
];

/// Windows shortcuts that use exactly this chord.
pub fn match_system(hotkey: &Hotkey) -> Vec<ConflictSource> {
    let Hotkey::Chord { modifiers, key } = hotkey else { return Vec::new() };
    let system = |name: &str| ConflictSource::System { name: name.to_string(), enabled: true };
    let mut out: Vec<ConflictSource> = SYSTEM_SHORTCUTS
        .iter()
        .filter(|(text, _)| text.parse::<Hotkey>().ok().as_ref() == Some(hotkey))
        .map(|(_, name)| system(name))
        .collect();
    let win_only = Modifiers { command: true, ..Default::default() };
    match key {
        Key::Letter(c) if *modifiers == win_only && c.is_ascii_alphabetic() => {
            out.push(system(WIN_LETTERS[(c.to_ascii_lowercase() as u8 - b'a') as usize]));
        }
        // Win+1 to Win+0 open the taskbar apps, with Shift, Ctrl, or Alt as variants.
        Key::Digit(_) if modifiers.command && !modifiers.function => {
            let others = [modifiers.shift, modifiers.control, modifiers.option].iter().filter(|m| **m).count();
            if others <= 1 {
                out.push(system("Open a taskbar app"));
            }
        }
        _ => {}
    }
    out.dedup();
    out
}

/// Known apps that are running now and use `hotkey` by default.
/// `running` are lowercase exe names.
pub fn match_known_apps(hotkey: &Hotkey, running: &[String]) -> Vec<ConflictSource> {
    KNOWN_APPS
        .iter()
        .filter(|app| app.hotkey.parse::<Hotkey>().ok().as_ref() == Some(hotkey))
        .filter_map(|app| {
            let exe = app.exe_names.iter().find(|exe| running.iter().any(|r| r == *exe))?;
            Some(ConflictSource::App { name: app.name.to_string(), bundle_id: exe.to_string(), confirmed: false })
        })
        .collect()
}

/// What `RegisterHotKey` said about a chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// Nobody holds it through `RegisterHotKey`.
    Free,
    /// Another program holds it.
    Taken,
    /// Not probed: Sayso holds it, it is not a chord, or the call failed otherwise.
    Unknown,
}

/// Try to register the chord on a scratch thread and release it at once.
pub fn probe(hotkey: &Hotkey) -> Probe {
    let Hotkey::Chord { modifiers, key } = *hotkey else { return Probe::Unknown };
    let Some(vk) = keymap::vk(key) else { return Probe::Unknown };
    if modifiers.function {
        return Probe::Unknown;
    }
    let mods = HOT_KEY_MODIFIERS(keymap::hotkey_mods(modifiers) | keymap::MOD_NOREPEAT);
    let hotkey = *hotkey;
    // A thread of its own: a hotkey with no window belongs to the calling
    // thread, and the GPUI thread must not get a stray WM_HOTKEY.
    let probe = std::thread::spawn(move || {
        const ID: i32 = 0xBEEF;
        // SAFETY: registers and unregisters a thread hotkey; no pointers.
        match unsafe { RegisterHotKey(None, ID, mods, u32::from(vk)) } {
            Ok(()) => {
                // SAFETY: as above.
                let _ = unsafe { UnregisterHotKey(None, ID) };
                Probe::Free
            }
            Err(err) if err.code() == ERROR_HOTKEY_ALREADY_REGISTERED.to_hresult() => Probe::Taken,
            Err(err) => {
                log::debug!("hotkey probe for {hotkey}: {err}");
                Probe::Unknown
            }
        }
    });
    probe.join().unwrap_or(Probe::Unknown)
}

/// Put the three sources together.
///
/// A taken chord that a Windows shortcut explains adds nothing. When exactly
/// one known app is running with it, that app is the holder, now confirmed.
/// Otherwise the holder is "Another app".
pub fn combine(system: Vec<ConflictSource>, mut apps: Vec<ConflictSource>, probe: Probe) -> Vec<ConflictSource> {
    if probe == Probe::Taken && system.is_empty() {
        if let [ConflictSource::App { confirmed, .. }] = apps.as_mut_slice() {
            *confirmed = true;
        } else {
            apps.push(ConflictSource::App { name: "Another app".into(), bundle_id: String::new(), confirmed: true });
        }
    }
    system.into_iter().chain(apps).collect()
}

/// All conflicts for `hotkey`: Windows shortcuts first, then apps.
/// `held` are the chords Sayso has registered itself; they are not probed,
/// because the probe would find Sayso.
pub fn conflicts(hotkey: &Hotkey, held: &[Hotkey]) -> Vec<HotkeyConflict> {
    let system = match_system(hotkey);
    let apps = match_known_apps(hotkey, &context::process_exe_names());
    let probed = if held.contains(hotkey) { Probe::Unknown } else { probe(hotkey) };
    combine(system, apps, probed).into_iter().map(|source| HotkeyConflict { hotkey: *hotkey, source }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hk(s: &str) -> Hotkey {
        s.parse().unwrap()
    }

    fn system(name: &str) -> ConflictSource {
        ConflictSource::System { name: name.into(), enabled: true }
    }

    fn app(name: &str, exe: &str, confirmed: bool) -> ConflictSource {
        ConflictSource::App { name: name.into(), bundle_id: exe.into(), confirmed }
    }

    #[test]
    fn every_table_hotkey_parses() {
        for (text, name) in SYSTEM_SHORTCUTS {
            assert!(text.parse::<Hotkey>().is_ok(), "{name}: {text}");
        }
        for known in KNOWN_APPS {
            assert!(known.hotkey.parse::<Hotkey>().is_ok(), "{}", known.name);
            assert!(known.exe_names.iter().all(|e| e.ends_with(".exe") && *e == e.to_lowercase()), "{}", known.name);
        }
    }

    #[test]
    fn alt_space_is_the_window_menu() {
        assert_eq!(match_system(&Hotkey::toggle_default()), vec![system("Window menu")]);
    }

    #[test]
    fn well_known_shortcuts_are_found() {
        assert_eq!(match_system(&hk("opt+tab")), vec![system("Switch apps")]);
        assert_eq!(match_system(&hk("opt+f4")), vec![system("Close window")]);
        assert_eq!(match_system(&hk("ctrl+shift+esc")), vec![system("Task Manager")]);
        assert_eq!(match_system(&hk("ctrl+cmd+v")), vec![system("Sound output")]);
        assert_eq!(match_system(&hk("cmd+space")), vec![system("Switch input language")]);
        assert_eq!(match_system(&hk("shift+cmd+s")), vec![system("Snipping Tool")]);
    }

    #[test]
    fn every_win_letter_is_reserved() {
        assert_eq!(match_system(&hk("cmd+v")), vec![system("Clipboard history")]);
        assert_eq!(match_system(&hk("cmd+h")), vec![system("Voice typing")]);
        assert_eq!(match_system(&hk("cmd+l")), vec![system("Lock the PC")]);
        for c in 'a'..='z' {
            assert_eq!(match_system(&hk(&format!("cmd+{c}"))).len(), 1, "win+{c}");
        }
        // With more modifiers, only the table counts.
        assert!(match_system(&hk("ctrl+shift+cmd+k")).is_empty());
    }

    #[test]
    fn win_digits_are_taskbar_apps() {
        assert_eq!(match_system(&hk("cmd+1")), vec![system("Open a taskbar app")]);
        assert_eq!(match_system(&hk("shift+cmd+3")), vec![system("Open a taskbar app")]);
        assert!(match_system(&hk("ctrl+shift+cmd+3")).is_empty());
        assert!(match_system(&hk("ctrl+3")).is_empty());
    }

    #[test]
    fn free_chords_and_solo_modifiers_have_no_system_conflict() {
        assert!(match_system(&hk("ctrl+opt+d")).is_empty());
        assert!(match_system(&hk("opt+shift+v")).is_empty(), "the paste-last default is free");
        assert!(match_system(&hk("right_option")).is_empty());
        assert!(match_system(&hk("f11")).is_empty());
    }

    #[test]
    fn known_apps_match_only_when_running() {
        let toggle = Hotkey::toggle_default();
        assert!(match_known_apps(&toggle, &["explorer.exe".into()]).is_empty());
        let running = ["explorer.exe".to_string(), "powertoys.powerlauncher.exe".into(), "chatgpt.exe".into()];
        assert_eq!(
            match_known_apps(&toggle, &running),
            vec![app("PowerToys Run", "powertoys.powerlauncher.exe", false), app("ChatGPT", "chatgpt.exe", false)]
        );
    }

    #[test]
    fn known_apps_only_match_their_own_hotkey() {
        let running = ["microsoft.cmdpal.ui.exe".to_string(), "raycast.exe".into()];
        assert_eq!(match_known_apps(&hk("opt+cmd+space"), &running), vec![app("Command Palette", "microsoft.cmdpal.ui.exe", false)]);
        assert!(match_known_apps(&hk("ctrl+opt+space"), &running).is_empty());
        assert!(match_known_apps(&hk("right_option"), &running).is_empty());
    }

    #[test]
    fn copilot_is_found_under_either_exe_name() {
        let hits = match_known_apps(&Hotkey::toggle_default(), &["microsoft.copilot.exe".into()]);
        assert_eq!(hits, vec![app("Microsoft Copilot", "microsoft.copilot.exe", false)]);
    }

    #[test]
    fn a_taken_chord_with_no_explanation_is_another_app() {
        assert_eq!(combine(vec![], vec![], Probe::Taken), vec![app("Another app", "", true)]);
        assert!(combine(vec![], vec![], Probe::Free).is_empty());
        assert!(combine(vec![], vec![], Probe::Unknown).is_empty());
    }

    #[test]
    fn a_taken_chord_confirms_the_one_running_app() {
        let apps = vec![app("Raycast", "raycast.exe", false)];
        assert_eq!(combine(vec![], apps.clone(), Probe::Taken), vec![app("Raycast", "raycast.exe", true)]);
        assert_eq!(combine(vec![], apps.clone(), Probe::Free), apps, "a hook-based app stays a hint");
        let two = vec![app("Raycast", "raycast.exe", false), app("Wox", "wox.exe", false)];
        let combined = combine(vec![], two.clone(), Probe::Taken);
        assert_eq!(combined[..2], two[..]);
        assert_eq!(combined[2], app("Another app", "", true));
    }

    #[test]
    fn a_windows_shortcut_explains_a_taken_chord() {
        let combined = combine(vec![system("Clipboard history")], vec![], Probe::Taken);
        assert_eq!(combined, vec![system("Clipboard history")]);
    }

    #[test]
    fn chords_sayso_holds_are_not_probed() {
        // The probe would find Sayso itself; `held` skips it. A solo key is never probed.
        assert_eq!(probe(&hk("right_option")), Probe::Unknown);
        assert_eq!(probe(&hk("fn+space")), Probe::Unknown);
    }

    #[test]
    fn the_probe_sees_a_registered_chord() {
        // Hold an unusual chord on another thread, then probe it.
        let chord = hk("ctrl+opt+shift+f19");
        let (held_tx, held_rx) = crossbeam_channel::bounded(1);
        let (done_tx, done_rx) = crossbeam_channel::bounded::<()>(1);
        let holder = std::thread::spawn(move || {
            let mods = HOT_KEY_MODIFIERS(keymap::MOD_CONTROL | keymap::MOD_ALT | keymap::MOD_SHIFT);
            // SAFETY: a thread hotkey without pointers, released below.
            let ok = unsafe { RegisterHotKey(None, 7, mods, 0x82) }.is_ok();
            held_tx.send(ok).unwrap();
            let _ = done_rx.recv();
            // SAFETY: as above.
            let _ = unsafe { UnregisterHotKey(None, 7) };
        });
        assert!(held_rx.recv().unwrap(), "F19 with three modifiers is free on a normal machine");
        assert_eq!(probe(&chord), Probe::Taken);
        done_tx.send(()).unwrap();
        holder.join().unwrap();
        assert_eq!(probe(&chord), Probe::Free);
    }
}
