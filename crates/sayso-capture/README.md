# sayso-capture

Capture the audio of this computer for a [Sayso](https://github.com/watzon/sayso) speech engine: 16 kHz mono frames of 20 ms, the format of the engines.

- **The microphone** (`mic`): the input devices, and a capture of one device with `cpal`. The audio callback converts, mixes to mono, resamples, and cuts the frames, and a worker thread gives each frame to your function. A slow function delays frames but never drops them.
- **The system audio** (`system`): the sound of other apps and calls. macOS 14.2 and later with a Core Audio process tap, Windows with WASAPI loopback of the default output device, and Linux with the monitor of the default sink (PulseAudio, or PipeWire through `pipewire-pulse`). It follows a change of the device, with an `Event::Gap`.
- **The clock** (`clock`): every frame of both captures has a time on one clock, so frames of the two can be compared.
- **Live sessions** (`live`): `LiveMicrophone` and `LiveSystemAudio` are the capture traits of `sayso-speech`.

## Example

```rust,no_run
let capture = sayso_capture::mic::start(None, Box::new(|frame| {
    println!("{} samples, level {:.2}", frame.samples.len(), frame.level);
}))?;
std::thread::sleep(std::time::Duration::from_secs(5));
drop(capture); // Stops the capture. The last frames reach the function first.
# Ok::<(), sayso_capture::mic::Error>(())
```

The system audio:

```rust,no_run
use sayso_capture::system::{self, Event, Exclude, Options};

let options = Options { exclude: vec![Exclude { name: "app".into(), pid: std::process::id() }] };
let capture = system::start(options, Box::new(|event| match event {
    Event::Frame(frame) => println!("{} samples at {:?}", frame.samples.len(), frame.time),
    Event::Gap { ms } => println!("the device changed: {ms} ms lost"),
    Event::Ended(end) => println!("the capture ended: {end:?}"),
}))?;
println!("left out: {:?}", capture.excludes());
std::thread::sleep(std::time::Duration::from_secs(5));
drop(capture);
# Ok::<(), sayso_capture::system::Error>(())
```

`system::start` can block until a person answers the question of the system on macOS, so call it from a thread that can wait. `system::access()` reads the permission and never asks. A refused capture on macOS gets no error from the system: its frames are silent.

On macOS, an app that captures the microphone needs `NSMicrophoneUsageDescription` in its `Info.plist`. On some systems a denied microphone gives silence instead of an error, so check the permission of the system first. An app that captures the system audio needs `NSAudioCaptureUsageDescription`. To also launch on macOS 14.0 and 14.1, link CoreAudio weakly (`-Wl,-weak_framework,CoreAudio`), because a strong import of the tap functions stops the launch there. On Linux, the build needs the ALSA headers (`libasound2-dev`).

## Documentation

- [The SDK plan](https://github.com/watzon/sayso/blob/main/docs/sdk.md)

## License

MIT. See `LICENSE`.
