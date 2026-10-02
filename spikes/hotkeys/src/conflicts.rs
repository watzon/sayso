//! Mode `conflicts <chord>`: system hotkey lookup (plist + CopySymbolicHotKeys) and RegisterEventHotKey probe.
use crate::{chord, ffi::*, keys::{self, Chord}};
use core_foundation::{array::CFArray, base::{CFType, TCFType}, boolean::CFBoolean, dictionary::CFDictionary,
    number::CFNumber, string::CFString};
use std::ptr;

const MASK: u64 = FLAG_SHIFT | FLAG_CONTROL | FLAG_ALT | FLAG_COMMAND;
/// CopySymbolicHotKeys reports modifiers in Carbon encoding, not CGEventFlags.
const CARBON_MASK: u64 = (cmdKey | shiftKey | optionKey | controlKey) as u64;

fn symbolic_name(id: &str) -> &'static str {
    match id {
        "7" => "Move focus to menu bar", "8" => "Move focus to Dock", "9" => "Move focus to active/next window",
        "10" => "Move focus to window toolbar", "11" => "Move focus to floating window", "12" => "Turn keyboard access on/off",
        "13" => "Change the way Tab moves focus", "15" => "Turn zoom on/off", "17" => "Zoom in", "19" => "Zoom out",
        "21" => "Invert colors", "23" => "Increase contrast", "25" => "Reduce contrast",
        "27" => "Move focus to next window in app (Cmd+`)", "28" => "Save screen as file (Cmd+Shift+3)",
        "29" => "Copy screen to clipboard (Ctrl+Cmd+Shift+3)", "30" => "Save area as file (Cmd+Shift+4)",
        "31" => "Copy area to clipboard (Ctrl+Cmd+Shift+4)", "32" => "Mission Control", "33" => "Application windows",
        "34" => "Mission Control (Ctrl+Up variant)", "35" => "Application windows (variant)", "36" => "Show Desktop",
        "37" => "Show Desktop (variant)", "51" => "Dock: turn hiding on/off (Opt+Cmd+D)", "52" => "Turn Dock hiding on/off",
        "57" => "Move focus to status menus", "59" => "VoiceOver toggle",
        "60" => "Select previous input source (Ctrl+Space)", "61" => "Select next source in Input menu (Ctrl+Opt+Space)",
        "64" => "Spotlight search (Cmd+Space)", "65" => "Finder search window (Cmd+Opt+Space)",
        "79" => "Move left a space", "80" => "Move left a space (variant)", "81" => "Move right a space",
        "82" => "Move right a space (variant)", "98" => "Show Help menu", "118" => "Switch to Desktop 1",
        "160" => "Show Launchpad", "162" => "Show Notification Center", "163" => "Notification Center",
        "164" => "Turn Do Not Disturb on/off", "175" => "Do Not Disturb (variant)", "176" => "Dictation (Fn/Fn)",
        "179" => "Emoji & Symbols / Globe action", "184" => "Screenshot and recording options (Cmd+Shift+5)",
        "190" => "Quick Note", "222" => "Show Applications (Mission Control)", "233" => "Stage Manager",
        _ => "(unnamed id)",
    }
}

struct Entry { id: String, name: &'static str, enabled: bool, ascii: i64, keycode: i64, mods: u64 }

fn plist_entries() -> Result<Vec<Entry>, String> {
    let out = std::process::Command::new("defaults").args(["export", "com.apple.symbolichotkeys", "-"])
        .output().map_err(|e| e.to_string())?;
    let v = plist::Value::from_reader_xml(&out.stdout[..]).map_err(|e| e.to_string())?;
    let hk = v.as_dictionary().and_then(|d| d.get("AppleSymbolicHotKeys")).and_then(|v| v.as_dictionary())
        .ok_or("no AppleSymbolicHotKeys")?;
    let mut res = vec![];
    for (id, e) in hk {
        let Some(d) = e.as_dictionary() else { continue };
        let enabled = d.get("enabled").and_then(|v| v.as_boolean()).unwrap_or(false);
        let params = d.get("value").and_then(|v| v.as_dictionary()).and_then(|v| v.get("parameters"))
            .and_then(|v| v.as_array());
        let Some(p) = params else { continue };
        let n = |i: usize| p.get(i).and_then(|v| v.as_signed_integer()).unwrap_or(-1);
        res.push(Entry { id: id.clone(), name: symbolic_name(id), enabled, ascii: n(0), keycode: n(1), mods: n(2) as u64 });
    }
    Ok(res)
}

/// Live, effective list from Carbon (includes OS defaults that never appear in the plist).
fn live_entries() -> Result<Vec<(i64, u64, bool)>, String> {
    let mut raw: CFTypeRef = ptr::null();
    let st = unsafe { CopySymbolicHotKeys(&mut raw) };
    if st != noErr || raw.is_null() { return Err(format!("CopySymbolicHotKeys OSStatus {st}")); }
    let arr: CFArray<CFDictionary<CFString, CFType>> = unsafe { CFArray::wrap_under_create_rule(raw as _) };
    let get_i = |d: &CFDictionary<CFString, CFType>, k: &str| {
        d.find(CFString::new(k)).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_i64())
    };
    Ok(arr.iter().map(|d| {
        let en = d.find(CFString::new("kHISymbolicHotKeyEnabled")).and_then(|v| v.downcast::<CFBoolean>())
            .is_some_and(bool::from);
        (get_i(&d, "kHISymbolicHotKeyCode").unwrap_or(-1), get_i(&d, "kHISymbolicHotKeyModifiers").unwrap_or(0) as u64, en)
    }).collect())
}

pub fn run(args: &[String]) {
    let Some(spec) = args.first() else { return println!("usage: conflicts <chord> [--hold SECS] [--twice] [--dump]") };
    let c: Chord = match keys::parse(spec) { Ok(c) => c, Err(e) => return println!("bad chord: {e}") };
    println!("chord: {} (keycode {}, cg mods {:#x}, carbon mods {:#x})", keys::describe(&c), c.keycode, c.cg, c.carbon);
    let dump = args.iter().any(|a| a == "--dump");

    println!("\n(a) com.apple.symbolichotkeys plist (explicit user/OS-written entries only):");
    match plist_entries() {
        Err(e) => println!("  error: {e}"),
        Ok(list) => {
            let hits: Vec<_> = list.iter().filter(|e| e.keycode == c.keycode as i64 && e.mods & MASK == c.cg).collect();
            for e in list.iter().filter(|_| dump) {
                println!("  id {:>3} enabled={:<5} ascii={:<5} keycode={:<5} mods={:#x}  {}", e.id, e.enabled, e.ascii, e.keycode, e.mods, e.name);
            }
            if hits.is_empty() { println!("  no entry in plist matches"); }
            for e in hits {
                println!("  MATCH id {} ({}) enabled={} params=[{}, {}, {}]", e.id, e.name, e.enabled, e.ascii, e.keycode, e.mods);
            }
        }
    }

    println!("\n(c) CopySymbolicHotKeys (live, effective; includes OS defaults):");
    match live_entries() {
        Err(e) => println!("  error: {e}"),
        Ok(list) => {
            for (code, mods, en) in list.iter().filter(|_| dump) { println!("  code={code} mods={mods:#x} enabled={en}"); }
            let hits: Vec<_> = list.iter().filter(|(k, m, _)| *k == c.keycode as i64 && m & CARBON_MASK == c.carbon as u64).collect();
            if hits.is_empty() { println!("  no entry matches ({} total entries)", list.len()); }
            for (code, mods, en) in hits { println!("  MATCH code={code} mods={mods:#x} enabled={en}"); }
        }
    }

    println!("\n(b) RegisterEventHotKey probe:");
    let (st, r1) = chord::register(&c, 1);
    println!("  first  registration -> OSStatus {st} {}", status_name(st));
    if args.iter().any(|a| a == "--twice") {
        let (st2, _) = chord::register(&c, 2);
        println!("  second registration (same process) -> OSStatus {st2} {}", status_name(st2));
    }
    if let Some(secs) = args.iter().position(|a| a == "--hold").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()) {
        println!("  holding registration for {secs}s (start a second instance now)");
        chord::run_loop(Some(secs), || {});
    }
    if !r1.is_null() { unsafe { UnregisterEventHotKey(r1) }; }
}

fn status_name(s: OSStatus) -> &'static str {
    match s { noErr => "(noErr)", eventHotKeyExistsErr => "(eventHotKeyExistsErr)", eventHotKeyInvalidErr => "(eventHotKeyInvalidErr)", _ => "" }
}
