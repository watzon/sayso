# sayso-speech

Jobs and live sessions over a [Sayso](https://github.com/watzon/sayso) speech engine. Sayso uses this crate for the jobs and the live sessions of the Host API, and the SDK uses it in local mode, so both run the same code.

- **Jobs** (`jobs`): read a file (WAV, FLAC, MP3, AAC, or M4A), find the speech, transcribe it in chunks, and find the speakers. One job runs at a time, first in, first out.
- **Live sessions** (`live`): transcribe audio while it arrives, from one or more sources. Each speech that ends is a part, and the final model transcribes it. At the stop, diarization runs on the recording of each source.
- **Basic VAD** (`basic_vad`): voice detection without a model.
- **The resampler** (`resample`): any sample rate to 16 kHz.

The events are the messages of [`sayso-protocol`](https://crates.io/crates/sayso-protocol). The crate gets the rest through traits:

| Trait | What it does |
|---|---|
| `SpeechEngine` | The calls to the engine: load, transcribe, streams, VAD, diarization |
| `ModelResolver` | The models of a request, and the checks (`unknown_model`, `wrong_role`, ...) |
| `ModelRetention` | Keeps the models of work in memory while it runs |
| `Gate` | Dictation first, then live sessions, then jobs |
| `EventSink` | Where the events go |
| `Recordings` | Where the recordings of live sessions go |

The calls block, and the work runs on threads of this crate, so you need no async runtime.

## Documentation

- [The SDK plan](https://github.com/watzon/sayso/blob/main/docs/sdk.md)
- [The Host API](https://github.com/watzon/sayso/blob/main/docs/extensions.md)

## License

MIT. See `LICENSE`. The decoder `symphonia` has the license MPL-2.0.
