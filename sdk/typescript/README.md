# @justsayso/sdk

Speech to text for apps, from [Sayso](https://github.com/watzon/sayso). Pure TypeScript, MIT licensed, no runtime dependencies, no native addon. Needs Node.js 22.12 or newer.

`Speech` has three modes:

- `auto`: use Sayso when it runs, else the engine of your app. It never starts Sayso.
- `sayso`: use Sayso (its models, its approval question, its AI styles). Fails with `host_not_running` when Sayso is not there. `{ sayso: { start: true } }` starts it.
- `local`: use the engine of your app. It never talks to Sayso.

```ts
import { Engine, Speech } from "@justsayso/sdk";

await using speech = await Speech.connect({
  mode: "auto",
  app: { name: "Notes" },
  capabilities: ["models.read", "transcribe.file"],
  engine: Engine.fromPackage(),
});
const { text } = await speech.transcribe({ path: "/abs/meeting.wav" });
```

Local mode reads WAV itself. FLAC, MP3, M4A, and microphone and system capture come from the helper program `sayso-audio`, which the SDK finds with `SAYSO_AUDIO_HELPER` or the package `@justsayso/audio-<platform>`; the wire format is `sdk/protocol/audio-helper.md`.

The other parts: `Client` (the plain Host API), `Engine` (the engine protocol over stdio), `Cleanup` and `RuntimeCleaner` (the cleanup runtime), `EmbeddedSpeech`, `Deepgram`, and the types of the protocols.

## Tests

`npm test` builds the package and runs the unit tests, the wire corpus in `../protocol/testdata`, and the integration tests. The integration tests build and run the Go programs `cmd/sayso-testhost`, `sdk/go/cmd/sayso-fake-engine`, and `sdk/go/cmd/sayso-fake-cleanup`, so Go must be installed. `SAYSO_TESTHOST`, `SAYSO_FAKE_ENGINE`, and `SAYSO_FAKE_CLEANUP` name programs that are built already.
