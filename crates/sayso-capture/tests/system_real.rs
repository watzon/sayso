//! Checks of the system audio on a real computer. Each one is ignored, because
//! it plays a sound or needs a person. Run one with:
//!
//!     cargo test -p sayso-capture --test system_real -- --ignored --nocapture <name>
//!
//! Keep the volume low: the sound goes to the speakers.
//!
//! On a Mac the first capture of a program can wait for the answer of a person
//! to a question of the system, and the capture does not end until then, so
//! `finds_a_quiet_tone` looks like it hangs. Look at `permission_state` first.

use sayso_capture::mic::{FRAME_SAMPLES, TARGET_RATE};
use sayso_capture::system::{self, End, Event, Exclude, Options};
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const TONE_HZ: f32 = 997.0;

/// Write one second of the tone as a WAV file of 48 kHz.
fn write_tone(path: &Path) {
    const RATE: u32 = 48_000;
    let samples: Vec<i16> = (0..RATE).map(|n| ((2.0 * std::f32::consts::PI * TONE_HZ * n as f32 / RATE as f32).sin() * 0.5 * f32::from(i16::MAX)) as i16).collect();
    let bytes = samples.len() as u32 * 2;
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(b"RIFF").unwrap();
    file.write_all(&(36 + bytes).to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&RATE.to_le_bytes()).unwrap();
    file.write_all(&(RATE * 2).to_le_bytes()).unwrap();
    file.write_all(&2u16.to_le_bytes()).unwrap();
    file.write_all(&16u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&bytes.to_le_bytes()).unwrap();
    for sample in samples {
        file.write_all(&sample.to_le_bytes()).unwrap();
    }
}

/// Play `path` quietly.
fn play(path: &Path) -> Child {
    #[cfg(target_os = "macos")]
    let child = Command::new("afplay").arg("-v").arg("0.03").arg(path).spawn();
    #[cfg(target_os = "linux")]
    // This volume scale is cubic: 16000 of 65536 is about -36 dB. Much less rounds to nothing on a 16-bit sink.
    let child = Command::new("paplay").arg("--volume=16000").arg(path).spawn();
    #[cfg(windows)]
    let child = Command::new("powershell").arg("-c").arg(format!("(New-Object Media.SoundPlayer '{}').PlaySync()", path.display())).spawn();
    child.expect("the player starts")
}

/// The strength of the tone in a frame, from 0 to about the amplitude.
fn tone_level(samples: &[f32]) -> f32 {
    let w = 2.0 * std::f32::consts::PI * TONE_HZ / TARGET_RATE as f32;
    let (mut re, mut im) = (0.0, 0.0);
    for (n, sample) in samples.iter().enumerate() {
        re += sample * (w * n as f32).cos();
        im += sample * (w * n as f32).sin();
    }
    2.0 * (re * re + im * im).sqrt() / samples.len() as f32
}

fn events() -> (Box<dyn FnMut(Event) + Send>, mpsc::Receiver<Event>) {
    let (tx, rx) = mpsc::channel();
    (Box::new(move |event| drop(tx.send(event))), rx)
}

/// Say what the system answers about the permission. It never asks the user.
#[test]
#[ignore = "only prints the state of this computer"]
fn permission_state() {
    println!("the permission of the system audio: {:?}", system::access());
}

/// Capture while a quiet tone plays: the frames have the right format, the
/// times go on, and the tone is in them.
///
/// The first run on a Mac can show a question of the system for the program
/// that started the test (the terminal). A person answers it. After a refusal
/// the buffers are silent, and the test says so. On Linux it needs `paplay`.
/// The first sound after the start of a new PulseAudio server was silent in the
/// monitor, also for `parec`: play a sound once before the test on such a server.
#[test]
#[ignore = "plays a quiet tone, and can show a question of the system"]
fn finds_a_quiet_tone() {
    let dir = std::env::temp_dir().join(format!("sayso-system-real-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tone = dir.join("tone.wav");
    write_tone(&tone);

    let (on_event, rx) = events();
    let capture = system::start(Options { exclude: vec![Exclude { name: "app".into(), pid: std::process::id() }] }, on_event).expect("the capture starts");
    println!("excludes: {:?}", capture.excludes());
    std::thread::sleep(Duration::from_millis(300));
    let mut player = play(&tone);
    let started = Instant::now();
    let (mut frames, mut non_zero, mut strongest, mut other_size, mut late) = (0usize, 0usize, 0.0f32, 0usize, 0usize);
    let mut last_time = None;
    while started.elapsed() < Duration::from_secs(3) {
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Event::Frame(frame)) => {
                frames += 1;
                other_size += usize::from(frame.samples.len() != FRAME_SAMPLES);
                non_zero += usize::from(frame.samples.iter().any(|s| *s != 0.0));
                strongest = strongest.max(tone_level(&frame.samples));
                late += usize::from(last_time.is_some_and(|last| frame.time < last));
                last_time = Some(frame.time);
            }
            Ok(other) => println!("event: {other:?}"),
            Err(_) => break,
        }
    }
    let _ = player.wait();
    drop(capture);
    let _ = std::fs::remove_dir_all(&dir);
    println!("{frames} frames in {:?}, {non_zero} not silent, the strongest tone {strongest:.5}", started.elapsed());
    assert!((120..=180).contains(&frames), "3 s is about 150 frames of 20 ms, and {frames} came");
    assert_eq!(other_size, 0, "each frame has {FRAME_SAMPLES} samples");
    assert_eq!(late, 0, "the times go up");
    assert!(non_zero > 0, "every frame is silent: on macOS, the permission is refused or has no answer yet. See System Settings > Privacy & Security > Screen & System Audio Recording");
    assert!(strongest > 0.001, "the tone is not in the frames: {strongest}");
}

/// A manual check of a change of the output device (also on the VMs).
///
/// Start it, and within 60 s change the default output device in the settings
/// of the system (on Linux: `pactl set-default-sink <name>`). It passes when
/// `Event::Gap` comes and frames go on after it.
#[test]
#[ignore = "needs a person to change the output device"]
fn manual_a_change_of_the_output_device_gives_a_gap() {
    let (on_event, rx) = events();
    let _capture = system::start(Options::default(), on_event).expect("the capture starts");
    println!("change the default output device now");
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut gap = None;
    let mut frames_after = 0;
    while Instant::now() < deadline && frames_after < 50 {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::Gap { ms }) => {
                println!("gap of {ms} ms");
                gap = Some(ms);
            }
            Ok(Event::Frame(_)) if gap.is_some() => frames_after += 1,
            Ok(Event::Ended(end)) => panic!("the capture ended: {end:?}"),
            _ => {}
        }
    }
    assert!(gap.is_some(), "no gap in 60 s");
    assert_eq!(frames_after, 50, "the frames did not go on after the gap");
}

/// A manual check of a permission that the user takes back (macOS).
///
/// Give the permission to the program that runs the test. Start it, and within
/// 120 s turn the program off in System Settings > Privacy & Security > Screen
/// & System Audio Recording. It passes when `Event::Ended(End::Permission)`
/// comes. It fails when TCC does not report it, and then the plan needs a change.
#[test]
#[ignore = "needs a person to take the permission back"]
fn manual_a_permission_taken_back_ends_the_capture() {
    let (on_event, rx) = events();
    let _capture = system::start(Options::default(), on_event).expect("the capture starts");
    println!("take the permission back now");
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if let Ok(Event::Ended(end)) = rx.recv_timeout(Duration::from_secs(1)) {
            assert_eq!(end, End::Permission);
            return;
        }
    }
    panic!("the capture did not end in 120 s");
}
