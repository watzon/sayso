//! Hotkey conflict detection.
//!
//! Two sources, as found in the hotkeys spike:
//! 1. macOS system shortcuts, from `CopySymbolicHotKeys`. It lists OS
//!    defaults too (the plist does not), but gives no names, so we label
//!    the well-known ones by key and modifiers. Modifiers use the Carbon mask.
//! 2. A small table of apps that grab a default hotkey, checked against the
//!    running apps. Carbon cannot see other apps' hotkeys, so this is a static hint.

use crate::context;
use crate::ffi::{CopySymbolicHotKeys, CFTypeRef, noErr};
use crate::keymap::{self, CARBON_ALL, CARBON_CMD, CARBON_CONTROL, CARBON_OPTION, CARBON_SHIFT};
use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use sayso_core::hotkey::Hotkey;
use sayso_platform::{AppInfo, ConflictSource, HotkeyConflict};
use std::ptr;

/// One row of `CopySymbolicHotKeys`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolicHotkey {
    pub keycode: i64,
    /// Carbon modifier mask.
    pub carbon_mods: u32,
    pub enabled: bool,
}

/// An app that is known to use a hotkey by default.
#[derive(Debug, Clone, Copy)]
pub struct KnownApp {
    /// Every bundle id the app ships under.
    pub bundle_ids: &'static [&'static str],
    pub name: &'static str,
    /// The default hotkey.
    pub hotkey: &'static str,
    /// Where the app stores its current hotkey, when Sayso can read it.
    pub setting: Option<AppSetting>,
}

/// A hotkey stored in an app's preferences.
#[derive(Debug, Clone, Copy)]
pub enum AppSetting {
    /// A `defaults` value like `"Option-49"`: modifier names and a key code, joined by "-".
    DashedKeycode { domain: &'static str, key: &'static str },
    /// The ChatGPT app's `keybindings.json` in `$CODEX_HOME` (default `~/.codex`).
    CodexKeymap,
}

pub const KNOWN_APPS: &[KnownApp] = &[
    KnownApp { bundle_ids: &["com.openai.codex"], name: "ChatGPT", hotkey: "opt+space", setting: Some(AppSetting::CodexKeymap) },
    // The older ChatGPT app keeps its shortcut in its sandbox container. Reading
    // it makes macOS ask for access to another app's data, so only the default counts.
    KnownApp { bundle_ids: &["com.openai.chat"], name: "ChatGPT", hotkey: "opt+space", setting: None },
    KnownApp {
        bundle_ids: &["com.raycast.macos"],
        name: "Raycast",
        hotkey: "opt+space",
        setting: Some(AppSetting::DashedKeycode { domain: "com.raycast.macos", key: "raycastGlobalHotkey" }),
    },
    KnownApp { bundle_ids: &["com.runningwithcrayons.Alfred"], name: "Alfred", hotkey: "opt+space", setting: None },
];

/// What an app's setting says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingValue {
    /// The setting was read: the app uses these hotkeys. Empty means none.
    Known(Vec<Hotkey>),
    /// No setting, or it could not be read. Assume the default.
    Unknown,
}

/// Parse `"Option-49"`, `"Command-Shift-2"`. An empty string means no hotkey.
pub fn parse_dashed_keycode(value: &str) -> SettingValue {
    let value = value.trim();
    if value.is_empty() {
        return SettingValue::Known(Vec::new());
    }
    let parts: Vec<&str> = value.split('-').collect();
    let Some((code, mods)) = parts.split_last() else { return SettingValue::Unknown };
    let Ok(code) = code.parse::<u16>() else { return SettingValue::Unknown };
    let Some(key) = keymap::key_from_keycode(code) else { return SettingValue::Unknown };
    let mut modifiers = sayso_core::hotkey::Modifiers::default();
    for m in mods {
        match m.to_ascii_lowercase().as_str() {
            "command" | "cmd" => modifiers.command = true,
            "option" | "alt" => modifiers.option = true,
            "control" | "ctrl" => modifiers.control = true,
            "shift" => modifiers.shift = true,
            "function" | "fn" => modifiers.function = true,
            _ => return SettingValue::Unknown,
        }
    }
    SettingValue::Known(vec![Hotkey::Chord { modifiers, key }])
}

/// The ChatGPT app's commands with a system-wide shortcut, and the macOS
/// default of each (from the app's command list). Only "Show Mini" has one.
const CODEX_GLOBAL_COMMANDS: &[(&str, Option<&str>)] = &[
    ("openAvatarOverlay", Some("Alt+Space")),
    ("hotkeyWindow", None),
    ("globalDictationHold", None),
    ("globalDictationSingleTap", None),
    ("realtimeVoice", None),
];

/// An Electron accelerator such as `"Alt+Space"` or `"CommandOrControl+Shift+K"`.
/// None for keys Sayso has no name for.
pub fn parse_accelerator(accelerator: &str) -> Option<Hotkey> {
    let tokens: Vec<String> = accelerator
        .split('+')
        .map(|t| match t.trim().to_ascii_lowercase().as_str() {
            // On macOS these all mean Command.
            "commandorcontrol" | "cmdorctrl" | "super" | "meta" => "cmd".to_string(),
            "return" => "enter".to_string(),
            other => other.to_string(),
        })
        .collect();
    tokens.join("+").parse().ok()
}

/// The hotkeys in a ChatGPT `keybindings.json`: a list of `{command, key}`.
/// An entry for a command replaces its default; `"key": null` turns it off.
/// A missing file (None) means every command has its default.
pub fn parse_codex_keymap(text: Option<&str>) -> SettingValue {
    let entries: Vec<(String, Option<String>)> = match text {
        None => Vec::new(),
        Some(text) => {
            let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(text) else {
                return SettingValue::Unknown;
            };
            items
                .iter()
                .filter_map(|i| Some((i.get("command")?.as_str()?.to_string(), i.get("key").and_then(|k| k.as_str()).map(str::to_string))))
                .collect()
        }
    };
    let mut hotkeys = Vec::new();
    for (command, default) in CODEX_GLOBAL_COMMANDS {
        let set: Vec<&Option<String>> = entries.iter().filter(|(c, _)| c == command).map(|(_, k)| k).collect();
        let keys: Vec<&str> = if set.is_empty() { default.iter().copied().collect() } else { set.iter().filter_map(|k| k.as_deref()).collect() };
        hotkeys.extend(keys.into_iter().filter_map(parse_accelerator));
    }
    SettingValue::Known(hotkeys)
}

fn codex_keymap_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".codex")))?;
    Some(home.join("keybindings.json"))
}

/// Read an app's hotkey setting from its preferences.
pub fn read_setting(setting: AppSetting) -> SettingValue {
    match setting {
        AppSetting::DashedKeycode { domain, key } => {
            let out = std::process::Command::new("/usr/bin/defaults").args(["read", domain, key]).output();
            match out {
                Ok(o) if o.status.success() => parse_dashed_keycode(&String::from_utf8_lossy(&o.stdout)),
                _ => SettingValue::Unknown,
            }
        }
        AppSetting::CodexKeymap => {
            let Some(path) = codex_keymap_path() else { return SettingValue::Unknown };
            match std::fs::read_to_string(&path) {
                Ok(text) => parse_codex_keymap(Some(&text)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => parse_codex_keymap(None),
                Err(_) => SettingValue::Unknown,
            }
        }
    }
}

/// Names for system shortcuts we can recognize by key code and Carbon modifiers.
/// The ids in the comments are the `com.apple.symbolichotkeys` ids.
const SYSTEM_LABELS: &[(u16, u32, &str)] = &[
    (49, CARBON_CMD, "Spotlight search"),                          // id 64
    (49, CARBON_CMD | CARBON_OPTION, "Finder search window"),      // id 65
    (49, CARBON_CONTROL, "Select previous input source"),          // id 60
    (49, CARBON_CONTROL | CARBON_OPTION, "Select next input source"), // id 61
    (49, CARBON_CONTROL | CARBON_CMD, "Emoji & Symbols"),
    (126, CARBON_CONTROL, "Mission Control"),
    (125, CARBON_CONTROL, "Application windows"),
    (123, CARBON_CONTROL, "Move left a space"),
    (124, CARBON_CONTROL, "Move right a space"),
    (20, CARBON_CMD | CARBON_SHIFT, "Screenshot (whole screen)"),
    (21, CARBON_CMD | CARBON_SHIFT, "Screenshot (selection)"),
    (23, CARBON_CMD | CARBON_SHIFT, "Screenshot and recording options"),
    (50, CARBON_CMD, "Move focus to next window of the app"),
    (2, CARBON_CMD | CARBON_OPTION, "Dock: turn hiding on or off"),
];

fn system_label(keycode: i64, carbon_mods: u32) -> String {
    SYSTEM_LABELS
        .iter()
        .find(|(k, m, _)| i64::from(*k) == keycode && *m == carbon_mods)
        .map(|(_, _, name)| (*name).to_string())
        .unwrap_or_else(|| "A macOS keyboard shortcut".to_string())
}

/// System shortcuts that use the same key and modifiers as `hotkey`.
/// Solo modifiers and unsupported keys never match.
pub fn match_system(list: &[SymbolicHotkey], hotkey: &Hotkey) -> Vec<ConflictSource> {
    let Hotkey::Chord { modifiers, key } = hotkey else { return Vec::new() };
    let Some(code) = keymap::keycode(*key) else { return Vec::new() };
    let want = keymap::carbon_mask(*modifiers);
    let mut out: Vec<ConflictSource> = Vec::new();
    for entry in list.iter().filter(|e| e.keycode == i64::from(code) && e.carbon_mods & CARBON_ALL == want) {
        let source = ConflictSource::System { name: system_label(entry.keycode, want), enabled: entry.enabled };
        if !out.contains(&source) {
            out.push(source);
        }
    }
    out
}

/// Known apps that are running now and use `hotkey`.
///
/// When the app's setting can be read, only a real match counts. Otherwise the
/// app's default counts, and the conflict is marked as not confirmed.
pub fn match_known_apps(
    hotkey: &Hotkey,
    running: &[AppInfo],
    read: impl Fn(AppSetting) -> SettingValue,
) -> Vec<ConflictSource> {
    let mut out = Vec::new();
    for app in KNOWN_APPS {
        let Some(bundle_id) = app.bundle_ids.iter().find(|id| running.iter().any(|r| r.bundle_id == **id)) else {
            continue;
        };
        let value = app.setting.map(&read).unwrap_or(SettingValue::Unknown);
        let (uses, confirmed) = match value {
            SettingValue::Known(current) => (current.contains(hotkey), true),
            SettingValue::Unknown => (app.hotkey.parse::<Hotkey>().ok().as_ref() == Some(hotkey), false),
        };
        if uses {
            out.push(ConflictSource::App { name: app.name.to_string(), bundle_id: bundle_id.to_string(), confirmed });
        }
    }
    out
}

/// Live system shortcut list. Empty when the call fails.
pub fn system_hotkeys() -> Vec<SymbolicHotkey> {
    let mut raw: CFTypeRef = ptr::null();
    // SAFETY: out-pointer to a local. On success we own the returned array.
    let status = unsafe { CopySymbolicHotKeys(&mut raw) };
    if status != noErr || raw.is_null() {
        log::warn!("CopySymbolicHotKeys failed with status {status}");
        return Vec::new();
    }
    // SAFETY: "Copy" rule, so we own one reference. Rows are CFDictionary.
    let rows: CFArray<CFDictionary<CFString, CFType>> = unsafe { CFArray::wrap_under_create_rule(raw.cast()) };
    let number = |row: &CFDictionary<CFString, CFType>, key: &str| {
        row.find(CFString::new(key)).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_i64())
    };
    rows.iter()
        .map(|row| SymbolicHotkey {
            keycode: number(&row, "kHISymbolicHotKeyCode").unwrap_or(-1),
            carbon_mods: number(&row, "kHISymbolicHotKeyModifiers").unwrap_or(0) as u32,
            enabled: row
                .find(CFString::new("kHISymbolicHotKeyEnabled"))
                .and_then(|v| v.downcast::<CFBoolean>())
                .is_some_and(bool::from),
        })
        .collect()
}

/// All conflicts for `hotkey`: system shortcuts first, then running apps.
pub fn conflicts(hotkey: &Hotkey) -> Vec<HotkeyConflict> {
    let mut sources = match_system(&system_hotkeys(), hotkey);
    sources.extend(match_known_apps(hotkey, &context::running_apps(), read_setting));
    sources.into_iter().map(|source| HotkeyConflict { hotkey: *hotkey, source }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hk(s: &str) -> Hotkey {
        s.parse().unwrap()
    }

    fn app(bundle_id: &str) -> AppInfo {
        AppInfo { bundle_id: bundle_id.into(), name: "x".into(), pid: 1 }
    }

    #[test]
    fn spotlight_is_found_with_its_enabled_flag() {
        // As returned by the live list: Cmd+Space with the Carbon mask 0x100.
        let list = [SymbolicHotkey { keycode: 49, carbon_mods: 0x100, enabled: false }];
        assert_eq!(
            match_system(&list, &hk("cmd+space")),
            vec![ConflictSource::System { name: "Spotlight search".into(), enabled: false }]
        );
    }

    #[test]
    fn carbon_masks_are_not_cg_flags() {
        // 0x100000 is Cmd in CGEventFlags. In the Carbon mask it means nothing.
        let list = [SymbolicHotkey { keycode: 49, carbon_mods: 0x10_0000, enabled: true }];
        assert!(match_system(&list, &hk("cmd+space")).is_empty());
    }

    #[test]
    fn emoji_and_input_source_rows_get_their_names() {
        let list = [
            SymbolicHotkey { keycode: 49, carbon_mods: 0x1000 | 0x100, enabled: true },
            SymbolicHotkey { keycode: 49, carbon_mods: 0x1000, enabled: false },
            SymbolicHotkey { keycode: 49, carbon_mods: 0x1000 | 0x800, enabled: true },
        ];
        let names = |s: &str| match_system(&list, &hk(s));
        assert_eq!(names("ctrl+cmd+space"), vec![ConflictSource::System { name: "Emoji & Symbols".into(), enabled: true }]);
        assert_eq!(names("ctrl+space"), vec![ConflictSource::System { name: "Select previous input source".into(), enabled: false }]);
        assert_eq!(names("ctrl+opt+space"), vec![ConflictSource::System { name: "Select next input source".into(), enabled: true }]);
    }

    #[test]
    fn extra_modifier_bits_in_the_row_are_ignored_but_missing_ones_do_not_match() {
        let list = [SymbolicHotkey { keycode: 49, carbon_mods: 0x100 | 0x8_0000, enabled: true }];
        assert_eq!(match_system(&list, &hk("cmd+space")).len(), 1);
        assert!(match_system(&list, &hk("cmd+shift+space")).is_empty());
        assert!(match_system(&list, &hk("space")).is_empty());
    }

    #[test]
    fn unknown_rows_get_a_generic_name_and_duplicates_collapse() {
        let list = [
            SymbolicHotkey { keycode: 40, carbon_mods: 0x100, enabled: true },
            SymbolicHotkey { keycode: 40, carbon_mods: 0x100, enabled: true },
        ];
        assert_eq!(
            match_system(&list, &hk("cmd+k")),
            vec![ConflictSource::System { name: "A macOS keyboard shortcut".into(), enabled: true }]
        );
    }

    fn unknown(_: AppSetting) -> SettingValue {
        SettingValue::Unknown
    }

    fn names(hits: &[ConflictSource]) -> Vec<(&str, bool)> {
        hits.iter()
            .map(|h| match h {
                ConflictSource::App { name, confirmed, .. } => (name.as_str(), *confirmed),
                other => panic!("unexpected {other:?}"),
            })
            .collect()
    }

    #[test]
    fn solo_modifiers_never_conflict() {
        let list = [SymbolicHotkey { keycode: 61, carbon_mods: 0, enabled: true }];
        assert!(match_system(&list, &hk("right_option")).is_empty());
        assert!(match_known_apps(&hk("right_option"), &[app("com.openai.chat")], unknown).is_empty());
    }

    #[test]
    fn known_apps_match_only_when_running() {
        let toggle = Hotkey::toggle_default();
        assert!(match_known_apps(&toggle, &[app("com.apple.finder")], unknown).is_empty());
        let hits = match_known_apps(&toggle, &[app("com.apple.finder"), app("com.raycast.macos"), app("com.openai.chat")], unknown);
        assert_eq!(names(&hits), [("ChatGPT", false), ("Raycast", false)]);
    }

    #[test]
    fn chatgpt_is_found_under_its_codex_bundle_id() {
        let hits = match_known_apps(&Hotkey::toggle_default(), &[app("com.openai.codex")], unknown);
        assert_eq!(names(&hits), [("ChatGPT", false)]);
    }

    #[test]
    fn a_read_setting_replaces_the_default() {
        let toggle = Hotkey::toggle_default();
        let running = [app("com.raycast.macos")];
        // Raycast moved to Cmd+Space: no conflict with Option+Space.
        let moved = |_: AppSetting| SettingValue::Known(vec![hk("cmd+space")]);
        assert!(match_known_apps(&toggle, &running, moved).is_empty());
        // Raycast set to Option+Space: a confirmed conflict.
        let same = |_: AppSetting| SettingValue::Known(vec![hk("opt+space")]);
        assert_eq!(names(&match_known_apps(&toggle, &running, same)), [("Raycast", true)]);
        // Raycast set to the user's other key.
        assert_eq!(names(&match_known_apps(&hk("cmd+space"), &running, moved)), [("Raycast", true)]);
        // Raycast hotkey turned off.
        let off = |_: AppSetting| SettingValue::Known(Vec::new());
        assert!(match_known_apps(&toggle, &running, off).is_empty());
    }

    #[test]
    fn dashed_keycodes_parse() {
        assert_eq!(parse_dashed_keycode("Command-49"), SettingValue::Known(vec![hk("cmd+space")]));
        assert_eq!(parse_dashed_keycode("Option-49\n"), SettingValue::Known(vec![hk("opt+space")]));
        assert_eq!(parse_dashed_keycode("Control-Shift-2"), SettingValue::Known(vec![hk("ctrl+shift+d")]));
        assert_eq!(parse_dashed_keycode(""), SettingValue::Known(Vec::new()));
        assert_eq!(parse_dashed_keycode("Hyper-49"), SettingValue::Unknown);
        assert_eq!(parse_dashed_keycode("Option-x"), SettingValue::Unknown);
    }

    #[test]
    fn electron_accelerators_parse() {
        assert_eq!(parse_accelerator("Alt+Space"), Some(hk("opt+space")));
        assert_eq!(parse_accelerator("CommandOrControl+Shift+K"), Some(hk("cmd+shift+k")));
        assert_eq!(parse_accelerator("Super+Alt+P"), Some(hk("cmd+opt+p")));
        assert_eq!(parse_accelerator("Ctrl+Option+D"), Some(hk("ctrl+opt+d")));
        assert_eq!(parse_accelerator("Hyper+Space"), None);
    }

    #[test]
    fn codex_keymap_uses_defaults_and_overrides() {
        // No file: Show Mini has its default.
        assert_eq!(parse_codex_keymap(None), SettingValue::Known(vec![hk("opt+space")]));
        // Turned off, as on the Mac this was written on.
        let off = r#"[{"command": "openAvatarOverlay", "key": null}]"#;
        assert_eq!(parse_codex_keymap(Some(off)), SettingValue::Known(Vec::new()));
        // Moved, plus a dictation key, and an entry for an app-only command.
        let moved = r#"[{"command":"openAvatarOverlay","key":"CmdOrCtrl+Shift+Space"},
                        {"command":"globalDictationHold","key":"Ctrl+Alt+D"},
                        {"command":"newTask","key":"Alt+Space"}]"#;
        assert_eq!(parse_codex_keymap(Some(moved)), SettingValue::Known(vec![hk("cmd+shift+space"), hk("ctrl+opt+d")]));
        // Keys Sayso cannot name are skipped; a broken file is unknown.
        let odd = r#"[{"command":"globalDictationHold","key":"Hyper"}]"#;
        assert_eq!(parse_codex_keymap(Some(odd)), SettingValue::Known(vec![hk("opt+space")]));
        assert_eq!(parse_codex_keymap(Some("{not json")), SettingValue::Unknown);
    }

    #[test]
    fn chatgpt_counts_only_when_its_keymap_says_so() {
        let toggle = Hotkey::toggle_default();
        let running = [app("com.openai.codex")];
        let off = |_: AppSetting| parse_codex_keymap(Some(r#"[{"command":"openAvatarOverlay","key":null}]"#));
        assert!(match_known_apps(&toggle, &running, off).is_empty());
        let default = |_: AppSetting| parse_codex_keymap(None);
        assert_eq!(names(&match_known_apps(&toggle, &running, default)), [("ChatGPT", true)]);
    }

    #[test]
    fn known_apps_only_match_their_own_hotkey() {
        assert!(match_known_apps(&hk("ctrl+cmd+v"), &[app("com.openai.chat")], unknown).is_empty());
    }

    #[test]
    fn every_known_app_hotkey_parses() {
        for app in KNOWN_APPS {
            assert!(app.hotkey.parse::<Hotkey>().is_ok(), "{}", app.name);
        }
    }

    #[test]
    #[ignore = "reads this Mac's Raycast preferences"]
    fn reads_raycast_on_this_mac() {
        let raycast = KNOWN_APPS.iter().find(|a| a.name == "Raycast").unwrap();
        println!("{:?}", read_setting(raycast.setting.unwrap()));
    }

    #[test]
    #[ignore = "reads this Mac's ChatGPT keymap"]
    fn reads_chatgpt_on_this_mac() {
        println!("{:?}", read_setting(AppSetting::CodexKeymap));
    }
}
