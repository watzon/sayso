# Model benchmark

The accuracy and speed figures on the Models page come from this benchmark. Sayso measures the builds that it ships (Core ML on macOS), with the real sidecar.

## Method

- **Clips:** 1,400 clips (2.9 h) from seven English test sets of the Open ASR Leaderboard (`hf-audio/open-asr-leaderboard`): AMI, Earnings22, GigaSpeech, LibriSpeech clean, LibriSpeech other, SPGISpeech, and VoxPopuli. 200 clips from each set. TED-LIUM is not in that dataset. A set is sorted by clip length, so the sample takes clips from its first, middle, and last file, with a fixed seed. Clips shorter than 1 s get silence to 1 s, because FluidAudio rejects shorter audio.
- **Run:** each clip goes through the sidecar as a final pass, in English, with no dictionary words. The first call after a load is not timed.
- **Accuracy:** the text and the reference go through Whisper's English normalizer, as on the leaderboard, so punctuation and capitals do not count. WER is over all words of a set. The page shows 100 minus the mean of the seven set WERs.
- **Speed:** audio time over processing time (RTFx), from the time that the sidecar reports.
- **Live preview:** for Parakeet Unified EN, whose preview runs another decoder than its final pass, a second run streams each clip and scores the last preview text.
- **Machine:** Apple M5 Pro, macOS 27.0. Windows and Linux run other builds. They show the same accuracy figures and no speed figures. Fluid Speech Mini on Linux shows its own accuracy figure (see "Linux").

## Run it again

```text
uv run scripts/benchmark/prepare.py
(cd native/macos/SaysoEngine && swift build -c release)
SAYSO_BENCH_MODELS=all cargo test --release -p sayso-engine-client --test benchmark benchmark_every_model -- --ignored --nocapture
SAYSO_BENCH_PREVIEW=parakeet-unified-en cargo test --release -p sayso-engine-client --test benchmark benchmark_preview -- --ignored --nocapture
uv run scripts/benchmark/score.py --rust crates/sayso-core/src/models/benchmark.rs --json docs/benchmark-results.json --machine "Apple M5 Pro, macOS 27.0"
```

The clips, results, and models go to `~/.cache/sayso-bench`. A run that stops continues where it stopped. Delete `results/<model>.jsonl` to measure a model again. The full run takes about five hours, most of it for the large Whisper models and Canary.

## Fixes that the first run found

The first run found three causes of empty text, and Sayso fixed them before the final run:

- **Whisper:** WhisperKit skipped recordings of 1 s or less, and its first-token check started a temperature fallback that sometimes ended with no text. 12% of the clips were empty. See the commit "Stop Whisper from returning no text".
- **SenseVoice:** the int8 and fp16 encoders gave NaN logits on about 10% of clear speech. Sayso now uses the fp32 encoder. See "Stop SenseVoice from returning no text".
- **Streaming final pass:** Nemotron and Parakeet EOU gave no text for many short dictations. The final pass now adds one second of silence on both sides. See "Pad the streaming final pass with silence".

Some very short clips ("Mm", "Yeah") still give no text, mostly with the small streaming models.

## Results

Measured on 2026-10-16. WER per set in percent. Speed is audio time over processing time.

| Model | Accuracy | WER | Speed | AMI | Earnings22 | GigaSpeech | LS clean | LS other | SPGISpeech | VoxPopuli |
|---|---|---|---|---|---|---|---|---|---|---|
| parakeet-unified-en | 93.7% | 6.29 | 145× | 8.64 | 11.16 | 10.42 | 1.78 | 3.08 | 2.44 | 6.50 |
| cohere-transcribe | 93.7% | 6.32 | 15× | 8.20 | 10.41 | 11.07 | 1.26 | 3.80 | 3.22 | 6.31 |
| parakeet-tdt-v2 | 93.6% | 6.37 | 151× | 9.02 | 10.59 | 10.19 | 1.88 | 3.74 | 2.89 | 6.29 |
| parakeet-ultra | 93.5% | 6.54 | 148× | 10.09 | 10.24 | 9.67 | 1.68 | 3.55 | 4.21 | 6.31 |
| fluid-speech-mini | 93.4% | 6.61 | 159× | 8.78 | 10.57 | 9.49 | 1.70 | 3.74 | 4.48 | 7.51 |
| parakeet-tdt-v3 | 93.1% | 6.91 | 132× | 9.51 | 11.80 | 10.03 | 1.91 | 3.93 | 4.62 | 6.57 |
| parakeet-redux | 93.1% | 6.94 | 93× | 9.17 | 10.35 | 9.97 | 2.32 | 5.06 | 5.04 | 6.70 |
| phonon-2 | 92.7% | 7.33 | 143× | 8.35 | 11.35 | 11.66 | 2.25 | 4.95 | 4.93 | 7.83 |
| nemotron-en | 92.4% | 7.57 | 61× | 11.26 | 11.08 | 10.10 | 2.43 | 5.94 | 3.59 | 8.59 |
| parakeet-unified-en (live preview) | 92.4% | 7.61 |  | 10.63 | 12.56 | 10.78 | 2.32 | 5.17 | 3.66 | 8.17 |
| parakeet-tdt-ctc-110m | 92.0% | 7.97 | 111× | 15.04 | 12.91 | 10.17 | 2.43 | 5.01 | 2.81 | 7.44 |
| apple-speech | 92.0% | 8.05 | 49× | 12.86 | 12.02 | 9.85 | 1.86 | 4.59 | 5.37 | 9.77 |
| whisper-large-v3-turbo | 91.8% | 8.19 | 12× | 16.98 | 11.91 | 10.67 | 2.22 | 3.82 | 3.20 | 8.53 |
| whisper-large-v3 | 91.6% | 8.41 | 4× | 17.37 | 12.99 | 11.10 | 1.81 | 4.35 | 3.53 | 7.74 |
| canary-1b-v2 | 91.5% | 8.51 | 5× | 16.25 | 13.50 | 11.71 | 2.71 | 4.73 | 3.22 | 7.46 |
| whisper-large-v2 | 91.4% | 8.57 | 4× | 17.42 | 13.12 | 10.40 | 2.50 | 5.58 | 3.47 | 7.51 |
| sensevoice-small | 91.1% | 8.88 | 30× | 10.04 | 13.42 | 15.21 | 3.48 | 7.18 | 3.28 | 9.55 |
| whisper-medium | 90.8% | 9.15 | 8× | 16.35 | 14.06 | 11.34 | 2.86 | 6.96 | 4.56 | 7.89 |
| nemotron-multilingual | 90.6% | 9.37 | 52× | 15.62 | 13.42 | 11.80 | 3.23 | 7.84 | 3.94 | 9.77 |
| whisper-distil-large-v3 | 90.3% | 9.72 | 12× | 18.44 | 15.30 | 11.50 | 2.68 | 6.19 | 4.73 | 9.17 |
| whisper-small | 90.2% | 9.77 | 18× | 17.27 | 13.50 | 11.86 | 4.05 | 8.11 | 4.52 | 9.04 |
| whisper-base | 88.4% | 11.62 | 63× | 19.17 | 17.61 | 12.63 | 4.59 | 12.57 | 4.38 | 10.39 |
| parakeet-eou-120m | 85.9% | 14.08 | 18× | 18.53 | 21.56 | 18.06 | 7.66 | 13.01 | 7.16 | 12.60 |
| whisper-tiny | 85.3% | 14.70 | 92× | 22.61 | 22.08 | 14.62 | 6.25 | 16.91 | 7.97 | 12.50 |

## Linux

Fluid Speech Mini runs another build on Linux: FluidVoice's GGUF on parakeet.cpp, on the GPU through Vulkan. The Linux catalog shows its own figures for it. Parakeet TDT v2 on the CPU is here for comparison only; the Linux catalog shows the macOS figures for it, like the other models.

Measured on 2026-10-18 with the same 1,400 clips and harness, `SAYSO_ENGINE_PATH` set to the Linux engine. Machine: NixOS, Ryzen 7 7700X, RTX 4070 (driver 595.104.02, Vulkan through NVIDIA's driver only). The GPU ran the clips back to back; after the GPU idles for a few seconds, as between dictations, NVIDIA's driver needs longer to raise its clocks, so one dictation is slower than this speed suggests.

| Model | Accuracy | WER | Speed | AMI | Earnings22 | GigaSpeech | LS clean | LS other | SPGISpeech | VoxPopuli |
|---|---|---|---|---|---|---|---|---|---|---|
| fluid-speech-mini (GGUF, Vulkan) | 93.4% | 6.56 | 213× | 8.98 | 10.33 | 9.49 | 1.73 | 3.71 | 4.67 | 7.04 |
| parakeet-tdt-v2 (sherpa-onnx, CPU) | 93.3% | 6.72 | 40× | 9.85 | 11.99 | 10.24 | 1.55 | 3.99 | 2.95 | 6.48 |

## Dictation while an extension works

Step 6 of `docs/extensions.md` needs this check: a job or a live session of an extension must not make dictation slower. The test measures the time of the final pass of a dictation, with the real Swift engine, in three cases.

### Method

- **Dictations:** the first 15 LibriSpeech clean clips of 5 to 15 s from the clips above (11.6 to 14.6 s). Each dictation does what the app does: `DictationGate::begin` when the recording starts, the recording time, `SttBackend::transcribe` with `parakeet-unified-en` (the measured time), then `DictationGate::end`, and 0.5 s of pause. The model is in memory before the first measurement.
- **Recording time:** each clip runs twice. First with a recording as long as the clip, as in real use. Then with no recording time: the final pass starts at the same moment as `begin`, so a part of a job or of a live session that is in the engine then is not done yet. That is the worst case.
- **Nothing else:** only the dictations.
- **A job:** an extension transcribes a file of 5 minutes (AMI meeting clips with 1 s of silence between them) with Silero VAD and diarization, again and again, over the real socket of the host. The dictations start when the first job has its first segment. 7 jobs ended in the 215 s of dictations.
- **A live session:** an extension feeds the same 5 minutes in real time to a live session with the source `client` and diarization. The dictations start after 10 s of audio. The session gave 55 segments.
- **Machine:** Apple M5 Pro, 48 GB, macOS 27.0, on 2026-10-26.

### Results

| Case | Recording | Median (ms) | p95 (ms) | Slowest (ms) |
|---|---|---|---|---|
| Nothing else | as long as the clip | 84 | 102 | 108 |
| Nothing else | none (worst case) | 74 | 81 | 82 |
| A job (VAD and diarization) | as long as the clip | 82 | 87 | 88 |
| A job (VAD and diarization) | none (worst case) | 72 | 114 | 142 |
| A live session (diarization) | as long as the clip | 82 | 89 | 90 |
| A live session (diarization) | none (worst case) | 70 | 78 | 80 |

With a real recording time, a job and a live session do not make dictation slower: the medians and the p95 are the same as with nothing else, within the noise of one run. The job finishes its current part during the recording and then waits, and a live session waits with its final passes.

In the worst case, a job adds up to about 60 ms to some dictations (p95 114 ms against 81 ms, slowest 142 ms). The final pass then waits in the engine for the chunk of the job that started before the dictation (up to 30 s of audio). The median does not change. A real dictation always has a recording time before its final pass, so a user does not see this case, except with a very short recording that starts while a chunk is in the engine. A fix, if it becomes necessary: smaller chunks for jobs (for example 15 s), or a priority for the final pass in the engine. The live session shows no such cost, because its parts are short.

The times with a recording are a little higher than the times without one, also with nothing else. The engine is idle during the recording, and the first call after an idle time is slower (the clocks of the chip go down). This is the same in each case, so it does not change the comparison.

### Run it again

```text
uv run scripts/benchmark/prepare.py
(cd native/macos/SaysoEngine && swift build -c release)
SAYSO_CONF_MODELS=<a models folder with parakeet-unified-en, silero-vad, speaker-diarization> \
  cargo test --release -p sayso-host --test dictation_bench -- --ignored --nocapture
```

`SAYSO_BENCH_CLIPS` sets the number of dictations (default 15). Without the clips in `~/.cache/sayso-bench`, set `SAYSO_CONF_AUDIO` to a 16 kHz mono WAV file. The run takes about 11 minutes and prints the table above.
