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
use sayso_core::hotkey::Hotkey;
use sayso_platform::{ConflictSource, HotkeyConflict};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Toggle,
    PushToTalk,
    PasteLast,
    CycleStyle,
}

impl Slot {
    pub fn get(self, c: &Config) -> Option<Hotkey> {
        match self {
            Slot::Toggle => c.hotkeys.toggle,
            Slot::PushToTalk => c.hotkeys.push_to_talk,
            Slot::PasteLast => c.hotkeys.paste_last,
            Slot::CycleStyle => c.hotkeys.cycle_style,
        }
    }

    pub fn apply(self, m: &mut AppModel, hotkey: Option<Hotkey>, cx: &mut Context<AppModel>) {
        match self {
            Slot::Toggle => m.set_toggle_hotkey(hotkey, cx),
            Slot::PushToTalk => m.set_push_to_talk(hotkey, cx),
            Slot::PasteLast => m.edit_config(cx, |c| c.hotkeys.paste_last = hotkey),
            Slot::CycleStyle => m.edit_config(cx, |c| c.hotkeys.cycle_style = hotkey),
        }
    }

    /// Another Sayso action that already uses `hotkey`.
    pub fn clash(self, c: &Config, hotkey: &Hotkey) -> Option<&'static str> {
        [
            (Slot::Toggle, "Start and stop dictation"),
            (Slot::PushToTalk, "Push to talk"),
            (Slot::PasteLast, "Paste last text"),
            (Slot::CycleStyle, "Next style"),
        ]
        .into_iter()
        .find(|(s, _)| *s != self && s.get(c).as_ref() == Some(hotkey))
        .map(|(_, name)| name)
    }
}

/// Recorder state that a view owns.
pub struct Recorder {
    pub slot: Option<Slot>,
    /// The system listener is not available, so only keys in this window count.
    pub window_only: bool,
    /// A message after the last attempt, for example "Already used by ...".
    pub note: Option<String>,
    generation: u64,
    pub focus: FocusHandle,
    conflicts: Vec<(Option<Hotkey>, Instant, Vec<HotkeyConflict>)>,
}

impl Recorder {
    pub fn new(cx: &mut App) -> Self {
        Self { slot: None, window_only: false, note: None, generation: 0, focus: cx.focus_handle(), conflicts: Vec::new() }
    }

    pub fn is_recording(&self, slot: Slot) -> bool {
        self.slot == Some(slot)
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
    r.slot = Some(slot);
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
                    r.generation == generation && r.slot == Some(slot)
                })
                .unwrap_or(false);
            if !current {
                return;
            }
            match result {
                Recorded::Key(hk) => {
                    let note = finish(m, slot, hk, cx);
                    let _ = view.update(cx, |v, cx| {
                        let r = get(v);
                        r.slot = None;
                        r.note = note;
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

/// Save a recorded key unless another Sayso action uses it. Returns a note for the UI.
fn finish(m: &mut AppModel, slot: Slot, hk: Hotkey, cx: &mut Context<AppModel>) -> Option<String> {
    if let Some(other) = slot.clash(&m.config, &hk) {
        return Some(format!("{} is already used by {other}. Choose another key.", words(&hk)));
    }
    slot.apply(m, Some(hk), cx);
    None
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
    let Some(slot) = r.slot else { return false };
    let k = &event.keystroke;
    let m = k.modifiers;
    if k.key == "escape" && !(m.control || m.alt || m.shift || m.platform) {
        r.cancel();
        cx.notify();
        return true;
    }
    let Some(hk) = keystroke_hotkey(k) else { return true };
    r.cancel();
    let note = model.update(cx, |m, cx| finish(m, slot, hk, cx));
    get(this).note = note;
    cx.notify();
    true
}

/// Map a GPUI keystroke to a hotkey. Bare letters and digits need a modifier.
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
        ConflictSource::App { name, confirmed: true, .. } => {
            format!("{name} also uses {keys}. Both apps will react. Choose another key, or change the shortcut in {name}.")
        }
        ConflictSource::App { name, confirmed: false, .. } => {
            let open = if open_app { " is open and" } else { "" };
            format!(
                "{name}{open} uses {keys} by default. If you did not change it in {name}, both apps will react. Choose another key, or change the shortcut in {name}."
            )
        }
        ConflictSource::System { name, .. } => format!(
            "{system} uses {keys} for {name}. Choose another key, or turn the shortcut off in {settings}.",
            system = crate::shell::os_text!("macOS", "Your desktop"),
            settings = crate::shell::os_text!("System Settings › Keyboard › Keyboard Shortcuts", "the keyboard settings of your desktop"),
        ),
    }
}
