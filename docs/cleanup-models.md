# Cleanup models

Status: built on 2026-10-21 on the branch feature/cleanup-models, not committed. The plan was reviewed by Codex (GPT-6-Astra) and corrected (see Review record). "Changes during the build" lists where the code differs from the first text of this plan. Issue #28. The design is on the page "Cleanup models · S1-mini" in the Paper file "Sayso — v0.1 Design".

This file says how Sayso runs two small local models that clean up a transcript but cannot follow a prompt: S1-mini by Superwhisper and Cleanup 1 by Spokenly.

## Goal

A user who wants only cleanup (no filler words, corrections applied, numbers as digits) can get it on this computer, with no Ollama, no server, and no cloud. Today every style needs a language model that reads a prompt. A cleanup model reads no prompt. It takes the text and a few fixed settings.

The two models are different, and the design must allow for more models with other differences later.

## The two models

| | S1-mini | Cleanup 1 |
|---|---|---|
| Publisher | Superwhisper | Spokenly |
| Base | Qwen3-0.6B, chat model | Qwen3-0.6B-Base, completion model |
| File | `s1-mini-q4_k_m.gguf`, 484,219,808 bytes | `cleanup-1-q8_0.gguf`, 639,442,784 bytes |
| Repository and revision | `superwhisper/s1-mini-GGUF` at `34add00a48a2e5d24e5a4ee5405a99620a3a240c` | `spokenly/cleanup-1-gguf` at `79936c95e5c0ea9162ae7a435e64cb3127020e36` |
| SHA-256 | `3b41ebe2502cbd03e811d5d16b022f5ab551eda58d62597d152f89535003c634` | `66509e0fe1578d2ac4b0773c79a6c83e8d6f218618dfe11e4532beacd999b6e8` |
| License | Apache 2.0 with an added term: keep the name "S1-mini" by "Superwhisper" exactly, and keep LICENSE and NOTICE | Apache 2.0, keep NOTICE |
| Languages | English | English (best), German, Spanish, French, Chinese, Russian, Japanese |
| Input | ChatML. A fixed system prompt (the model card says a change gives bad output). The user message is a control line, a new line, and the text. The assistant turn starts with an empty think block | Plain `key: value` lines, then `out:` |
| Settings | Styling: `casual`, `semi-casual`, `semi-formal`, `formal`. Structure: `prose`, `lists` (three items or more). Context: `general`, `email` | `tone`: `very casual`, `casual`, `formal`, or no tone (then it follows the text before the cursor). Tone changes capital letters and the final period, not words |
| Context input | None | `app`, `site`, `before` (text before the cursor), `after` |
| Limits | Under about 1,000 input tokens; split longer text at sentence ends. `max_new_tokens` about 1.3 × input + 32 | 4,096 tokens for prompt and answer together |
| Decoding | Greedy. Thinking must be off | Greedy. Stops at the end-of-text token. The answer starts with a space |
| Special outputs | Empty text for an input of only filler words | None known |

Sources: the model cards at `huggingface.co/superwhisper/s1-mini`, `huggingface.co/superwhisper/s1-mini-GGUF`, `huggingface.co/spokenly/cleanup-1`, and `huggingface.co/spokenly/cleanup-1-gguf`, read on 2026-10-21. All quality figures come from the publishers. Nobody has compared the two models on the same set.

## Evaluation

Run on 2026-10-21 on an Apple M5 Pro with Homebrew llama.cpp 0.6.0 (`llama-server`, Metal, greedy), with the pinned files and the raw prompts of this plan. The set is the 58 transcripts of `style_eval.rs` and 7 extra cases. The script and the output are outside the repository (`~/.cache/sayso-cleanup-eval`).

| | S1-mini | Cleanup 1 |
|---|---|---|
| Cases that pass, of the styles that the model runs | 40 of 53 | 39 of 49 |
| Median time for one dictation | 68 ms | 82 ms |
| Longest (a long rambling text) | 485 ms | 493 ms |
| Only filler words ("um uh you know") | empty answer | empty answer |
| German, Spanish, French | good, although the card says English only | good |

What the failures say:

- **Not the model's job:** spelled words ("k a t z") and dictionary words ("say so") fail on both. In Sayso, the spelled-word step and the replacements run before the model, so these pass in the app. Dictionary words that are not replacements stay a gap.
- **S1-mini:** acronyms ("APi", "http"), "gonna" becomes "going to", and one correction lost the rest of the sentence ("Let's make it Friday at 10" for "schedule the design review for thursday … make it friday at ten"). Lists come only when the text is clearly a list: "pack my passport my charger a jacket and some snacks" and "first install … second … third" became lists, and "fix the login bug update the docs and ship version two" stayed a sentence.
- **Cleanup 1:** numbers without a thousands comma ("25000"), a 24-hour time for "five thirty pm" ("17:30"), "yOLO". It has no list or email output.
- **Both:** a dropped thought ("I want to go to the never mind") stays, and commas are added to correct text ("The deploy finished, and all the checks passed").
- The app context (`app: Slack` or `app: Mail`) made no difference on the one message that was tested.

S1-mini stays English only in Sayso, as its publisher says, although it did well on three other languages here.

## Terms

These go into `CONTEXT.md` with the code.

**Cleanup model**
A local model that cleans up text with fixed settings and reads no prompt. S1-mini and Cleanup 1 are cleanup models. It is a provider of a style, like Apple Intelligence. It is not a model in the sense of `CONTEXT.md` (it does not transcribe speech).

**Cleanup settings**
The part of a style that a cleanup model uses: tone, layout, and format. A style without cleanup settings needs a provider that reads prompts.

**Tone**
How a cleanup model writes capital letters and the final period: very casual (no capitals, no final period), casual (no final period), or formal. Tone does not change words. Only a model that has tones uses it.

Changes to existing terms:

- **Enhancer:** "The implementations are the OpenAI-compatible endpoint, the Claude CLI, the Codex CLI, and the language model of the engine." A cleanup model is not an enhancer. It implements `Cleaner`.
- **Style:** "A named AI enhancement with a prompt, optional cleanup settings, and optional provider, model, and timeout overrides." "Raw" is the style with no prompt and no cleanup settings.
- **Engine:** unchanged. The cleanup runtime is a separate process (see decision 1).

New term for the process:

**Cleanup runtime**
The process `sayso-cleanup` that runs a cleanup model on this computer with llama.cpp. It knows no model. It loads a GGUF file and completes text.

## Decisions

Chris confirmed the recommendations on 2026-10-21.

1. **Runtime: one Rust process, `sayso-cleanup`, on all three systems.** It links llama.cpp through `llama-cpp-2` (Metal on macOS, CPU on Windows and Linux). Reasons:
   - One implementation, not one in Swift and two in Rust.
   - The Linux engine links a pinned, patched ggml into parakeet.cpp (`crates/sayso-engine/build.rs`), and the Windows engine links whisper.cpp. A second ggml in the same binary can clash, and a link that works today does not prove that the backends agree or that later updates will.
   - A crash or a hang in llama.cpp does not stop dictation. Sayso kills the process and inserts the text after replacements.
   - Sayso frees the memory when it stops the process.

   The cost is one more binary in each release file, and one more binary to sign and notarize on macOS. `llama-cpp-2` and `llama-cpp-sys-2` are pinned to an exact version. Default features are off (no OpenMP, so no `libgomp` on Linux). The spike results are in "Runtime spike".
2. **The app downloads the model files, not the engine.** The engines' download paths are for speech models (`EngineKind`, no hash check). Sayso pins the repository revision, the size, and the SHA-256 of each file. A new resumable downloader is necessary: the updater's downloader (`crates/sayso-update/src/download.rs`) starts again from zero and deletes the partial file after an error. See "Download".
3. **A cleanup model is a provider**, the same as Apple Intelligence: the user can make it the default provider, and a style can name it. In memory it is `ProviderKind::CleanupModel { model }`. On disk it is not in `[[ai.providers]]`. See "Config and stored data" for the reason.
4. **Tone is a setting of Cleanup 1 only.** A style stores `tone` as `very-casual`, `casual`, or `formal` (the default). Cleanup 1 gets the same value. S1-mini always gets `semi-formal`, and its editor shows no Tone. The evaluation showed why: on S1-mini, `casual` and `semi-casual` keep filler words and spoken corrections ("hey um are we still on … at noon? no wait at 1"), and `semi-formal` and `formal` gave the same text in every case. On Cleanup 1, the three tones change only capital letters and the final period, and the cleanup stays the same.
5. **Layout and format are needs, not hints.** `layout = "list"` and `format = "email"` need a model that has them. Only S1-mini has them. So Email and Notes run on S1-mini and not on Cleanup 1.
6. **Context.** Sayso gives Cleanup 1 the app name and the site of the dictation (`Session.app.name`, `Session.site`). Both stay on this computer. There is no setting for this. `before` and `after` wait until Sayso reads the text around the cursor, which it does not do today.
7. **Language: run only when the language is known and supported.** See "Language".
8. **Polished needs a provider that reads prompts.** Neither model fixes grammar. In the evaluation, "me and him was going" and "the new onboarding flow are" stayed on both models, and Cleanup 1 also kept "buyed". So Polished gets no cleanup settings.

## Which styles each model runs

| Style | Cleanup settings | S1-mini | Cleanup 1 |
|---|---|---|---|
| Clean | formal | yes | yes |
| Polished | none | no | no |
| Rambling | none | no | no |
| Message | casual | yes | yes |
| Email | formal, email | yes | no |
| Notes | formal, list | yes, when the text is a list (see Evaluation) | no |
| Raw | none, no prompt | nothing runs | nothing runs |
| A user style | as its file says | when set and supported | when set and supported |

`CleanupModelInfo::runs(style)` is true when the style has cleanup settings, every value is known, and the model has every layout and format value that they ask for. The Styles page, the editor, the provider menu, and the pipeline all use this one function.

## Style files

```toml
id = "email"
name = "Email"
prompt = "Format this as an email body. ..."

[cleanup]
format = "email"
```

- `layout` is `paragraphs` (default) or `list`. `format` is `plain` (default) or `email`. `tone` is `formal` (default), `casual`, or `very-casual`.
- The values are strings in the Rust type, not enums. A value that this version does not know does not break the style file. It makes `runs` false for every model, so a newer file runs as "cleanup only" in an older Sayso.
- The `cleanup` key has three states:
  - **Absent.** A user style has no cleanup settings. A file that overrides a built-in style gets the built-in settings, so the overrides of today keep working on a cleanup model.
  - **A table** (`[cleanup]`, also an empty one). These settings.
  - **`cleanup = false`.** No cleanup settings, also for an override of a built-in style.
- The editor writes the table when the user saves a style while a cleanup model is its provider. It writes nothing for the other providers, so a file stays as it was.
- **Downgrade.** A Sayso from before this change ignores the `cleanup` key. When the user saves that style in the older version, the key is lost. Nothing else is lost.

## When a style uses AI

Today `Style::uses_ai()` is "the prompt is not empty". The pipeline (`pipeline.rs:39`, `pipeline.rs:49`), `apply_style` (`dictation.rs:463`), and the History page (`history.rs:729`) use it. A user style with cleanup settings and no prompt would then skip cleanup.

- `Style::is_raw()`: no prompt and no cleanup settings. Raw stays the style with no AI, on every provider.
- `Style::can_run(provider) -> RunPlan` replaces `uses_ai()` at every call site. It is one of:
  - `Prompt`: a provider that reads prompts, and the prompt is not empty.
  - `Cleanup(settings)`: a cleanup model that runs the style.
  - `CleanupOnly`: a cleanup model that does not run the style. Cleanup with the default settings, and a notice.
  - `Nothing(reason)`: Raw; a provider that reads prompts and an empty prompt; the language is not supported; the model file is not on disk.
- The model and the temperature of a style are for providers that read prompts. A cleanup model ignores them. The timeout of a style applies to both.
- History › Enhance again and Settings › Providers › Run test use the same `RunPlan`. Run test on a cleanup model runs Clean on a fixed English sentence.

## Language

A cleanup model runs only when Sayso knows the language and the model supports it. Sayso does not guess.

1. The dictation language is a language code: that language.
2. The dictation language is "auto" and the final model knows one language only (for example Parakeet Unified EN): that language.
3. Otherwise, a text language detector (`whatlang`, pure Rust) on the text after replacements. Its result counts only when it is reliable (`Info::is_reliable()`). A short text is often not reliable.
4. No reliable language: no cleanup. The text after replacements goes in, with the notice "Sayso could not tell the language, so it did not clean up this text."

S1-mini supports `en`. Cleanup 1 supports `en`, `de`, `es`, `fr`, `zh`, `ru`, and `ja`. The language goes into `CleanupRequest`. The check runs in `sayso-cleanup-models` (`sayso-core::cleanup` re-exports it), before the request goes to the runtime. Mixed-language text is not reliable, so it does not run.

A detected language from the engines (a `language` field on `Transcript`) is later work. It would make step 3 rare.

## Pipeline

The pipeline stays: transcribe → replacements → spoken punctuation → style → insert. The style step gets a second kind of runner.

- `sayso-cleanup-models` (MIT, pure, no I/O; `sayso-core::cleanup` re-exports it, see ADR 0002):
  - `CleanupModelInfo`: id, display name, publisher, file, revision, size, SHA-256, license files, languages, tones, layouts, formats, context keys, the input token limit, the output reserve, and the end markers.
  - `CleanupRequest { text, language, settings, app, site, timeout }`.
  - `render(model, request) -> RawPrompt { text, end, max_tokens }`. It writes the exact input format of each model:
    - S1-mini: `<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n[Styling: semi-formal] [Structure: {prose|lists}] [Context: {general|email}]\n{text}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n`. The end is the token `<|im_end|>` or the end-of-generation token.
    - Cleanup 1: `app: {name}\nsite: {host}\ntone: {tone}\ntext: {text}\nout:`, with the lines that have no value left out, in the order of the model card. The end is the end-of-text token. The answer starts with a space, which Sayso removes.
  - `plan_chunks(model, text, count_tokens) -> Vec<Chunk>`. See "Long text".
  - `check(model, request, answer) -> Result<String, EnhanceError>`. See "Checking an answer".
- `trait Cleaner: Send + Sync { fn id(&self) -> &str; fn clean(&self, request: &CleanupRequest) -> Result<CleanupResponse, EnhanceError>; }`. A cleanup model is never cloud.
- `TextPipeline` takes `Runner::Enhancer(&dyn Enhancer)` or `Runner::Cleaner(&dyn Cleaner)`, and runs the `RunPlan`.
- `apply_style` in `dictation.rs` passes the app name, the site, and the language to the pipeline. It does not do this today.
- **The rules for one dictation:**
  1. `Cleanup(settings)`: clean with the style's settings.
  2. `CleanupOnly`: clean with the default settings. The overlay says "Rambling needs a provider that reads prompts. Sayso did a cleanup only."
  3. `Nothing(language)`: no cleanup. The text after replacements goes in. The overlay says "S1-mini knows English only." or the notice of "Language".
  4. The model file is not on disk, the runtime fails, the time runs out, or `check` fails for any part: the text after replacements goes in, the whole text and not a mix of cleaned and not cleaned parts, with the reason, as for every provider failure today.

### Long text

The runtime counts tokens with the tokenizer of the loaded file (`count` request). Sayso counts the whole rendered prompt, not only the text.

- S1-mini: the text of one part is at most 1,000 tokens. `max_tokens` is 1.3 × the text tokens + 32.
- Cleanup 1: the prompt and the answer together are at most 4,096 tokens. `max_tokens` is 1.3 × the text tokens + 32, and the prompt is at most 4,096 − `max_tokens`. That is about 1,750 text tokens.
- A text over the limit splits at the last sentence end (`.`, `?`, `!`, `。`, `？`, `！`, or a new line) before the limit. With no sentence end, it splits at the last comma, then at the last space, then (for Chinese and Japanese text) at a character. Every character of the input is in one part, and each separator stays with the part before it.
- The parts go to the model one at a time. Sayso joins the answers with one space, or with the new line that was the separator.
- A correction that goes over a split ("on tuesday. no wait wednesday") can stay. This is a known limit. A normal dictation is far below the limit: the longest case in the evaluation was about 120 tokens.
- One deadline covers all parts.

### Checking an answer

The guarantee that text is never lost comes from the end check, not from a length check.

1. **End.** The runtime says why it stopped: `end` (the end marker), `limit` (`max_tokens`), `context` (the context is full), or `cancelled`. Only `end` passes. Anything else is a failure.
2. **Clean the answer.** Remove the end marker and any control token text (`<|im_start|>`, `<|im_end|>`, `<think>`, `</think>`). Trim white space.
3. **Empty answer.** An empty answer passes only when every word of the input is in the filler list of its language (English: um, uh, er, ah, hmm, mm, you know, like, so; German: äh, ähm, hm; French: euh, bah; Spanish: eh, este, pues; and the same idea for the other languages of Cleanup 1). Then Sayso inserts nothing. In all other cases, an empty answer is a failure.
4. **Too short.** An answer with fewer than 25% of the letters and digits of the input fails, for an input with 40 or more letters and digits. This counts characters, not words, so it also works for Chinese and Japanese, and a spoken number ("nine hundred and ninety nine") that becomes digits does not trip it. It is a safety net with a loose limit, not a promise. The evaluation set tunes it.
5. **Too long.** The runaway check of today (`pipeline.rs`, `is_runaway`).

### Dictionary words after the model

The models take no word list. Replacements and spelled words run before the model, as today. After the model, Sayso puts back a spelling that the model changed:

- The terms are the dictionary words and the targets of the replacements that ran (for example "GitHub").
- A term is put back only when the text that went to the model had the term with its exact spelling, and the answer has the same letters with another case, as a whole word (Unicode word boundaries). So "us" in the input never becomes "US".
- Longer terms first. A term inside a longer term that was already put back is not changed.
- When the tone is very casual and the term has a capital letter, the term still gets its spelling back. A name stays a name.

## Runtime and protocol

- `sayso-cleanup` speaks JSON lines on stdin and stdout, with the same frame as the engines: every request has `v` (the protocol version) and `id`, and every reply has the `id` of its request.
  - `hello` → `{version, llama_cpp, backend}` (`metal` or `cpu`).
  - `load {model_id, path, sha256, n_ctx}` → `{model_id, load_ms}`. The runtime checks nothing about the file except that llama.cpp can load it. The app checks the hash before it sends `load`.
  - `count {model_id, text}` → `{tokens}`.
  - `complete {model_id, prompt, end, max_tokens}` → `{text, stop: "end" | "limit" | "context", tokens_in, tokens_out, ms}`.
  - `unload` → `{}`.
  - An error reply has `code` (`not_loaded`, `wrong_model`, `load_failed`, `decode_failed`) and `message`.
- One request at a time. The client sends a request only after the reply to the one before. `complete` and `count` name the model. If another model is loaded, the reply is `wrong_model`, and the client sends `load` again.
- **Cancel.** llama.cpp has no cheap way to stop one decode from outside. So the client kills the process when the deadline passes, waits for it to exit, and starts it again at the next use. A late reply cannot reach a new request, because the process is gone.
- **Load at the start of a dictation.** When a dictation starts and its style will use a cleanup model, the client sends `load` while the microphone records. This is a hint: when the user changes the style during the recording (`cycle_style`), the request at the end names the right model, and the runtime loads it then.
- **Load time.** `load` has its own limit of 15 s, because the first start on a Mac compiles the Metal shaders (about 4 s in the spike). The deadline of the style step (the HTTP timeout of today, 4 s, or the style's timeout) starts when the text is ready, not at the load. If the load is still running at the end of the recording, the style step waits for it within its own deadline.
- **Stop.** The client stops the process after 10 minutes with no request, before it deletes a model file (Windows locks open files), and when the user turns AI off. This follows `memory.rs`.
- **Memory.** The spike measured about 1.25 GB resident on macOS with a 4,096-token context. The client uses the smallest context that the request needs (S1-mini: 2,048), and the work item measures memory again with a speech model loaded.

## Download

A new module in the app, not the updater's downloader:

- The file goes to `<models dir>/cleanup/<id>/<file>.partial`. A retry sends `Range: bytes=<size of partial>-`. A `206` reply with a matching `Content-Range` continues the file. A `200` reply starts the file again. Any other reply is a failure.
- A partial file from another revision (the catalog keeps the revision next to it in `<file>.partial.json`) is deleted.
- After the last byte, Sayso hashes the whole file. A wrong hash deletes the file and shows an error.
- LICENSE and NOTICE are downloaded from the same pinned revision before the model file is marked ready.
- **Ready** is one atomic step: rename the `.partial` file, then write `ready.json` with the revision and the hash. A folder with no `ready.json` is not ready, also when the file is there.
- One download per model at a time. Delete during a download cancels the download first. Delete of a loaded model stops the runtime first.

## Config and stored data

**Why not `[[ai.providers]]`.** `Config::from_sections` (`config.rs:782`) parses each section in one piece. An older Sayso that finds a provider with an unknown `kind` fails the whole `[ai]` section and uses the default: no providers, no style rules, AI off. The next change in Settings saves that default (`edit_config`, `model.rs:469`). So a downgrade would lose all providers and style rules.

- Cleanup models go into their own list, which an older Sayso ignores:

  ```toml
  [[ai.cleanup_models]]
  id = "s1-mini"
  name = "S1-mini"
  model = "s1-mini"
  ```

- In memory they are `Provider { kind: ProviderKind::CleanupModel { model } }` in `ai.providers`, so the rest of the code sees one list. `Ai` has its own `Serialize` and `Deserialize` that move them to `cleanup_models` and back.
- **Downgrade, exactly:** an older Sayso keeps every other provider and the style rules. `ai.default_provider` or a style can name the id of a cleanup model. For the older Sayso that provider does not exist, so those styles insert the text after replacements, as with a removed provider today. When the older Sayso saves the config, `cleanup_models` is lost. The model files stay on disk, and the user adds the model again in one step.
- **Later kinds.** `ai.providers` changes to read each entry on its own. An entry with a kind that this version does not know stays in the file as it was (kept as a TOML value, written back on save), and it does not run. So a later new kind does not break this version.
- **Backup.** Before the first save that writes `cleanup_models`, Sayso copies `config.toml` to `config.toml.before-cleanup-models` one time.
- Update every exhaustive `match` on `ProviderKind`:
  - `config.rs`: `name`, `model`, `thinking`, `set_model`, `is_cloud`.
  - `sayso-enhance/src/lib.rs`: `build_enhancer`, which becomes `build_runner` and returns an enhancer or a cleaner.
  - `sayso-enhance/src/models.rs`: `list_provider_models` (none for a cleanup model).
  - `hub/pages/ext.rs`: `provider_detail`.
  - Also review the partial matches in `dictation.rs`, `model.rs`, `hub/settings/providers.rs`, and `onboarding/ai.rs`.
- **History.** No new `EnhanceOutcome` variant, because an older Sayso cannot read one (`sayso-store/src/history.rs:84`). A cleanup run is `Applied { provider_id, model, elapsed_ms }`. Add `#[serde(default)] note: Option<String>` for the "cleanup only" case. An older Sayso ignores the field. `EnhanceTimings` gets `#[serde(default)]` fields for the load time, the token counts, the stop reason, and the number of parts.

## What the user sees

The design is on the Paper page "Cleanup models · S1-mini":

- **1a, 1b. Settings › Providers.** S1-mini and Cleanup 1 are rows, each with a **No prompts** badge. An open row shows:
  - which styles the model runs;
  - the languages;
  - the model file, with Show in Finder;
  - Run test, Make default, and Delete download.

  Before the download, a row shows Download and the size, with a progress bar while it downloads.
- **2a, 2b, 2c. Styles.** The route column says `Local · S1-mini` for a style on a cleanup model that is not the default. It says `S1-mini can't run this style` in red when the provider of the style cannot run it. The featured style is one that the default provider can run.
- **3a, 3b, 3c. Style editor.** With a cleanup model as the provider, the prompt folds away, and the editor shows the settings that the model has:
  - S1-mini: Layout and Format as an email.
  - Cleanup 1: Tone (Very casual, Casual, Formal) only.

  In the provider menu, a cleanup model that cannot run the style is grey and shows "No prompts".
- **4. Onboarding step 3.** S1-mini and Cleanup 1 are the first two options, with their sizes. "Download and continue" starts the download, and onboarding goes on. Until the file is ready, a dictation inserts the text after replacements, and the overlay says that the model is downloading.
  - The preselected option follows the dictation language of step 2: English selects S1-mini; German, Spanish, French, Chinese, Russian, or Japanese selects Cleanup 1; other languages select nothing.
- **Attribution.** S1-mini always shows as "S1-mini" with "by Superwhisper". Settings › About lists both models with their licenses.

All new strings go into the English catalog and the 9 other catalogs.

## Runtime spike

Run on 2026-10-21 on the M5 Pro (`~/.cache/sayso-cleanup-eval/spike`, `llama-cpp-2` 0.1.160 with `metal`):

- The release binary is 10 MB and links only system frameworks.
- The first start on this Mac compiled the Metal shaders in about 4 s. After that, a load took 370–430 ms.
- One request took 100–320 ms, with a new context each time. Both models gave the same text as `llama-server`.
- About 1.25 GB resident with a 4,096-token context.

Still to do, before any UI work (work item 2):

- Build and run on every target that Sayso ships: macOS arm64, Windows x86_64, Linux x86_64, and Linux arm64. Windows and Linux run through a PR (no local VMs).
- The CPU instructions that the Windows and Linux builds need, and their speed on a CPU with no GPU.
- That Metal really offloads the layers in the signed, installed app bundle.
- Default features off, so no OpenMP.
- A cold start, the longest input, and memory with a speech model loaded.

## Work

1. **Evaluation.** Done once with `llama-server` (see Evaluation). In the build, add a `cleanup` provider branch to `sayso-enhance/tests/style_eval.rs` (`SAYSO_EVAL_PROVIDER=s1-mini` or `spokenly`) that runs through `TextPipeline`, so the spelled-word step and replacements count. Add cases in German and Spanish for Cleanup 1, and list cases for S1-mini.
2. **Runtime.** The crate and binary `sayso-cleanup` with the protocol, a CI job that builds it for every shipped target, and the open spike checks. This is a gate: no UI work before it passes.
3. **Core.** `sayso-core::cleanup` (catalog, `render`, `plan_chunks`, `check`, `runs`, language rules, dictionary spelling), `Style::cleanup`, `RunPlan`, and the config changes with their round-trip tests.
4. **Client and download.** The runtime client in the app (start, load hint, deadline, kill, idle stop) and the downloader.
5. **Pipeline.** `Cleaner`, `Runner`, the rules, history, and diagnostics. History › Enhance again and Run test.
6. **UI.** Providers, Styles, editor, provider menu, onboarding, overlay notices, About, and the strings in 10 catalogs.
7. **Packaging.** Add the binary to the 12 release files, macOS signing and notarization, Flatpak, AUR, the Nix flake, and the Homebrew tap.
8. **Docs.** `CONTEXT.md`, the NOTES files of the touched crates, and the user docs.

## Checks

- `render`: golden strings that match the formats in the model cards, byte for byte.
- `check`: a stop at `limit` or `context` fails, also with a long answer. An empty answer for "um uh" passes, and for a sentence it fails. A spoken number that becomes digits passes. A Chinese or Japanese answer that drops half the text fails. Control token text is removed.
- `plan_chunks`: a text with no sentence end, a long Chinese text with no spaces, text with new lines, and a text just over the limit. Every input character is in exactly one part.
- A failure in the second of two parts inserts the whole text after replacements.
- Language: "auto" with an English-only final model runs. "auto", a short text, and a multilingual final model does not run. German on S1-mini does not run. German on Cleanup 1 runs.
- `runs`: the table "Which styles each model runs" is a test. An unknown `layout` value makes `runs` false and does not break the file.
- Style files: absent, a table, and `cleanup = false`, for a user style and for an override. A style with cleanup settings and no prompt runs on a cleanup model. Raw never runs.
- Dictionary spelling: "us" stays, "Gpui" becomes "GPUI", "github" becomes "GitHub" after a replacement, overlapping terms, and a very casual tone.
- Config: load, edit, and save a config with a cleanup model, then load it again. A config from today loads with no change. A config with an unknown provider kind keeps that entry through a save. The backup is written one time.
- Runtime: a timeout kills the process and the next request works. `wrong_model` loads again. A crash inserts the text after replacements. Delete stops the runtime first.
- Download: a wrong SHA-256 deletes the file. An interrupted download resumes. A server that answers `200` to a range request starts again. No `ready.json`, no model.
- Old history rows load. New rows load in the version before this change.
- The style evaluation passes for each style that a model runs, on both models, before release.
- A live dictation on macOS, and a run on Windows and Linux through a PR (no local VMs).

## Open questions

1. Does Cleanup 1 on macOS later use the Core ML version that Spokenly ships? The plan uses the same GGUF with Metal everywhere, for one path. Measure Core ML only if Metal is too slow.
2. The 25% limit of "Too short" and the filler lists. The evaluation set tunes them.

## Changes during the build

- **Context and part size.** The runtime loads a model with a context of 2,048 tokens (`cleanup::N_CTX`), and a part has at most 700 text tokens (`MAX_PART_TOKENS`), for both models. A part, its prompt, and the longest answer (1.3 × 700 + 32) fit, and the memory of the context is half of 4,096.
- **Deadline.** A fixed deadline does not fit a model whose time grows with the text: about 2,400 tokens took 10.7 s on the M5 Pro. The deadline of a cleanup is the style timeout plus 15 ms for each text token (`cleaner::TIME_PER_TOKEN`), counted before the first part. A load has its own limit (15 s) and does not count.
- **History.** No `note` field on `EnhanceOutcome::Applied`: adding a field there touched 9 places. The cleanup details (load time, parts, tokens, language, "cleanup only") are in `EnhanceTimings.cleanup`, which an older Sayso ignores. The overlay notice comes from `PipelineOutput.notice`.
- **Too short.** The check counts content units, not characters: a word, a Chinese, Japanese, or Korean character, or a run of number words or digits ("nine hundred and ninety nine" and "999" are one unit each). It fails an answer with fewer than a quarter of the units of an input with 10 units or more. A character count failed on spoken numbers.
- **Language notices.** "{model} does not know {language}, so the text is not cleaned up." and "Sayso could not tell the language, so the text is not cleaned up."
- **Through the pipeline.** With the spelled-word step and the replacements of the app (`style_eval.rs`, `SAYSO_EVAL_PROVIDER=s1-mini` or `spokenly-cleanup-1`): S1-mini 41 of 51, Cleanup 1 41 of 47. Spelled words now pass. S1-mini Notes made no list for the two Notes cases of the set (it makes a list only when the text clearly is one).
- **Loops.** The same sentence 30 times made S1-mini repeat until its limit. The end check stops it, and the text after replacements goes in (`tests/real_models.rs`).
- **A repeated paragraph.** S1-mini wrote a text that had the same paragraph twice only one time. The length check did not catch it (more than half stayed). This is a limit of the model.

## Review record

Codex on GPT-6-Astra reviewed the first version of this plan on 2026-10-21. All 11 findings are accepted:

| # | Severity | Finding | Where it is fixed |
|---|---|---|---|
| 1 | Blocker | A cut-off answer could pass the length check | "Checking an answer": the stop reason |
| 2 | Major | `uses_ai()` needs a prompt, so a style with cleanup settings only would skip cleanup | "When a style uses AI" |
| 3 | Major | An unknown provider kind resets the whole `[ai]` section in an older Sayso, and a save keeps the loss | "Config and stored data" |
| 4 | Major | The protocol had no ids, no version, no cancel, no model name | "Runtime and protocol" |
| 5 | Major | No token count, and no split for text with no sentence end | "Long text" |
| 6 | Major | A link test does not prove that llama.cpp can live in the engines | Decision 1 and "Runtime spike" |
| 7 | Major | An unsure language ran on S1-mini | "Language" |
| 8 | Major | The inheritance of cleanup settings was not defined | "Style files" |
| 9 | Major | A word-count check fails on numbers and on Chinese and Japanese | "Checking an answer" |
| 10 | Minor | The spelling fix for dictionary words had no boundaries | "Dictionary words after the model" |
| 11 | Minor | The updater's downloader cannot resume | "Download" |

### Code review

Codex on GPT-6-Astra reviewed the code in three rounds on 2026-10-21.

- **Round 1: 9 findings, all fixed.**
  - An empty answer (only filler words) still went to the insert. It now ends like a dictation with no speech: no paste, no keys, no Enter.
  - History used the language and the speech model of today, not of the entry.
  - A failed model switch left the client with a wrong loaded model.
  - Count errors did not stop the split.
  - "Delete download" blocked the UI and did not exclude a running load.
  - The spelling fix changed "may" when the text had "May" too.
  - History could not run a style with cleanup settings and no prompt.
  - The runtime stayed loaded after AI was turned off.
  - A zero timeout still had a 15 s limit.
- **Round 2: 3 fixes were partial, and there were 2 new findings. All fixed.**
  - The counts now share the cleanup deadline.
  - The block of a model that is being deleted, and the off switch, are checked under the lock of every load.
  - Live cleanup uses the speech model of the final pass.
  - A failed delete shows on the provider row.
- **Round 3:** every fix is complete and there are no new defects. Verdict: ready to commit.
