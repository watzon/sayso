# sayso-capture

Capture the audio of this computer for a [Sayso](https://github.com/watzon/sayso) speech engine: 16 kHz mono frames of 20 ms, the format of the engines.

- **The microphone** (`mic`): the input devices, and a capture of one device with `cpal`. The audio callback converts, mixes to mono, resamples, and cuts the frames, and a worker thread gives each frame to your function. A slow function delays frames but never drops them.
- **The system audio** (the sound of other apps and calls) comes later. See "System audio" in the SDK plan.

## Example

```rust,no_run
let capture = sayso_capture::mic::start(None, Box::new(|frame| {
    println!("{} samples, level {:.2}", frame.samples.len(), frame.level);
}))?;
std::thread::sleep(std::time::Duration::from_secs(5));
drop(capture); // Stops the capture. The last frames reach the function first.
# Ok::<(), sayso_capture::mic::Error>(())
```

On macOS, an app that captures the microphone needs `NSMicrophoneUsageDescription` in its `Info.plist`. On some systems a denied microphone gives silence instead of an error, so check the permission of the system first. On Linux, the build needs the ALSA headers (`libasound2-dev`).

## Documentation

- [The SDK plan](https://github.com/watzon/sayso/blob/main/docs/sdk.md)

## License

MIT. See `LICENSE`.
