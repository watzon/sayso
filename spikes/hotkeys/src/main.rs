//! Throwaway spike: macOS hotkey approaches for Sayso. See RESULT.md.
#![allow(non_upper_case_globals)] // Apple constant names kept verbatim
mod chord;
mod conflicts;
mod ffi;
mod keys;
mod synth;
mod tap;

use std::{sync::OnceLock, time::{Instant, SystemTime, UNIX_EPOCH}};

static START: OnceLock<Instant> = OnceLock::new();

/// "wall-clock ms.mmm (+elapsed)" timestamp for event lines.
pub fn now() -> String {
    let el = START.get_or_init(Instant::now).elapsed();
    let wall = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    format!("{}.{:03} +{:.3}s", wall.as_secs() % 100_000, wall.subsec_millis(), el.as_secs_f64())
}

/// PID of the process that turned on Secure Event Input, from the session dictionary.
fn secure_input_owner() -> Option<(i64, String)> {
    use core_foundation::{base::{CFType, TCFType}, dictionary::CFDictionary, number::CFNumber, string::CFString};
    let raw = unsafe { ffi::CGSessionCopyCurrentDictionary() };
    if raw.is_null() { return None; }
    let d: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_create_rule(raw as _) };
    let pid = d.find(CFString::new("kCGSSessionSecureInputPID"))?.downcast::<CFNumber>()?.to_i64()?;
    let out = std::process::Command::new("ps").args(["-p", &pid.to_string(), "-o", "comm="]).output().ok()?;
    Some((pid, String::from_utf8_lossy(&out.stdout).trim().to_string()))
}

fn perms(args: &[String]) {
    use ffi::*;
    // `perms --secure N` turns Secure Event Input on for N seconds, so a second `perms` can observe it.
    let secure_secs: Option<f64> = args.iter().position(|a| a == "--secure").and_then(|i| args.get(i + 1)?.parse().ok());
    if secure_secs.is_some() { println!("EnableSecureEventInput -> {}", unsafe { EnableSecureEventInput() }); }
    unsafe {
        println!("CGPreflightListenEventAccess (Input Monitoring) = {}", CGPreflightListenEventAccess());
        println!("CGPreflightPostEventAccess   (post events)      = {}", CGPreflightPostEventAccess());
        println!("AXIsProcessTrusted           (Accessibility)    = {}", AXIsProcessTrusted());
        println!("IsSecureEventInputEnabled    (Secure Input)     = {}", IsSecureEventInputEnabled());
    }
    match secure_input_owner() {
        Some((pid, name)) => println!("Secure Input is held by pid {pid} ({name})"),
        None => println!("Secure Input owner: none"),
    }
    if let Some(s) = secure_secs {
        chord::run_loop(Some(s), || {});
        unsafe { DisableSecureEventInput() };
    }
}

fn main() {
    START.get_or_init(Instant::now);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    match args.first().map(String::as_str) {
        Some("chord") => chord::run(rest),
        Some("chord-gh") => chord::run_gh(rest),
        Some("tap") => tap::run(rest),
        Some("handy") => tap::run_handy(rest),
        Some("perms") => perms(rest),
        Some("conflicts") => conflicts::run(rest),
        Some("synth") => synth::run(rest),
        _ => println!("usage: hotkeys-spike <chord [chord] [secs] | chord-gh [secs] | tap [secs] [--single-esc] [--recording] | handy [secs] | perms [--secure N] | conflicts <chord> [--hold N] [--twice] [--dump] | synth <name>>"),
    }
}
