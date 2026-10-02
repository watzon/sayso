# sayso-transcribe

Blocking HTTP clients for the cloud final pass. Each call sends one WAV recording to a speech provider and returns the transcript. The contract types are in `sayso_core::speech`.

## API

- `transcribe(&SpeechProvider, Option<&str>, &SpeechRequest) -> Result<String, SpeechError>`. Returns the transcript, trimmed. Silence gives `Ok("")`.
- `check(&SpeechProvider, Option<&str>, Duration) -> Result<(), SpeechError>`. A cheap request that proves the address and the key work. It sends no audio, except for AssemblyAI.
- `list_models(&SpeechProvider, Option<&str>, Duration) -> Result<Vec<String>, SpeechError>`. Reads `data[].id` from `GET {base}/models`, sorted. Only the OpenAI-compatible kinds have a list. The other kinds return `kind.known_models()` and send no request.

The caller reads the API key from the Keychain and passes it in. The crate never stores, logs, or returns the key. The audio is never logged. The debug log has the provider id, the model, the status, and the elapsed time.

## Providers

| Kind | Endpoint | Auth | Language field | Vocabulary field | Response path |
|---|---|---|---|---|---|
| OpenAi, Groq, OpenAiCompatible | `POST {base}/audio/transcriptions`, multipart | `Authorization: Bearer <key>` | `language` | `prompt`: words joined with ", ", cut at a word boundary to 800 characters | `text` |
| OpenAi, model `gpt-transcribe` | same | same | `languages[]` | `keywords`: one field, one word per line (`<`, `>`, CR, LF removed) | `text` |
| Mistral | same | same | `language` | `context_bias`: one field per word, at most 100 | `text` |
| ElevenLabs | `POST {base}/speech-to-text`, multipart (`model_id`, `tag_audio_events=false`, `diarize=false`) | `xi-api-key: <key>` | `language_code` | `keyterms`: one field per word, at most 100, words of 50 characters or more skipped | `text` |
| Deepgram | `POST {base}/listen?model=..&smart_format=true&punctuate=true`, raw WAV, `Content-Type: audio/wav` | `Authorization: Token <key>` | query `language`, or `detect_language=true` when there is no language | query `keyterm`, one per word, only for models that start with `nova-3`, at most 400 characters in total | `results.channels[0].alternatives[0].transcript` |
| AssemblyAi | `POST {base}/transcribe`, multipart: `config` (JSON), then `audio` | `Authorization: <key>` (no "Bearer"), `X-AAI-Model: <model>` | `config.language_codes: [code]` | `config.keyterms_prompt`: at most 100 words | `text` |

`check` per kind: `GET {base}/models` for the OpenAI-compatible kinds and ElevenLabs, `GET {base}/projects` for Deepgram. AssemblyAI has no list endpoint. Its `check` sends 0.5 s of silence (16 kHz, mono, 16-bit). A 2xx answer passes. HTTP 400 or 422 with `error_code` `audio_too_short` or `bad_audio` also passes, because the service accepted the key before it read the audio. 401 and 403 fail.

Error text per kind: OpenAI-compatible `error.message`, then `message`, `detail`, and a plain `error` string. ElevenLabs `detail.message`, `detail` (string), or `detail[0].msg`. Deepgram `err_msg`, then `message`. AssemblyAI `error`, then `detail` (string), then `error_code`.

## Behavior

- Setup is checked before any request. No address gives `NotConfigured("no address is set")`. A cloud provider (`SpeechProvider::is_cloud`) with a missing, empty, or blank key gives `NotConfigured("no API key is saved")`. A local server may have no key. Then no auth header is sent. With a key, the header is sent.
- `request.timeout` is one deadline for the whole call, retries included. A zero timeout means 30 s. `check` and `list_models` use their `timeout` argument in the same way.
- Errors: a timeout is `Timeout(ms)` with the deadline that was set. Other transport errors are `Network`. A non-2xx answer is `Http { status, message }`. The message is the provider's error text. If the body has none, it is the first 200 characters of the body. A 2xx body without a transcript string is `InvalidResponse`.
- Retry without hints, for every provider: if the first answer is HTTP 400 or 422 and the request had a language or a dictionary word, `transcribe` sends the request once more without the language and the words. It returns the second result. A provider that rejects a hint (an unknown field, a language it does not support, a word it does not accept) then still gives a transcript. The retry uses what is left of the deadline.
- Local URLs bypass any system proxy (the same rule as `sayso-enhance`).
- Multipart bodies are built by hand (`src/multipart.rs`). Each body has its own boundary.

## Tested (no network, no keys)

Mock `tiny_http` server that records the method, path with query, headers, and body bytes of each request (`tests/http.rs`):
- Per adapter: path, auth header, model, language field (and its absence), vocabulary field, WAV bytes in the body (binary-safe), parsed transcript.
- `gpt-transcribe` field names, and that only the `OpenAi` kind gets them. Mistral `context_bias` with the 100-word limit. The 800-character prompt limit. ElevenLabs long-word skip. Deepgram `detect_language`, percent-encoding, `nova-3` only, and the key term budget. AssemblyAI part order and config JSON.
- The retry without hints after 400 and 422 for every adapter, the retry returning the second error, no retry without hints, no retry for other statuses, the shared deadline.
- Error mapping for each provider's error JSON, an error body that is not JSON, 200 without a transcript, an unreachable port, a slow server, silence.
- Missing key for a cloud provider (no request is sent), missing address, a local server without a key and without an auth header, the user agent.
- `check` and `list_models` for the OpenAI-compatible kinds, ElevenLabs, and Deepgram. AssemblyAI `check` with silent audio, accepted `audio_too_short` and `bad_audio`, rejected 401, 403, and other 400 answers. `list_models` for the other kinds returns known ids with no request.

Unit tests: multipart builder, prompt truncation, percent-encoding, silent WAV header, deadline, error message fallback.

`tests/real.rs` has one `#[ignore]` test per provider. It reads `SAYSO_TEST_OPENAI_KEY`, `SAYSO_TEST_GROQ_KEY`, `SAYSO_TEST_ELEVENLABS_KEY`, `SAYSO_TEST_DEEPGRAM_KEY`, `SAYSO_TEST_ASSEMBLYAI_KEY`, or `SAYSO_TEST_MISTRAL_KEY`, and skips when it is not set. Run it with `cargo test -p sayso-transcribe --test real -- --ignored --nocapture`. Nobody has run it yet.

## Not tested

No call to a real provider has run. The request shapes follow the public API references as read on 2026-10-02. These details are unverified:

- OpenAI `gpt-transcribe`: the field names `languages[]` and `keywords`, and the newline format of `keywords`.
- ElevenLabs `keyterms`: one repeated field per word. The API may want a JSON array in one field. The 50-character and 100-word limits are unverified.
- Mistral `context_bias`: one repeated field per word. The API may want a comma-separated string or a JSON array. The 100-word limit is unverified.
- AssemblyAI Sync: the path `/transcribe`, the `X-AAI-Model` header, the `config` part and the names `language_codes` and `keyterms_prompt`, the `error_code` values `audio_too_short` and `bad_audio`, and the status codes for them. The default base URL `https://sync.assemblyai.com/v1` comes from `sayso-core`.
- Error JSON for Groq, Mistral, and ElevenLabs. The code assumes Groq and Mistral use the OpenAI shape.
- Deepgram `check` uses `GET /projects`. A key that is limited to transcription may get 403 there.
- Deepgram `detect_language=true` with `nova-3`, and `keyterm` with a language other than English.
- Whether every provider answers 400 or 422 for a hint it rejects. A provider that answers 500 gets no retry.
- HTTPS and TLS, proxies, redirects, and large recordings.
