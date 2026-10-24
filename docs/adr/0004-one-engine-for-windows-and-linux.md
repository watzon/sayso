# Windows and Linux share one engine, `sayso-engine`

Windows had its own engine, `native/portable` (transcribe-rs), because the Windows and Linux ports were made at the same time. The extension platform adds VAD, diarization, segments, concurrent streams, and feature negotiation to the engine protocol, and each of those changes must be built once for each engine. So Windows moves to `crates/sayso-engine`, the Linux engine, and `native/portable` is deleted. sherpa-onnx then gives Windows VAD and diarization with the code that Linux uses. Whisper on Windows stays on whisper.cpp, through `transcribe-rs` with only its `whisper-cpp` feature, so the binary links one ONNX Runtime and Whisper keeps its speed and its downloaded files. Design: "Windows engine" in [extensions.md](../extensions.md).

## Considered options

- **Silero VAD through `ort` in `native/portable`.** Small for VAD, but diarization through `ort` means writing the segmentation, embedding, and clustering code that sherpa-onnx already has, and the plan keeps three engines.
- **sherpa-onnx inside `native/portable`.** `transcribe-rs` links ONNX Runtime statically through `ort`, and sherpa-onnx links its own copy. Two static copies in one binary can clash, and the plan keeps three engines.

## Consequences

- Parakeet TDT v2 and v3 on Windows need new files, because sherpa-onnx reads other files than `transcribe-rs`. The model ids stay, and Sayso downloads the models in use again at the first start after the update.
- The re-decode live preview moves from `native/portable` into `sayso-engine`, so Linux gets a live preview for Parakeet TDT v3 too.
- The Windows libraries of sherpa-onnx use the static C runtime (`/MT`), and whisper.cpp the DLL runtime (`/MD`). So on Windows sherpa-onnx links as DLLs (`sherpa-onnx-c-api.dll`, `onnxruntime.dll`) that ship beside the engine, and the engine is no longer one file.
- If sherpa-onnx is more than 20 % slower than `transcribe-rs` for Parakeet on the reference Windows machine, the move stops and this decision is made again.
