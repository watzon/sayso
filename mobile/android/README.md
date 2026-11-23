# Sayso Mobile for Android

The Sayso keyboard (an input method) and its companion app, in one APK with the
package id `app.sayso.mobile`.

- `app/src/main/kotlin/app/sayso/mobile/ime/`: the keyboard. `SaysoImeService`
  is the `InputMethodService`; `KeyboardView`, `SuggestionStripView`,
  `DiagnosticsPanelView` and `EmojiPanelView` are plain Views drawn on a Canvas.
- `keyboard/`: logic with no Android UI, covered by unit tests: layouts, the
  shift state machine, the EditorInfo mapping, and `EditorSession`, which
  applies engine edits to the editor.
- `engine/`: the `TypingEngine` interface and `FakeTypingEngine`. The native
  engine, `NativeTypingEngine` with its JNI bindings `SaysoCore`, is in
  `app/src/native/kotlin/`; the JNI glue is `app/src/main/cpp/sayso_jni.c`.
- `ui/` and `MainActivity`: the companion app in Jetpack Compose (setup,
  personal dictionary and replacements, settings, and the acknowledgements
  for the typing model's data and the speech engine).
- `settings/`: settings the app writes and the keyboard reads when it opens
  (`KeyboardSettings`, `VoiceSettings`).
- `voice/`: dictation in the keyboard (see [Voice](#voice)).
- `asr/`: the speech model catalog and manager, and local speech recognition
  in the `:asr` process.

## Build

Needs Go, JDK 21, and the Android SDK (platform 37.1) with NDK 27.1.12297006
and CMake 3.22.1. One script builds everything from a clean checkout:

```sh
mobile/scripts/build-android.sh
```

It builds `libsayso_core.so` for arm64-v8a and x86_64, checks
`mobile/models/en-US.sslm` against the sha256 in
`mobile/tools/sslmgen/DATA_SOURCES.md`, builds the core and the JNI glue for
this computer so the JVM tests run the real engine, downloads the sherpa-onnx
Android AAR and checks it against its pinned sha256
(`mobile/scripts/fetch-sherpa-onnx.sh`), writes `local.properties` if it is
missing, runs `./gradlew testDebugUnitTest assembleDebug` with the native
engine, checks the APK (both ABIs and no other, the libraries, the model
stored uncompressed, no speech model weights), and prints its path and size.
The APK is `app/build/outputs/apk/debug/app-debug.apk`.

By hand, from `mobile/android` (point Gradle at the SDK with `ANDROID_HOME` or
a `local.properties` file holding `sdk.dir=...`):

```sh
../scripts/build-core-android.sh   # once, and after core changes
./gradlew testDebugUnitTest assembleDebug
```

The engine is picked at build time:

- **Native** (the default when `mobile/build/jniLibs/<abi>/libsayso_core.so`
  exists for both ABIs): `src/native/kotlin`, the CMake build of `sayso_jni`,
  both libraries for arm64-v8a and x86_64, and `mobile/models/` as assets.
- **Fake** (the default otherwise; needs no NDK): `src/fake/kotlin`, the
  Kotlin `FakeTypingEngine`, no native code and no model.

`-PsaysoNative=true` or `false` forces one.

The local speech engine is in the build when the sherpa-onnx AAR is at
`mobile/build/sherpa-onnx/sherpa-onnx-2.0.3.aar` (`fetch-sherpa-onnx.sh` puts
it there; `-PsaysoSherpaAar=<file>` points elsewhere): `src/sherpa/kotlin`.
Without it, `src/nosherpa/kotlin` has no local engine, and voice uses the
system recognizer or the cloud. `-PsaysoCoreDir=<dir>` reads the
core from `<dir>/<abi>/libsayso_core.so` instead. `-PsaysoHostLibDir=<dir>`
(set by the script) points the JVM tests at a host build of the core and the
JNI glue; without it, `NativeTypingEngineTest` is skipped.

`SherpaLocalAsrTest` decodes real speech with the app's engine and the
catalog's models on this computer. It is skipped unless
`-PsaysoAsrTestDir=<dir>` points at a folder with `lib/` (the sherpa-onnx JNI
library for this OS from the same release, for example
`sherpa-onnx-native-lib-osx-aarch64-2.0.3.jar` unpacked), `models/<model id>/`
(the catalog's files), and `0.wav` (LibriSpeech 1089-134686-0000, from the
`test_wavs` of the model repositories). On macOS also set
`DYLD_LIBRARY_PATH=<dir>/lib`.

## Install and turn on the keyboard

```sh
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

Open **Sayso** on the phone and follow the setup tab:

1. **Open keyboard settings** and turn on Sayso Keyboard.
2. **Choose keyboard** and pick Sayso Keyboard.
3. Type in the playground field.

Long-press the space bar to switch keyboards later.

## Keyboard gestures

| Gesture | Result |
|---|---|
| Tap shift | Next letter in capitals |
| Double-tap shift | Caps Lock; tap again to turn it off |
| Hold a letter or the period | Accents or punctuation; slide to pick, lift to type |
| Hold backspace | Repeats |
| Swipe left from backspace | Deletes a word, one more for each further swipe step |
| Drag along the space bar | Moves the cursor |
| Backspace right after an autocorrect | Restores the typed word |

## The engine boundary

The keyboard UI makes no typing decisions. It hands the raw position of each
letter touch to `TypingEngine.onTouch` and types the key the engine returns.
Space and punctuation go to `onSpaceOrPunct`, which returns a `CommitAction`
(delete N characters before the cursor, then insert text); `EditorSession`
applies it. Suggestions, autocorrect, revert, the personal dictionary, and the
diagnostics overlay all go through the same interface. See
`app/src/main/kotlin/app/sayso/mobile/engine/TypingEngine.kt`.

`SaysoEngine.get(context)` holds one engine for the process, shared by the
keyboard and the companion app. It calls `createTypingEngine(context)`, which
is defined once per engine: `src/fake/kotlin/.../CreateTypingEngine.kt`
returns `FakeTypingEngine` (nearest key with a learned offset, a short English
word list), `src/native/kotlin/.../CreateTypingEngine.kt` returns
`NativeTypingEngine.create(context)`.

### The native engine

`NativeTypingEngine` maps the `TypingEngine` calls onto the C ABI of
`mobile/core/capi` (see `API.md` there) through `SaysoCore`, whose `external`
functions are implemented in `sayso_jni.c`:

- **Model.** `en-US.sslm` is an APK asset stored uncompressed
  (`androidResources.noCompress`), opened with `AssetFileDescriptor` and
  `sayso_engine_create_fd`, so the core maps it in place. If that fails, it is
  copied to `noBackupFilesDir` once and opened from there.
- **Learning** goes to `filesDir/typing.journal`. Private fields (passwords,
  incognito, no-personalized-learning) set `SAYSO_EDITOR_PRIVACY`: nothing is
  learned or written. Dictionary edits from the companion app lift it for the
  edit, so they are kept even if the last field was private.
- **No allocation per touch.** `onTouch` passes primitives both ways. The other
  calls fill arrays the engine owns and reuses; only the returned Strings and
  lists are new.
- **Edits** use UTF-16 lengths (`delete_utf16`), as `deleteSurroundingText`
  counts.
- **Settings** reach the core through `sayso_set_preferences` (autocorrect
  off, normal, strict; next-word prediction) each time the keyboard opens.
  Reset learned data clears the journal and adds back the personal words and
  replacements the user added; words the core learned by itself go.
- **Diagnostics** show each key's expansion for the next touch and its learned
  offset, and each candidate's total, spatial, and language-model scores and
  edits. The margin is measured from the autocorrect bar (threshold 0).

The unit tests in `FakeTypingEngineTest` describe the behavior the keyboard
expects from any engine; `NativeTypingEngineTest` runs the native engine
through the real JNI glue on the JVM.

## Voice

The mic at the right of the suggestion strip starts a dictation. The keys
give way to the voice bar: the engine and the style, the time, a waveform of
the microphone level, the live text, Cancel (×), Hands-free, and Done (✓).
Done inserts the text; an Undo chip in the strip then removes exactly that
text, while it is still before the cursor. With Hands-free on, Done also sends
with the field's action (send, search, go) and starts the next dictation.

```
mic tap ─▶ VoiceController ── planEngines(choice, availability, field) ─▶ first engine
             │
             ├─ A local:  AudioRecord 16 kHz mono ─▶ AsrClient ═Messenger═▶ AsrService (:asr)
             │                                                     sherpa-onnx: streaming model → live text
             │                                                                  Parakeet       → final text
             ├─ B system: SpeechRecognizer (on-device when available; records itself)
             └─ C cloud:  AudioRecord ─▶ VoiceCore.transcribeCloud (background)
                          (also streams to :asr when a local model is active: live text and fallback)
Done ─▶ final text ─▶ VoiceCore.postProcess(raw, before, after)
                  ─▶ VoiceCore.enhance (background, 3 s, only for a style with a prompt and cloud Styles on)
                  ─▶ insert through EditorSession ─▶ Undo chip ─▶ VoiceCore.recordHistory
```

- **Microphone.** A visible keyboard shares the foreground app's microphone
  access, so it records with `AudioRecord` once `RECORD_AUDIO` is granted;
  no foreground service. Without the permission the mic opens the companion
  app, which asks for it at once (`MainActivity.EXTRA_REQUEST_MIC`). The
  keyboard holds transient exclusive audio focus while recording, so music
  pauses and resumes, and cancels the dictation when it hides or the field
  changes.
- **Engines.** `planEngines` (in `voice/VoicePolicy.kt`) orders them: Auto is
  the local model when one is active, then the system recognizer; System puts
  the system recognizer first; Cloud puts the cloud first when it is
  configured. The system recognizer is everyone's last resort, so voice works
  with nothing downloaded.
- **The `:asr` process.** `AsrService` runs sherpa-onnx in
  `android:process=":asr"`, so a native crash or an out-of-memory kill there
  never takes the keyboard down. `AsrClient` binds on the first dictation,
  keeps the models loaded for a minute after the last one, and reports a dead
  process once; the keyboard then says so and hands the microphone to the
  system recognizer, and skips the local engine for five minutes.
- **Sessions.** `VoiceMachine` numbers each dictation. Results for an older
  number (a late partial, a final after Cancel, a style result after a new
  start) are dropped, so they never reach the editor.
- **Private fields.** Password, incognito (`IME_FLAG_NO_PERSONALIZED_LEARNING`)
  and other private fields never use cloud transcription or cloud Styles, and
  write no history (`VoicePolicy`). The system recognizer is asked to stay
  offline there (`EXTRA_PREFER_OFFLINE`).
- **Models.** `ModelCatalog` lists the downloadable models; none is in the
  APK. `ModelManager` downloads one file at a time into `noBackupFilesDir`,
  resumes with HTTP Range, checks each file's size and sha256 before it gets
  its name, deletes, and sets the active model (`VoiceSettings.finalModelId`
  for a batch model, `liveModelId` for a streaming one). Its `download` blocks
  and reports progress, for the companion app to run on a background thread.
  `ramAdvisory` warns when the device has less RAM than a model wants.

| Model | Role | Download | sha256 (per file in `ModelCatalog.kt`) | License |
|---|---|---|---|---|
| Parakeet TDT 0.6B v3 int8 | Final text, 25 European languages | 670.6 MB | encoder `1afbe69c…2c479` | CC BY 4.0 |
| Streaming Zipformer English 2023-06-26 | Live text; final text when it is the only model | 73.4 MB | encoder `563fde43…45ac1` | Apache-2.0 |

Parakeet wants a phone with 6 GB of RAM: loading it and decoding a 17 s clip
peaked at about 2 GB on a Mac (sherpa-onnx 2.0.3, int8, 4 threads; not yet
measured on a phone). Below 6 GB the app should offer the streaming model
alone.

## Developer Diagnostics

Turn on **Developer diagnostics** in the Settings tab. The keyboard then
outlines each letter's effective hit area, draws the learned touch offset as
an arrow from the key center, and shows the candidate scores and the
autocorrect margin above the suggestion strip. With the native engine a
candidate reads `text total=spatial+lm eEdits` (lower is better, in nats).
