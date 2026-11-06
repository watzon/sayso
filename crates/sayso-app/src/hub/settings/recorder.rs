//! Hotkey recording, shared by Settings › Dictation and onboarding.
//!
//! Two listeners run while a recorder is open:
//! - the system key listener (`record_hotkey`), which needs Input Monitoring
//!   and also sees solo modifiers such as Right Option;
//! - key presses in our own window, which work without any permission.
//!
//! The first complete combination wins.

use super::ext::Recorded;
use crate::model::AppModel;
use gpui_kit::*;
use sayso_core::config::Config;
use sayso_core::hotkey::{Hotkey, Key};
use sayso_i18n::t;
use sayso_platform::{ConflictSource, HotkeyConflict};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot {
    Toggle,
    PushToTalk,
    PasteLast,
    CycleStyle,
    Incognito,
    /// The style hotkey of the style with this id (`[style_hotkeys]`). The
    /// style editor keeps a new key until Save, so this slot stages the key.
    Style(SharedString),
}

impl Slot {
    pub fn get(&self, c: &Config) -> Option<Hotkey> {
        match self {
            Slot::Toggle => c.hotkeys.toggle,
            Slot::PushToTalk => c.hotkeys.push_to_talk,
            Slot::PasteLast => c.hotkeys.paste_last,
            Slot::CycleStyle => c.hotkeys.cycle_style,
            Slot::Incognito => c.hotkeys.incognito,
            Slot::Style(id) => c.style_hotkeys.get(id),
        }
    }

    pub fn apply(&self, m: &mut AppModel, hotkey: Option<Hotkey>, cx: &mut Context<AppModel>) {
        match self {
            Slot::Toggle => m.set_toggle_hotkey(hotkey, cx),
            Slot::PushToTalk => m.set_push_to_talk(hotkey, cx),
            Slot::PasteLast => m.edit_config(cx, |c| c.hotkeys.paste_last = hotkey),
            Slot::CycleStyle => m.edit_config(cx, |c| c.hotkeys.cycle_style = hotkey),
            Slot::Incognito => m.edit_config(cx, |c| c.hotkeys.incognito = hotkey),
            Slot::Style(id) => m.edit_config(cx, |c| c.style_hotkeys.set(id, hotkey)),
        }
    }

    /// A recorded key goes to the view, not to the config file.
    fn stages(&self) -> bool {
        matches!(self, Slot::Style(_))
    }

    /// Another Sayso action that uses or overlaps `hotkey`: a hotkey of
    /// `[hotkeys]` or the style hotkey of another style.
    pub fn clash(&self, c: &Config, hotkey: &Hotkey) -> Option<Slot> {
        let styles = c.style_hotkeys.iter().map(|(id, _)| Slot::Style(SharedString::from(id.to_string())));
        [Slot::Toggle, Slot::PushToTalk, Slot::PasteLast, Slot::CycleStyle, Slot::Incognito]
            .into_iter()
            .chain(styles)
            .find(|s| s != self && s.get(c).is_some_and(|other| match (self, s) {
                (Slot::Toggle, _) | (_, Slot::Toggle) => other.overlaps(*hotkey),
                _ => other == *hotkey,
            }))
    }

    /// The name of the action, for sentences. A style has
    /// `Note::UsedByStyle` with its name instead.
    fn label(&self) -> String {
        match self {
            Slot::Toggle => t!("settings.recorder.action_toggle"),
            Slot::PushToTalk => t!("settings.recorder.action_push_to_talk"),
            Slot::PasteLast => t!("settings.recorder.action_paste_last"),
            Slot::CycleStyle => t!("settings.recorder.action_cycle_style"),
            Slot::Incognito => t!("settings.recorder.action_incognito"),
            Slot::Style(id) => t!("settings.recorder.action_style", style = id),
        }
    }
}

/// Why a recorded key was not saved. The recorder keeps this, not the text,
/// so the note changes with the interface language.
#[derive(Debug, Clone, PartialEq)]
pub enum Note {
    /// Modifier-only keys for an action other than toggle or push-to-talk.
    SoloOnlyForPushToTalk(Hotkey),
    /// Another Sayso action uses the key.
    UsedBy(Hotkey, Slot),
    /// The style hotkey of another style uses the key: the name of that style.
    UsedByStyle(Hotkey, String),
}

impl Note {
    pub fn text(&self) -> String {
        match self {
            Note::SoloOnlyForPushToTalk(hk) => {
                t!("settings.recorder.solo_only_push_to_talk", keys = words(hk), example = words(&Hotkey::toggle_default()))
            }
            Note::UsedBy(hk, other) => t!("settings.recorder.used_by", keys = words(hk), action = other.label()),
            Note::UsedByStyle(hk, name) => t!("settings.recorder.used_by", keys = words(hk), action = t!("settings.recorder.action_style", style = name)),
        }
    }
}

impl From<Note> for SharedString {
    fn from(note: Note) -> Self {
        note.text().into()
    }
}

/// Recorder state that a view owns.
pub struct Recorder {
    pub slot: Option<Slot>,
    /// The system listener is not available, so only keys in this window count.
    pub window_only: bool,
    /// A message after the last attempt, for example "Already used by ...".
    pub note: Option<Note>,
    /// A key that a staging slot recorded. The view takes it.
    staged: Option<Hotkey>,
    generation: u64,
    pub focus: FocusHandle,
    conflicts: Vec<(Option<Hotkey>, Instant, Vec<HotkeyConflict>)>,
}

impl Recorder {
    pub fn new(cx: &mut App) -> Self {
        Self { slot: None, window_only: false, note: None, staged: None, generation: 0, focus: cx.focus_handle(), conflicts: Vec::new() }
    }

    pub fn is_recording(&self, slot: &Slot) -> bool {
        self.slot.as_ref() == Some(slot)
    }

    /// The key that a staging slot recorded since the last call (see `Slot::Style`).
    pub fn take_staged(&mut self) -> Option<Hotkey> {
        self.staged.take()
    }

    /// Conflicts for a hotkey, cached for a few seconds (running apps change).
    pub fn conflicts(&mut self, model: &AppModel, hotkey: Option<Hotkey>) -> Vec<HotkeyConflict> {
        let Some(hk) = hotkey else { return Vec::new() };
        if let Some((_, t, list)) = self.conflicts.iter().find(|(h, _, _)| *h == hotkey)
            && t.elapsed() < Duration::from_secs(4) {
                return list.clone();
            }
        let list: Vec<HotkeyConflict> = model
            .hotkey_conflicts(&hk)
            .into_iter()
            .filter(|c| !matches!(c.source, ConflictSource::System { enabled: false, .. }))
            .collect();
        self.conflicts.retain(|(h, _, _)| *h != hotkey);
        self.conflicts.push((hotkey, Instant::now(), list.clone()));
        list
    }

    pub fn cancel(&mut self) {
        self.slot = None;
        self.generation += 1;
    }
}

/// Start recording into `slot`. `get` finds the recorder inside the view.
pub fn start<V: 'static>(
    get: fn(&mut V) -> &mut Recorder,
    this: &mut V,
    model: &Entity<AppModel>,
    slot: Slot,
    window: &mut Window,
    cx: &mut Context<V>,
) {
    let r = get(this);
    r.slot = Some(slot.clone());
    r.window_only = false;
    r.note = None;
    r.generation += 1;
    let generation = r.generation;
    r.focus.focus(window, cx);
    let view = cx.entity().downgrade();
    model.update(cx, move |m, cx| {
        m.record_hotkey(cx, move |m, result, cx| {
            let current = view
                .update(cx, |v, _| {
                    let r = get(v);
                    r.generation == generation && r.slot.as_ref() == Some(&slot)
                })
                .unwrap_or(false);
            if !current {
                return;
            }
            match result {
                Recorded::Key(hk) => {
                    let done = finish(m, &slot, hk, cx);
                    let _ = view.update(cx, |v, cx| {
                        let r = get(v);
                        r.slot = None;
                        r.take(done);
                        cx.notify();
                    });
                }
                Recorded::Unavailable => {
                    let _ = view.update(cx, |v, cx| {
                        get(v).window_only = true;
                        cx.notify();
                    });
                }
                Recorded::TimedOut => {
                    let _ = view.update(cx, |v, cx| {
                        get(v).slot = None;
                        cx.notify();
                    });
                }
            }
        });
    });
    cx.notify();
}

/// Modifier keys work for start/stop and push-to-talk. The other actions
/// keep their existing chords.
fn unusable(slot: &Slot, hk: &Hotkey) -> Option<Note> {
    match hk {
        Hotkey::Solo(_) | Hotkey::ModifierOnly(_) if !matches!(slot, Slot::Toggle | Slot::PushToTalk) => Some(Note::SoloOnlyForPushToTalk(*hk)),
        _ => None,
    }
}

/// What happened to a recorded key.
#[derive(Debug, Clone, PartialEq)]
enum Finished {
    /// The key is in the config file.
    Saved,
    /// A staging slot: the view keeps the key until the user saves.
    Staged(Hotkey),
    /// The key was not used, for this reason.
    Refused(Note),
}

impl Recorder {
    fn take(&mut self, done: Finished) {
        match done {
            Finished::Saved => self.note = None,
            Finished::Staged(hk) => {
                self.note = None;
                self.staged = Some(hk);
            }
            Finished::Refused(note) => self.note = Some(note),
        }
    }
}

/// Check a recorded key against the action and the other Sayso hotkeys.
fn check(m: &AppModel, slot: &Slot, hk: Hotkey) -> Result<(), Note> {
    if let Some(note) = unusable(slot, &hk) {
        return Err(note);
    }
    match slot.clash(&m.config, &hk) {
        Some(Slot::Style(id)) => Err(Note::UsedByStyle(hk, m.style_name(&id))),
        Some(other) => Err(Note::UsedBy(hk, other)),
        None => Ok(()),
    }
}

/// Save or stage a recorded key unless the action cannot use it or another
/// Sayso action uses it.
fn finish(m: &mut AppModel, slot: &Slot, hk: Hotkey, cx: &mut Context<AppModel>) -> Finished {
    if let Err(note) = check(m, slot, hk) {
        return Finished::Refused(note);
    }
    if slot.stages() {
        return Finished::Staged(hk);
    }
    slot.apply(m, Some(hk), cx);
    Finished::Saved
}

/// Handle a key press in the window while a recorder is open. Returns true when it used the key.
pub fn on_key_down<V: 'static>(
    get: fn(&mut V) -> &mut Recorder,
    this: &mut V,
    model: &Entity<AppModel>,
    event: &KeyDownEvent,
    cx: &mut Context<V>,
) -> bool {
    let r = get(this);
    let Some(slot) = r.slot.clone() else { return false };
    let k = &event.keystroke;
    let m = k.modifiers;
    if k.key == "escape" && !(m.control || m.alt || m.shift || m.platform) {
        r.cancel();
        cx.notify();
        return true;
    }
    let Some(hk) = keystroke_hotkey(k) else { return true };
    r.cancel();
    let done = model.update(cx, |m, cx| finish(m, &slot, hk, cx));
    get(this).take(done);
    cx.notify();
    true
}

/// Map a GPUI keystroke to a hotkey. Bare letters, digits, and punctuation
/// keys need a modifier. GPUI names a punctuation key by the character it
/// types; with Shift it gives the shifted character (`{`), which Sayso cannot
/// map back to a key, so only the system listener records those.
pub fn keystroke_hotkey(k: &Keystroke) -> Option<Hotkey> {
    let m = k.modifiers;
    let key = match k.key.as_str() {
        "space" => "space".to_string(),
        "enter" => "return".to_string(),
        "tab" => "tab".to_string(),
        "backspace" => "backspace".to_string(),
        "escape" => "esc".to_string(),
        s if s.len() == 1 && s.chars().all(|c| c.is_ascii_alphanumeric()) => s.to_ascii_lowercase(),
        s if s.starts_with('f') && s[1..].parse::<u8>().is_ok() => s.to_string(),
        s if s.chars().count() == 1 && s.chars().next().and_then(Key::punctuation_index).is_some() => s.to_string(),
        _ => return None,
    };
    let any_mod = m.control || m.alt || m.shift || m.platform || m.function;
    if !any_mod && !key.starts_with('f') {
        return None;
    }
    let mut parts = Vec::new();
    if m.function {
        parts.push("fn");
    }
    if m.control {
        parts.push("ctrl");
    }
    if m.alt {
        parts.push("opt");
    }
    if m.shift {
        parts.push("shift");
    }
    if m.platform {
        parts.push("cmd");
    }
    let text = parts.into_iter().map(str::to_string).chain([key]).collect::<Vec<_>>().join("+");
    text.parse().ok()
}

/// A hotkey in words, for sentences: "Option+Space".
pub fn words(hk: &Hotkey) -> String {
    hk.keycaps()
        .into_iter()
        .map(|cap| match cap.as_str() {
            "⌃" => "Control".to_string(),
            "⌥" => "Option".to_string(),
            "⇧" => "Shift".to_string(),
            "⌘" => "Command".to_string(),
            "↵" => "Return".to_string(),
            "⇥" => "Tab".to_string(),
            "⌫" => "Delete".to_string(),
            "fn" => "Fn".to_string(),
            other => other.replace('⌥', "Option").replace('⌘', "Command").replace('⌃', "Control").replace('⇧', "Shift"),
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// The banner text for one conflict.
pub fn conflict_text(c: &HotkeyConflict, open_app: bool) -> String {
    let keys = words(&c.hotkey);
    match &c.source {
        ConflictSource::App { name, confirmed: true, .. } => t!("settings.recorder.conflict_app", name = name, keys = keys),
        ConflictSource::App { name, confirmed: false, .. } if open_app => t!("settings.recorder.conflict_app_open_default", name = name, keys = keys),
        ConflictSource::App { name, confirmed: false, .. } => t!("settings.recorder.conflict_app_default", name = name, keys = keys),
        ConflictSource::System { name, .. } => t!(
            crate::shell::os_text!("settings.recorder.conflict_system_mac", "settings.recorder.conflict_system_windows", "settings.recorder.conflict_system_other"),
            keys = keys,
            name = shortcut_name(name)
        ),
    }
}

/// The name of a system shortcut in the interface language. The platform
/// crates give the English name (it is also in the log); a name that is not
/// here, such as a product name, stays as it is.
fn shortcut_name(name: &str) -> String {
    match name {
        "Spotlight search" => t!("settings.recorder.shortcut.spotlight_search"),
        "Finder search window" => t!("settings.recorder.shortcut.finder_search_window"),
        "Select previous input source" => t!("settings.recorder.shortcut.select_previous_input_source"),
        "Select next input source" => t!("settings.recorder.shortcut.select_next_input_source"),
        "Emoji & Symbols" => t!("settings.recorder.shortcut.emoji_symbols"),
        "Application windows" => t!("settings.recorder.shortcut.application_windows"),
        "Move left a space" => t!("settings.recorder.shortcut.move_left_a_space"),
        "Move right a space" => t!("settings.recorder.shortcut.move_right_a_space"),
        "Screenshot (whole screen)" => t!("settings.recorder.shortcut.screenshot_whole_screen"),
        "Screenshot (selection)" => t!("settings.recorder.shortcut.screenshot_selection"),
        "Screenshot and recording options" => t!("settings.recorder.shortcut.screenshot_and_recording_options"),
        "Move focus to next window of the app" => t!("settings.recorder.shortcut.move_focus_to_next_window_of_the_app"),
        "Dock: turn hiding on or off" => t!("settings.recorder.shortcut.dock_turn_hiding_on_or_off"),
        "A macOS keyboard shortcut" => t!("settings.recorder.shortcut.a_macos_keyboard_shortcut"),
        "Window menu" => t!("settings.recorder.shortcut.window_menu"),
        "Switch apps" => t!("settings.recorder.shortcut.switch_apps"),
        "Close window" => t!("settings.recorder.shortcut.close_window"),
        "Cycle through windows" => t!("settings.recorder.shortcut.cycle_through_windows"),
        "Start menu" => t!("settings.recorder.shortcut.start_menu"),
        "Task Manager" => t!("settings.recorder.shortcut.task_manager"),
        "Switch input language" => t!("settings.recorder.shortcut.switch_input_language"),
        "Previous input language" => t!("settings.recorder.shortcut.previous_input_language"),
        "Task view" => t!("settings.recorder.shortcut.task_view"),
        "Close Magnifier" => t!("settings.recorder.shortcut.close_magnifier"),
        "Sound output" => t!("settings.recorder.shortcut.sound_output"),
        "Color filters" => t!("settings.recorder.shortcut.color_filters"),
        "New virtual desktop" => t!("settings.recorder.shortcut.new_virtual_desktop"),
        "Close virtual desktop" => t!("settings.recorder.shortcut.close_virtual_desktop"),
        "Narrator settings" => t!("settings.recorder.shortcut.narrator_settings"),
        "On-Screen Keyboard" => t!("settings.recorder.shortcut.on_screen_keyboard"),
        "Quick Assist" => t!("settings.recorder.shortcut.quick_assist"),
        "Narrator" => t!("settings.recorder.shortcut.narrator"),
        "Restart the graphics driver" => t!("settings.recorder.shortcut.restart_the_graphics_driver"),
        "Restore minimized windows" => t!("settings.recorder.shortcut.restore_minimized_windows"),
        "Snipping Tool screen recording" => t!("settings.recorder.shortcut.snipping_tool_screen_recording"),
        "Snipping Tool" => t!("settings.recorder.shortcut.snipping_tool"),
        "HDR on or off" => t!("settings.recorder.shortcut.hdr_on_or_off"),
        "Date and time" => t!("settings.recorder.shortcut.date_and_time"),
        "Mute the microphone in a call" => t!("settings.recorder.shortcut.mute_the_microphone_in_a_call"),
        "Game Bar recording" => t!("settings.recorder.shortcut.game_bar_recording"),
        "Reserved for debuggers" => t!("settings.recorder.shortcut.reserved_for_debuggers"),
        "Quick Settings" => t!("settings.recorder.shortcut.quick_settings"),
        "Notification area" => t!("settings.recorder.shortcut.notification_area"),
        "Show the desktop" => t!("settings.recorder.shortcut.show_the_desktop"),
        "File Explorer" => t!("settings.recorder.shortcut.file_explorer"),
        "Feedback Hub" => t!("settings.recorder.shortcut.feedback_hub"),
        "Game Bar" => t!("settings.recorder.shortcut.game_bar"),
        "Voice typing" => t!("settings.recorder.shortcut.voice_typing"),
        "Settings" => t!("settings.recorder.shortcut.settings"),
        "A Windows shortcut" => t!("settings.recorder.shortcut.a_windows_shortcut"),
        "Cast" => t!("settings.recorder.shortcut.cast"),
        "Lock the PC" => t!("settings.recorder.shortcut.lock_the_pc"),
        "Minimize all windows" => t!("settings.recorder.shortcut.minimize_all_windows"),
        "Notification center" => t!("settings.recorder.shortcut.notification_center"),
        "Lock the screen rotation" => t!("settings.recorder.shortcut.lock_the_screen_rotation"),
        "Project to a screen" => t!("settings.recorder.shortcut.project_to_a_screen"),
        "Search" => t!("settings.recorder.shortcut.search"),
        "Run" => t!("settings.recorder.shortcut.run"),
        "Taskbar apps" => t!("settings.recorder.shortcut.taskbar_apps"),
        "Accessibility settings" => t!("settings.recorder.shortcut.accessibility_settings"),
        "Clipboard history" => t!("settings.recorder.shortcut.clipboard_history"),
        "Widgets" => t!("settings.recorder.shortcut.widgets"),
        "Quick Link menu" => t!("settings.recorder.shortcut.quick_link_menu"),
        "Snap layouts" => t!("settings.recorder.shortcut.snap_layouts"),
        "Open a taskbar app" => t!("settings.recorder.shortcut.open_a_taskbar_app"),
        "the window menu" => t!("settings.recorder.shortcut.the_window_menu"),
        "Switch input source" => t!("settings.recorder.shortcut.switch_input_source"),
        "Switch applications" => t!("settings.recorder.shortcut.switch_applications"),
        "the Run a command dialog" => t!("settings.recorder.shortcut.the_run_a_command_dialog"),
        "the notification list" => t!("settings.recorder.shortcut.the_notification_list"),
        "Show all apps" => t!("settings.recorder.shortcut.show_all_apps"),
        "Launch terminal" => t!("settings.recorder.shortcut.launch_terminal"),
        "Lock screen" => t!("settings.recorder.shortcut.lock_screen"),
        "Walk through windows" => t!("settings.recorder.shortcut.walk_through_windows"),
        "Lock session" => t!("settings.recorder.shortcut.lock_session"),
        "Launch Konsole" => t!("settings.recorder.shortcut.launch_konsole"),
        "Switch windows" => t!("settings.recorder.shortcut.switch_windows"),
        "the Run dialog" => t!("settings.recorder.shortcut.the_run_dialog"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the GPUI glob has its own `test` macro.
    use super::{Note, Slot, keystroke_hotkey, unusable};
    use sayso_core::hotkey::{Hotkey, Key, SoloModifier};

    fn keystroke(key: &str, control: bool) -> gpui_kit::Keystroke {
        gpui_kit::Keystroke {
            modifiers: gpui_kit::Modifiers { control, ..Default::default() },
            key: key.into(),
            key_char: None,
        }
    }

    #[test]
    fn window_keys_record_punctuation_chords() {
        assert_eq!(keystroke_hotkey(&keystroke("[", true)), Some("ctrl+[".parse().unwrap()));
        assert_eq!(keystroke_hotkey(&keystroke("]", true)), Some("ctrl+]".parse().unwrap()));
        for c in Key::PUNCTUATION {
            let hotkey = keystroke_hotkey(&keystroke(&c.to_string(), true));
            assert!(matches!(hotkey, Some(Hotkey::Chord { key: Key::Punctuation(k), .. }) if k == c), "{c}");
            assert_eq!(keystroke_hotkey(&keystroke(&c.to_string(), false)), None, "a bare {c} is not a hotkey");
        }
        // GPUI gives the shifted character for Shift with a punctuation key.
        assert_eq!(keystroke_hotkey(&keystroke("{", true)), None);
    }

    #[test]
    fn modifier_toggle_conflicts_are_rejected_in_either_assignment_order() {
        use sayso_core::config::Config;

        for (toggle, other) in [
            ("left_control", "left_control"),
            ("left_control", "left_control+left_option"),
            ("left_control+left_option", "left_control"),
        ] {
            let toggle: Hotkey = toggle.parse().unwrap();
            let other: Hotkey = other.parse().unwrap();
            for slot in [Slot::PushToTalk, Slot::PasteLast, Slot::CycleStyle, Slot::Incognito] {
                let mut config = Config::default();
                config.hotkeys.toggle = Some(toggle);
                config.hotkeys.paste_last = None;
                config.hotkeys.cycle_style = None;
                assert_eq!(slot.clash(&config, &other), Some(Slot::Toggle), "{toggle}, {other}");
                config.hotkeys.toggle = None;
                match &slot {
                    Slot::PushToTalk => config.hotkeys.push_to_talk = Some(other),
                    Slot::PasteLast => config.hotkeys.paste_last = Some(other),
                    Slot::CycleStyle => config.hotkeys.cycle_style = Some(other),
                    Slot::Incognito => config.hotkeys.incognito = Some(other),
                    Slot::Toggle | Slot::Style(_) => unreachable!(),
                }
                assert_eq!(Slot::Toggle.clash(&config, &toggle), Some(slot));
            }
        }
    }

    #[test]
    fn distinct_modifiers_and_existing_chords_remain_available() {
        use sayso_core::config::Config;

        for (toggle, ptt) in [
            ("left_control+left_option", "right_control+right_option"),
            ("left_control", "right_control"),
            ("left_control+left_option", "shift+d"),
            ("left_control+left_option", "ctrl+opt+d"),
            ("left_control", "ctrl+opt+d"),
            ("ctrl+opt+space", "ctrl+opt+d"),
            ("opt+space", "right_option"),
        ] {
            let toggle = toggle.parse().unwrap();
            let ptt = ptt.parse().unwrap();
            let mut config = Config::default();
            config.hotkeys.toggle = Some(toggle);
            config.hotkeys.push_to_talk = Some(ptt);
            config.hotkeys.paste_last = None;
            config.hotkeys.cycle_style = None;
            assert_eq!(Slot::PushToTalk.clash(&config, &ptt), None);
            assert_eq!(Slot::Toggle.clash(&config, &toggle), None);
            assert!(config.validate().is_empty(), "{toggle}, {ptt}");
        }
    }

    #[test]
    fn combined_modifiers_work_for_recording_without_changing_other_actions() {
        for binding in ["left_command+left_option", "left_command+left_control+left_option+left_shift"] {
            let hotkey = binding.parse::<Hotkey>().unwrap();
            assert_eq!(unusable(&Slot::Toggle, &hotkey), None);
            assert_eq!(unusable(&Slot::PushToTalk, &hotkey), None);
            for slot in [Slot::PasteLast, Slot::CycleStyle, Slot::Incognito] {
                assert!(unusable(&slot, &hotkey).is_some());
                assert_eq!(unusable(&slot, &Hotkey::paste_last_default()), None);
            }
        }
    }

    #[test]
    fn solo_modifiers_work_for_both_recording_actions() {
        let solo = Hotkey::Solo(SoloModifier::RightOption);
        assert_eq!(unusable(&Slot::PushToTalk, &solo), None);
        assert_eq!(unusable(&Slot::Toggle, &solo), None);
        for slot in [Slot::PasteLast, Slot::CycleStyle, Slot::Incognito] {
            assert_eq!(unusable(&slot, &solo), Some(Note::SoloOnlyForPushToTalk(solo)));
        }
        assert_eq!(unusable(&Slot::Toggle, &Hotkey::toggle_default()), None);
    }

    fn style(id: &str) -> Slot {
        Slot::Style(id.to_string().into())
    }

    #[test]
    fn a_style_hotkey_clashes_with_every_other_sayso_hotkey_but_its_own() {
        use sayso_core::config::Config;

        let key = |s: &str| s.parse::<Hotkey>().unwrap();
        let mut config = Config::default();
        config.hotkeys.toggle = Some(key("ctrl+opt+d"));
        config.hotkeys.cycle_style = Some(key("ctrl+opt+s"));
        config.style_hotkeys.set("email", Some(key("ctrl+opt+e")));
        assert_eq!(style("notes").clash(&config, &key("ctrl+opt+d")), Some(Slot::Toggle));
        assert_eq!(style("notes").clash(&config, &key("ctrl+opt+s")), Some(Slot::CycleStyle));
        assert_eq!(style("notes").clash(&config, &key("ctrl+opt+e")), Some(style("email")));
        // The key that the style has already is not a clash, and a new style has no id yet.
        assert_eq!(style("email").clash(&config, &key("ctrl+opt+e")), None);
        assert_eq!(style("").clash(&config, &key("ctrl+opt+n")), None);
        // The fixed hotkeys see the style hotkeys too.
        assert_eq!(Slot::Incognito.clash(&config, &key("ctrl+opt+e")), Some(style("email")));
        assert_eq!(Slot::Toggle.clash(&config, &key("ctrl+opt+e")), Some(style("email")));
    }

    #[test]
    fn a_style_hotkey_needs_a_regular_key_and_is_staged() {
        for binding in ["right_option", "left_command+left_option"] {
            let hotkey = binding.parse::<Hotkey>().unwrap();
            assert_eq!(unusable(&style("email"), &hotkey), Some(Note::SoloOnlyForPushToTalk(hotkey)));
        }
        assert_eq!(unusable(&style("email"), &"ctrl+opt+e".parse().unwrap()), None);
        assert!(style("email").stages());
        assert!(!Slot::Toggle.stages() && !Slot::CycleStyle.stages());
    }
}
