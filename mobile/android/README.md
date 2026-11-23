# Sayso Mobile for Android

The Sayso keyboard (an input method) and its companion app, in one APK with the
package id `app.sayso.mobile`.

- `app/src/main/kotlin/app/sayso/mobile/ime/`: the keyboard. `SaysoImeService`
  is the `InputMethodService`; `KeyboardView`, `SuggestionStripView`,
  `DiagnosticsPanelView` and `EmojiPanelView` are plain Views drawn on a Canvas.
- `keyboard/`: logic with no Android UI, covered by unit tests: layouts, the
  shift state machine, the EditorInfo mapping, and `EditorSession`, which
  applies engine edits to the editor.
- `engine/`: the `TypingEngine` interface and `FakeTypingEngine`.
- `ui/` and `MainActivity`: the companion app in Jetpack Compose (setup,
  personal dictionary and replacements, settings).
- `settings/`: settings the app writes and the keyboard reads when it opens.

## Build

Needs JDK 21 and the Android SDK (platform 37.1). Point Gradle at the SDK with
`ANDROID_HOME` or a `local.properties` file holding `sdk.dir=...`.

```sh
cd mobile/android
./gradlew testDebugUnitTest assembleDebug
```

The APK is `app/build/outputs/apk/debug/app-debug.apk`.

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
is defined once per engine:

- `app/src/fake/kotlin/.../CreateTypingEngine.kt` returns `FakeTypingEngine`
  (nearest key with a learned offset, a short English word list). This is the
  default build.
- `app/src/native/kotlin/.../CreateTypingEngine.kt` returns
  `NativeTypingEngine`, which does not exist yet.

### Plugging in the native core

1. Build `libsayso_core.so` from `mobile/core` for `arm64-v8a` and `x86_64`,
   into `<dir>/<abi>/libsayso_core.so`.
2. Add `NativeTypingEngine.kt` (implements `TypingEngine`, declares `external`
   functions, loads `sayso_jni`) to `app/src/native/kotlin/app/sayso/mobile/engine/`.
3. Fill in `app/src/main/cpp/sayso_jni.c` with the JNI functions that call the
   C ABI. `app/src/main/cpp/CMakeLists.txt` already links it against
   `libsayso_core.so`.
4. Build with the flag:

   ```sh
   ./gradlew assembleDebug -PsaysoNative=true -PsaysoCoreDir=/path/to/dir
   ```

   `saysoCoreDir` defaults to `mobile/core/build/android`. The flag swaps the
   source folder, turns on the CMake build, and packages `libsayso_core.so`.
   It needs CMake 3.22.1 or later from the SDK manager. The unit tests
   in `FakeTypingEngineTest` describe the behavior the keyboard expects from
   any engine.

## Developer Diagnostics

Turn on **Developer diagnostics** in the Settings tab. The keyboard then
outlines each letter's effective hit area, draws the learned touch offset as
an arrow from the key center, and shows the candidate scores and the
autocorrect margin above the suggestion strip.
