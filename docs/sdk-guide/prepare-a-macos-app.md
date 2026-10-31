# How to prepare a macOS app

In local mode, the permissions belong to your app and not to Sayso. This page lists what your app needs on macOS to use the microphone and the system audio, and to start on every supported version.

This page applies to local mode and to `Mode::Auto` when Sayso does not run. In Sayso mode, Sayso captures the audio, and the system asks the user about Sayso.

## Add the usage texts to Info.plist

Your app needs two keys in its `Info.plist`. Write the texts for your own app, and say why you record:

```xml
<key>NSMicrophoneUsageDescription</key>
<string>Notetaker records your voice to write the notes of a meeting.</string>
<key>NSAudioCaptureUsageDescription</key>
<string>Notetaker records the sound of a call, so it can write down what the others say.</string>
```

- `NSMicrophoneUsageDescription` is for the source `mic`.
- `NSAudioCaptureUsageDescription` is for the source `system`. Without it, the system does not ask in the normal way, and macOS can end a program that asks.

The keys work in an app bundle. A program that is not in a bundle gets the question for the system audio from `AudioDeviceStart`, which blocks until a person answers. Test the capture in a bundled app.

If your app uses the hardened runtime, it also needs the entitlement `com.apple.security.device.audio-input` for the microphone. Sayso has it in `packaging/Sayso.entitlements`.

## Link CoreAudio weakly

The audio library `cpal` imports the functions of the process tap that came with macOS 14.2. A normal link makes these imports strong, and the program does not start on macOS 14.0 and 14.1. Add this `build.rs` next to `Cargo.toml` of your app:

<!-- check: unchecked -->
```rust
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-bins=-Wl,-weak_framework,CoreAudio");
    }
}
```

Every app that uses `sayso-sdk` needs this, also an app that never uses the source `system`. The SDK cannot set the flag for you, because a library cannot set a link flag for the program of another package.

Check the result after a build. Replace `scribe` with the name of your program:

```sh
nm -m target/debug/scribe | grep ProcessTap
```

With the flag, both lines say `weak external`:

```text
(undefined) weak external _AudioHardwareCreateProcessTap (from CoreAudio)
(undefined) weak external _AudioHardwareDestroyProcessTap (from CoreAudio)
```

Without the flag, they say only `external`. We ran both builds of the tutorial program to see this.

On macOS 14.0 and 14.1, the source `system` returns the error code `unsupported`.

## Ask for the permissions at the right time

- `Speech::features().system_audio` tells you, without a question, if the source `system` can start. It is true when the system supports the capture and the user did not refuse your app. Your app can still get `permission` at the start.
- A first `start_session` with `Source::System` can wait up to 120 seconds for the answer of the user. Call it on a thread that can wait. The microphone of the session opens after the answer, and a refusal opens no microphone and leaves no recording.
- If the user refuses, `start_session` returns the error code `permission`. If the user takes the permission back during a session, that source ends with `source_ended { reason: "permission" }`. The other sources go on.
- A refused capture gets no error from Core Audio and only silence, so the SDK reads the permission state itself. We have not seen a real refusal or a real take-back on macOS yet, so test these with a real person before you rely on them.

## Know what the capture leaves out

In local mode on macOS, `excludes` is `["app"]`: the capture leaves out the sound of your own app. If the process has played no sound yet, it has no Core Audio process object, and the SDK adds it when it appears. In Sayso mode, `excludes` is `["sayso", "app"]`.
