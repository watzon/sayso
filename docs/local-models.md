# Fluid Speech Mini

Status: built on 2026-10-15. S1-mini by Superwhisper was looked at and dropped: it is a fixed cleanup model that cannot run a style prompt.

## The model

An English speech model by FluidVoice, released on 2026-10-13. It is a quantized Parakeet Ultra (a post-trained Parakeet TDT 0.6B v3). It does batch transcription only, so it can be a final model but not a preview model.

- Core ML build: `FluidVoice-Labs/fluid-speech-mini-coreml`, revision `d81c3160dfe5a5e1f8f7267164a07d30617a3a1a`, 253 MB, macOS 15 or later.
- Linux: the GGUF build for parakeet.cpp, `FluidVoice-Labs/fluid-speech-mini-gguf`, revision `490bcd5de891f5efd0e732434dfb0a4b0899aead`, 347 MB, on the GPU through Vulkan ([linux.md](linux.md)).
- Not on Windows. The ONNX build uses `MatMulNBits` 2-bit and 4-bit weights, which the Windows engine does not run.

## License

CC BY 4.0, and the parent models are also CC BY 4.0. Commercial use and a download at runtime are allowed. Model details shows the credit that the license asks for: "Fluid Speech Mini by FluidVoice. Built on NVIDIA Parakeet TDT 0.6B v3 and Moondream Parakeet Ultra."

## Accuracy

FluidVoice reports a 4.54% average WER on its own test sets. Sayso's benchmark (`docs/benchmark.md`) gives 6.61% WER (93.4%) at 159× real time, between Parakeet Ultra and Parakeet TDT v3. The Linux GGUF gives 6.56% at 213× on an RTX 4070.

## How it runs

FluidAudio 0.17.5 has no repository entry for the model. Only a fork (`altic-dev/FluidAudio`) has one. The sidecar handles it with upstream FluidAudio:

- **Download:** its own downloader fetches the files at the pinned revision, because `ModelHub.download` takes only FluidAudio's own repositories.
- **Load:** `AsrModels.loadLocal(from:version: .ultra)`, with a link `JointDecisionv3.mlmodelc` to `JointDecision.mlmodelc`. Do not use `AsrModels.load(from:version:)`: it ignores the folder and downloads the real Parakeet Ultra.
- **Result:** a 14 s clip takes about 0.07 s, with punctuation and capitals. The first load takes about 10 s.
