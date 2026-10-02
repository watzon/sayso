# Engine spike results (S3 + S4)

Machine: macOS 27.0, Apple Silicon M5 Pro, Xcode 27, Swift 6.4 (package built in Swift 5 language mode, see "Gotchas").
Date: 2026-10-01. Throwaway code. Nothing was uploaded or notarized. No TCC or System Settings changes.

## Layout

| Path | What |
|---|---|
| `Engine/` | SwiftPM package, executable target `SaysoEngine` (`Engine/Sources/SaysoEngine/main.swift`, about 250 lines) |
| `drive.py` | Python NDJSON driver. Spawns the binary, runs the scenarios, writes `runs/<label>.jsonl` (all events with timestamps) |
| `audio/short.wav`, `audio/long30.wav` | 16 kHz mono 16-bit WAV, 8.15 s and 29.23 s (made with `say` + `afconvert -f WAVE -d LEI16@16000 -c 1`; long one is one longer script, not a concatenation) |
| `.models/` | Custom model directory (about 1.87 GB for all three engines) |
| `app/`, `sign.sh`, `notarize.sh` | S4 bundle, signing script, notarization script (never run) |
| `runs/` | Raw event logs (`cold`, `warm`, `signed`, `signed2`) and `signing.txt` |

## S3. Engine build

### Versions (exact pins)

- `argmax-oss-swift` 1.1.0 (`exact:`), product `WhisperKit`. Package also exports `ArgmaxOSS`, `TTSKit`, `SpeakerKit`.
- `FluidAudio` 0.17.5 (`exact:`), product `FluidAudio`. Pulls a binary target `NemoTextProcessing` (xcframework v0.3.1 from GitHub releases) and `swift-argument-parser` 1.8.2 (through argmax).
- Platform `.macOS(.v14)`. Tools version 6.0.
- **No build conflict** between WhisperKit and FluidAudio. One resolve, one build (63 s clean release build). Result: one 21 MB arm64 binary that links only system libraries and `/usr/lib/swift` (checked with `otool -L`). No frameworks or dylibs to bundle.

### API names used (all read from source in `.build/checkouts`)

FluidAudio:
- `UnifiedAsrManager` (actor, batch). `init(configuration:config:encoderPrecision:)`, `loadModels(to:configuration:progressHandler:)`, `loadModels(from:)`, `transcribe(_ samples: [Float]) -> String`. Long audio uses 15 s windows with 2 s overlap.
- `StreamingUnifiedAsrManager` (actor, streaming). `init(config: UnifiedConfig)`, `loadModels(to:)`, `appendAudio(_ AVAudioPCMBuffer)` (resamples to 16 kHz itself), `processBufferedAudio()`, `setPartialTranscriptCallback { String in }`, `getPartialTranscript()`, `finish() -> String`, `reset()`, `consumeWordTimings()`.
- `UnifiedConfig(leftFrames:chunkFrames:rightFrames:)`. A frame is 80 ms. The 320 ms tier is `UnifiedConfig(leftFrames: 70, chunkFrames: 2, rightFrames: 2)`. The tier id `70_2_2` is confirmed: the HF repo has `parakeet_unified_encoder_streaming_70_2_2[_int8].mlmodelc`. Other tiers in the repo: `70_7_1`, `70_7_7`, `70_13_13` (default, 2.08 s). `UnifiedConfig.latencyMs` reports 320 for `70_2_2`.
- `UnifiedEncoderPrecision` (`.int8` default, `.fp16`). I used int8.
- `ModelHub.download(_ repo: Repo, to:, variant:, additionalModelNames:, progressHandler:)` with `Repo.parakeetUnified` (HF `FluidInference/parakeet-unified-en-0.6b-coreml`). Batch uses `variant: "offline"`. Streaming uses `variant: nil` plus `additionalModelNames: [ModelNames.ParakeetUnified.streamingEncoderFile(precision:contextSuffix:)]`. I call this first so download time and load time are separate.
- `ProgressHandler = (DownloadProgress) -> Void`. `DownloadProgress.fractionCompleted` and `.phase` (`.listing`, `.downloading(completedFiles:totalFiles:)`, `.compiling(modelName:)`).
- `AudioConverter().resampleAudioFile(path:) -> [Float]` (any file to 16 kHz mono Float32).

WhisperKit:
- `WhisperKit.download(variant:downloadBase:progressCallback:) -> URL` (model folder). Variant string is `openai_whisper-large-v3-v20240930_626MB` and resolves in repo `argmaxinc/whisperkit-coreml`.
- `WhisperKitConfig(downloadBase:modelFolder:verbose:logLevel:load:download:)`, `WhisperKit(config)` (async init).
- `whisper.transcribe(audioArray: [Float], decodeOptions: DecodingOptions(language: "en", detectLanguage: false)) -> [TranscriptionResult]`, `.text`.

### Custom model directory

| Library | Accepts custom dir? | How | Resulting layout under `.models/` |
|---|---|---|---|
| FluidAudio | Yes | `loadModels(to: URL)` or `ModelHub.download(_, to: URL)`. Default is `~/Library/Application Support/FluidAudio/Models`. Also `loadModels(from: URL)` to load an already-downloaded folder with no network. | `parakeet-unified-en-0.6b/` (repo folder name appended). Batch and streaming share it: shared decoder (13.8 MB), joint (3.3 MB), vocab; plus one encoder per mode. |
| WhisperKit | Yes | `downloadBase: URL` on `WhisperKit.download` and `WhisperKitConfig`. The tokenizer also goes to `downloadBase` (the `tokenizerFolder` default is `downloadBase`). Or `modelFolder: String` to point at a ready folder. Default is `~/Documents/huggingface`. | `models/argmaxinc/whisperkit-coreml/openai_whisper-large-v3-v20240930_626MB/` and `models/openai/whisper-large-v3/` (tokenizer) |

Verified: after the cold run `~/Library/Application Support/FluidAudio` does not exist and `~/Documents/huggingface` was not modified (mtime from earlier). The engine reads the dir from env `SAYSO_MODELS_DIR`, default `./.models`.

Two other disk locations that are NOT controllable by the model dir:
- **Core ML / ANE compile cache**: `~/Library/Caches/<id>/com.apple.e5rt.e5bundlecache`, managed by the OS. `<id>` is the executable name for a bare binary (`SaysoEngine`), and the **enclosing app bundle id** once the sidecar sits inside an app bundle (`dev.sayso.spike`; observed). So the first run inside the shipped app compiles again (see timings, "signed" row), then stays warm.
- **URLSession cache** for downloads: `~/Library/Caches/<id>/Cache.db` (4 MB after the downloads).

### Timings (this machine)

"Cold" = first ever load of that model for this process identity, includes download and ANE compile. "Warm" = second process launch, same bare binary. Download speed depends on my network (roughly 35 MB/s to 40 MB/s).

| Engine / model | Size on disk | Download (cold) | Cold load (ANE compile) | Warm load | RSS after load |
|---|---|---|---|---|---|
| Parakeet Unified batch (int8 offline encoder + decoder + joint) | 578 MB encoder + 17 MB shared, about 595 MB | 17.3 s | 10.9 s | 0.13 s | 122 MB cold, 48 MB warm |
| Parakeet Unified streaming `70_2_2` int8 | 578 MB encoder (shared files already present) | 8.4 s | 7.3 s | 0.08 s | 139 MB cold, 76 MB warm |
| WhisperKit `openai_whisper-large-v3-v20240930_626MB` | 609 MB (incl. tokenizer, 638 MB measured delta) | 16.0 s | **62.3 s** | 1.0 s | 155 MB cold, 116 MB warm |

Warm rows exclude about 1 s (FluidAudio) to 2.3 s (WhisperKit) of "download" time that is only a network tree-listing call when files are already present. The engine should skip the download call when the folder is complete (`loadModels(to:)` on its own does a local completeness check; `WhisperKit(modelFolder:)` skips the hub). Process start to first output line: 16 ms.

Caveat: I cannot prove the ANE cache had no earlier entry for these exact models. The cache is per executable id (above), and `~/Library/Caches/SaysoEngine` was created by my first run, so the cold numbers are real for a first install.

Transcription time (ms, load excluded). Two runs per clip in the same process; first run, second run. Warm process in the table; the cold process differed by at most 20 % (Whisper short first run was 1057 ms the very first time).

| Clip | Parakeet batch | Whisper large-v3 626 MB | Parakeet streaming, whole file fed as fast as possible |
|---|---|---|---|
| short, 8.15 s | 66 / 54 | 788 / 752 | 688 total (11.8x real time), `finish()` 14 ms |
| long, 29.23 s | 197 / 187 | 1379 / 1379 | 2519 total (11.6x real time), `finish()` 15 ms |

Batch Parakeet is about 15x to 25x faster than Whisper large-v3 here. For a typical 5 s utterance the batch pass adds well under 100 ms.

### Transcripts (long clip, differences only)

Reference script has "GitHub Actions", "pull request", "schema change touches", "555 0134".
- Batch: correct on all four, writes `555-0134 or send`. The short clip says "Seizo" for "Sayso".
- Streaming `70_2_2`: "yesterday, the Kubernetes" (period lost), "GitHub actions" (lower case), **"pool request"**, "schema changed touches". Short clip: "Seizo", `5550134` (digits not hyphenated, same as batch).
- Whisper: all words right, `555-0134, or`. Short clip: "Sazo".
- Token-level diff of streaming final vs batch final (whitespace tokens, punctuation counts): 12.5 % (short), 7.5 % (long). Whisper vs batch: 18.8 % (short), 1.3 % (long). So the preview is close but not identical to the final pass. Paste the batch result as the plan says.

### Streaming observations (`stream_file`, 160 ms chunks, real-time mode sleeps one chunk per chunk)

- Partial events fire only when the decoder emitted new tokens. They arrive once per 160 ms chunk while speech is present: 29 partials (short), 108 (long). Median gap between partials in audio time is 160 ms. I only tested `chunk_ms: 160`. The model steps in 160 ms units (chunk = 2 frames), so smaller feed chunks should not give faster updates. Not tested.
- Processing cost per chunk (time from `appendAudio` to partial callback): median 15 ms, p95 26 ms, max 123 ms (long clip, real-time mode). The cost of one step is far below the 160 ms chunk.
- First partial ("I", long clip) appeared when 800 ms of audio had been fed. This includes the clip's leading silence plus the 320 ms theoretical latency (chunk + right context). Each later word shows up a few chunks after it is spoken.
- Partials are **append-only**: 0 of 108 partials revised earlier text. The last partial equals the final text exactly. `finish()` after the last chunk takes about 15 ms. So there is no "text jumps" problem inside the stream, but the stream text can differ from the batch text (above).
- The real-time and fast modes produced identical text.
- Streaming and batch managers can live in one process (RSS 116 MB with all three loaded in a warm process, 155 MB in the cold one; no ANE conflict seen).

### Protocol notes

- Requests: `{"v":1,"id":"r1","type":"load_model","engine":"whisper"}` etc. `engine` is optional on `transcribe_file` and `stream_file` (defaults to the last loaded engine). `stream_file` takes extra `realtime: bool`. Extra request `mic_status` (S4).
- Events carry the request `id`: `model_state` (`downloading`, `loading`, `ready` with `download_ms`, `load_ms`), `download_progress`, `partial` (`text`, `audio_ms`, `wall_ms`, `since_chunk_ms`), `final` (`text`, `ms`, plus `total_ms`, `partials` for streams), `error`, `log`.
- `download_progress` is far too chatty: about 30,000 events for one 600 MB model (FluidAudio) and about 80 for WhisperKit. Throttle it in the engine before the real app.
- Stdout hygiene: the engine does `dup(1)` for the protocol and `dup2(2, 1)` so stray `print()` calls in dependencies go to stderr. FluidAudio's `AppLogger` writes to stderr (`mirrorsToConsole`); WhisperKit's `Logging` defaults to `.none`.

### Gotchas

- Swift 6 language mode gives concurrency errors on the top-level mutable globals. The spike uses `swiftLanguageMode(.v5)`. A real engine should wrap state in an actor.
- `ModelHub.download(...)` and `WhisperKit.download(...)` always call the network even when files exist (about 1 s to 2 s). Check for a complete folder first when offline.
- FluidAudio int8 encoder: the library forces `cpuAndNeuralEngine` for it (GPU path crashes in MPSGraph), already handled inside the package.

## S4. Bundle and signing

### Layout

```
build/SaysoSpike.app/Contents/
  Info.plist            dev.sayso.spike, LSUIElement=true, NSMicrophoneUsageDescription
  PkgInfo
  MacOS/SaysoSpike      Swift main (swiftc, arm64, macOS 14)
  MacOS/SaysoEngine     the sidecar from Engine/.build/release
```

Sidecar location: **`Contents/MacOS`**. Apple's guidance ("Placing content in a bundle", Developer Forums notarization threads) lists `Contents/MacOS` for the main executable, helper apps and helper tools, and `Contents/Frameworks` for frameworks and dylibs. Code in other places may work in development but can fail at notarization. Rust side: find it with `current_exe().parent().join("SaysoEngine")`. (`Contents/Helpers` is used by Chromium and Electron, but it is a custom path, so each item there needs explicit signing and Apple does not list it as a standard location.)

### Signing

`sign.sh` (rebuild, assemble, sign inside-out, verify). Order: sidecar first with `--identifier dev.sayso.spike.engine`, then the bundle with `--identifier dev.sayso.spike`. Both use `--force --timestamp --options runtime`, entitlements `com.apple.security.device.audio-input` and identity `Developer ID Application: Watzon Ventures LLc (MB5789APU7)`. No `--deep` for signing (Apple discourages it); `--deep` only for verify. Run twice, both runs succeeded with no keychain prompt.

Outputs (full text in `runs/signing.txt`):

```
$ codesign --verify --deep --strict --verbose=2 build/SaysoSpike.app
--prepared:.../Contents/MacOS/SaysoEngine
--validated:.../Contents/MacOS/SaysoEngine
build/SaysoSpike.app: valid on disk
build/SaysoSpike.app: satisfies its Designated Requirement

$ codesign -d --entitlements - build/SaysoSpike.app     (same output for the sidecar)
[Dict]
	[Key] com.apple.security.device.audio-input
	[Value]
		[Bool] true

$ codesign -dvv build/SaysoSpike.app    (key lines)
Identifier=dev.sayso.spike
CodeDirectory v=20500 ... flags=0x10000(runtime)
Authority=Developer ID Application: Watzon Ventures LLc (MB5789APU7)
Authority=Developer ID Certification Authority
Authority=Apple Root CA
Timestamp=Oct 1, 2026 at 5:53:43 PM
TeamIdentifier=MB5789APU7

$ spctl --assess --type execute -vv build/SaysoSpike.app
build/SaysoSpike.app: rejected
source=Unnotarized Developer ID
origin=Developer ID Application: Watzon Ventures LLc (MB5789APU7)
(exit 3; same for the sidecar)
```

Designated requirement (what TCC matches on): `identifier "dev.sayso.spike" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */ and certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */ and certificate leaf[subject.OU] = MB5789APU7`. It contains the bundle id and team id, not the cdhash, so it is identical after every rebuild with the same identity.

Findings:
- The sidecar runs fine under the hardened runtime with only the audio-input entitlement. No JIT, unsigned-memory or library-validation exception is needed for Core ML, ANE or the Swift runtime (full cold-to-warm run of batch and streaming from the signed copy; transcripts identical).
- Inside the signed bundle the first load recompiles for ANE (batch 12.3 s, streaming 14.8 s) because the cache dir is now `~/Library/Caches/dev.sayso.spike`. After a rebuild and re-sign (`signed2` run, new binary hashes) the load is warm again (0.14 s and 0.08 s). So the ANE cache survives re-signing.
- `notarize.sh` is written and NOT run. It takes a keychain profile argument, runs `ditto -c -k --keepParent`, `xcrun notarytool submit --keychain-profile <profile> --wait`, `xcrun stapler staple`, `stapler validate` and `spctl`. The owner must first create the profile with `xcrun notarytool store-credentials`.
- Not verified: that Apple accepts this bundle at notarization (the plan's "unverified" item). Likely fine: all code is Developer ID signed, hardened, timestamped, no nested bundles, standard location. Needs the owner's approval to test.

### Is the sidecar's mic access attributed to the parent app?

Researched, not tested end to end (a test needs a real TCC prompt).
- macOS TCC checks the **responsible process**: the first ancestor started by launchd, or a process that was spawned with the "disclaim responsibility" attribute. A helper started with `Process`/`posix_spawn` without disclaiming is covered by the parent app's TCC record. Sources: [ghostty #9263](https://github.com/ghostty-org/ghostty/issues/9263), [vscode #307364](https://github.com/microsoft/vscode/issues/307364), [t3code #728](https://github.com/pingdotgg/t3code/issues/728), [oh-my-openagent #9345](https://github.com/code-yeongyu/oh-my-openagent/issues/9345).
- With the hardened runtime, the entitlement `com.apple.security.device.audio-input` must be present on the responsible app. Without it macOS denies silently. Reports also say child processes that capture should carry it ([cmux #1325](https://github.com/manaflow-ai/cmux/issues/1325)). So the spike signs the sidecar with it too (harmless).
- The prompt text comes from `NSMicrophoneUsageDescription` of the responsible app. A bare sidecar with no Info.plist is fine when launched by the app.
- Launching the app from a terminal makes Terminal the responsible process, so always test with `open`.
- Product advice: record audio in the Rust app (the responsible process, one grant, one entitlement) and stream PCM or files to the sidecar. The sidecar then never needs mic access.
- I added a read-only `mic_status` request to the sidecar. Run directly from my shell it printed `mic authorizationStatus=notDetermined` (it never prompts). Step 6 below checks it as a child of the app.

## Human test: does the mic grant survive a rebuild and re-sign?

The app logs to `/tmp/sayso-spike.log`. Always start it with `open`, never by running the binary.

1. `cd /Users/watzon/Projects/personal/sayso/spikes/engine && ./sign.sh`
2. `rm -f /tmp/sayso-spike.log && open build/SaysoSpike.app`
3. A system prompt appears: "Sayso Spike would like to access the microphone" with the text from Info.plist. Click **Allow**. (No Dock icon appears. The app is `LSUIElement` and quits by itself.)
4. `cat /tmp/sayso-spike.log`. Expected lines:
   - `authorizationStatus(.audio) at launch = notDetermined`
   - `requesting microphone access (a prompt should appear)`
   - `requestAccess result: granted=true, status now authorized`
   - `spawned sidecar pid N` and `sidecar says: ... "mic_status":"authorized"` (this is the attribution check: authorized in the child means it follows the parent's grant). If the sidecar says `notDetermined` or `denied` while the app says `authorized`, the sidecar is its own TCC subject and must not capture audio.
5. Rebuild and re-sign: `./sign.sh` (it wipes `build/` and signs again; the DR stays the same). Optional: edit a comment in `app/main.swift` first so the binary hash changes.
6. `rm -f /tmp/sayso-spike.log && open build/SaysoSpike.app`
7. Pass criteria: no prompt appears, and the log shows `authorizationStatus(.audio) at launch = authorized`, then `done`.
8. Negative control (optional, not run by me): sign ad hoc with `codesign --force --sign - build/SaysoSpike.app`, then run step 6. Expect the prompt to come back because the designated requirement changed (ad-hoc signatures use the cdhash). Then run `./sign.sh` again.
9. To reset between experiments, the user can remove the grant in System Settings > Privacy & Security > Microphone (I did not use `tccutil`).
