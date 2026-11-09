# Refiner and Sayso models: plan

Status: proposal, written on 2026-11-05. Chris answered the decisions of section 13 on the same day, chose the model names (decision 8), set the order of the work (decision 9), put real speech first (decision 10), chose Deepgram for live words (decision 11), and launched the collection page before the lawyer's review (decision 12). The rest is not agreed. Phase 0, the collection page, is live at https://justsayso.app/voices. Nothing else is built. **To continue this work, start at section 15:** it says what is built, where each part is, and what to do next. The terms of today are in [CONTEXT.md](../CONTEXT.md). New terms are in section 14. They go into `CONTEXT.md` when this plan is agreed.

## 1. Goal

Sayso writes names, places, acronyms, paths, and code symbols correctly, because it knows the context of each dictation. It does this on the user's computer, on hardware that most people have. After a dictation, the user does not touch the keyboard.

The plan has four parts:

1. **Real speech.** A collection page on the Sayso website, with a game, where people record the speech that the models need. It comes first, and it is the only source of audio.
2. **The refiner.** A new crate that does the work that a model does badly. It collects context, finds candidates, checks them against the audio, and guarantees that the final text contains no invented words.
3. **Sayso models.** Three text models in three sizes that work with the refiner (Sayso Solo, Sayso Quartet, and Sayso Choir), and later a speech model that hears context.
4. **The data factory and the Dictation Bench.** A pipeline that makes training data, and a public benchmark. With them, Sayso can train a new model each time a better base model comes out, and it can prove that it beats the competition.

Success: on the Dictation Bench (section 8), local Sayso beats S1-mini and Cleanup 1 in every category. It also closes most of the gap between the raw transcript and a large cloud model that sees the full context. Section 11 has the gates.

## 2. The competition

| | S1-mini (Superwhisper) | Cleanup 1 (Spokenly) | Avalon (Aqua Voice) |
|---|---|---|---|
| Runs | Local | Local | Cloud |
| What it is | Cleanup model, Qwen3-0.6B | Cleanup model, Qwen3-0.6B-Base | Speech model, trained on prompts, code, and email |
| Context | None | App, site, text before and after the cursor | Reads the active window |
| Reads a prompt | No | No | Not a text model |
| Languages | English | 7 | Not checked |

The facts for S1-mini and Cleanup 1 come from [cleanup-models.md](cleanup-models.md). The facts for Avalon come from Aqua's own pages. Aqua reports 97.4% key-term accuracy on AISpeak-10, its own benchmark, against 65.1% for Whisper large-v3. Nobody has tested this independently.

The gap: no product does context-aware recognition locally, and no product can prove that its AI did not change the meaning. Sayso can do both, because it owns its engines, its cleanup runtime, and its pipeline.

## 3. Why more context makes a small model worse

A small model that gets context as text must do two jobs: find the 1% of the context that matters, and rewrite the transcript. It does the first job badly. It copies context words into the wrong places, it follows instructions that are in the context, and it changes correct words.

Sayso already has evidence of this:

- The macOS engine boosts Dictionary words with CTC keyword spotting (`native/macos/SaysoEngine/Sources/SaysoEngine/VocabularyBooster.swift`). When the acoustic rescue was on, the sound score alone changed ordinary words: "macOS" became "MayFirmOS", and "distros" became "Pindrop". This happened with a short Dictionary. The fix was to turn the rescue off and require a spelling similarity of 0.60.
- In the cleanup model evaluation, `app: Slack` and `app: Mail` made no difference to Cleanup 1.

So one signal is not enough, and raw context does not help a small model. The design follows from these two facts:

- **Context is a list of candidates, not text.** The model never sees raw screen text.
- **Code finds the candidates.** The number of candidates depends on the number of doubtful words, not on the amount of context.
- **Each decision uses several independent signals:** the sound of the audio, the sound of the transcript, the fit in the sentence, the source of the candidate, and the history of the user.
- **The model chooses. It does not write.** Constrained decoding makes it impossible for the model to output a word that the refiner did not allow.

## 4. The design

```
start of dictation -> context snapshot -> context words --------------+
                                                                      |
audio -> engine: transcript, word times, CTC log-probabilities -------+
                                                                      v
              refiner: sound match, spotting, resolvers -> lattice
                                                                      |
                 +----------------------------------------------------+
                 |                                                    |
                 v                                                    v
     decision without a model                     Solo, Quartet, or Choir, constrained to the lattice
     (calibrated classifier)                      (choice and cleanup in one pass)
                 |                                                    |
                 +---------------------> check -> style -> insert <---+
```

The refiner runs for every dictation. The model is an upgrade. Without the model, the refiner still fixes entities, paths, numbers, and Dictionary words, and the style runs as it does today.

## 5. The refiner

The refiner is the engine of this plan in the everyday sense of the word. It is not called an engine, because in `CONTEXT.md` the engine is the sidecar that runs speech models.

Two new crates:

- **`sayso-refine`**: pure logic with no I/O, like `sayso-cleanup-models`. It has the context words, the spoken forms, the phonetic index, the resolvers, the lattice, the features, the decision, the check, and the explanations. Its tests run on fixtures. License: Apache-2.0 with a NOTICE file (section 13, decision 1). It depends only on crates with a permissive license. With no I/O, it also compiles to WebAssembly, so the collection page (section 7.1) can run the same code.
- **`sayso-context`**: I/O. It has the context snapshot, the context adapters, and the intake from connected apps. It uses `sayso-platform-*` for accessibility. Those crates are GPL-3.0-only, so `sayso-context` stays GPL-3.0-only too. An app that uses `sayso-refine` without Sayso brings its own context, or uses the MIT types of `context.provide`.

### 5.1 Context snapshot

When the dictation starts, Sayso collects context in the background. The user speaks for several seconds, so this adds no wait.

| Source | macOS | Windows | Linux |
|---|---|---|---|
| Text before and after the cursor | AX | UI Automation | AT-SPI, limited |
| Title and visible text of the focused window | AX | UI Automation | Limited |
| App and site | Yes (today) | Yes (today) | App only (today) |
| Clipboard | Yes | Yes | Yes |
| OCR of the window, when it gives no accessibility text | Vision | Windows.Media.Ocr | Later |

Sayso does not read the text around the cursor today. Cleanup 1 already accepts it as `before` and `after`.

Rules:

- Sayso never reads password fields or secure input.
- The user can turn off each source, and context for each app.
- The snapshot stays in memory for one dictation. Sayso never stores it, and it never sends it to a cloud provider.
- A style provider that reads prompts can get the matched context words (about 20 at most), never the snapshot. This is a setting.

### 5.2 Context adapters

An adapter knows where the useful words of one kind of app are. Adapters are code, not AI.

| App kind | Context words from | Resolver |
|---|---|---|
| Terminal | Working folder of the shell, file tree (with `.gitignore`), git branches | Spoken path to a path on disk |
| Code editor | Project tree, symbols of the open files (tree-sitter) | Symbol spelling (`useEffect`, `get_user_by_id`) |
| Chat | Names of the members, channel names | None |
| Mail | To and Cc names, subject, thread | Spoken email address |
| Browser | Title, headings, and visible text of the page | Spoken URL |
| Calendar (opt-in) | People in the current or next event | None |
| Any app | Text near the cursor, window title | None |

Connected apps can send context words through the Host API with a new capability, `context.provide`. An editor extension sends its symbols. A terminal sends its working folder. This is better than reading the screen, because the app knows its own data.

Spike: get the working folder of the shell in each common terminal, on each system.

### 5.3 Context words

A context word is one entry with a written form, a kind, a source, and one or more spoken forms.

Only rare words become context words. The filter is "rare in general, present here". A word in the most common 50,000 words of the language is not a context word, because the speech model already knows it. An adapter or the Dictionary can still mark a common word (for example a person named Will). For such a word, the "keep" alternative wins unless the sound and the fit both support the change.

Rules for each kind make the spoken forms:

- Symbols: split camelCase and snake_case. `getUserById` becomes "get user by id".
- Paths: `src/lib.rs` becomes "source slash lib dot r s" and "s r c slash lib dot rs".
- Acronyms: `API` becomes "A P I". `SQL` becomes "sequel" and "S Q L".
- Versions: `v0.15.0` becomes "version zero point fifteen" and "v zero fifteen zero".
- Learned forms: the words that the user said for this entry before.

Dictionary words, learned words, and context words all use the same matcher.

### 5.4 Sound match

Speech errors are phonetic: "cooper netties" for Kubernetes, "a mirror" for Amira. Embedding search does not find these errors. The refiner compares sounds, not meanings.

- A G2P (letters to sounds) gives the phonemes of each spoken form and of the transcript. For English: CMUdict, plus a small G2P model for words that are not in it.
- An index of phoneme n-grams finds near matches fast, also with 100,000 context words.
- Matching runs in two directions. From the transcript to the context words, it finds spans that look wrong ("cooper netties"). From the context words to the transcript, it finds names that hide in common words ("ask a mirror about"). The second direction is cheap, because the list of context words is short after the rarity filter.
- The live preview gives partial text while the user speaks, so matching starts before the recording stops.
- The costs of a substitution come from a confusion table that the data factory learns (section 7): which phonemes the speech models of Sayso mix up.

Phase 1 target (Gate 1): the correct entity is in the top 10 for its span in at least 90% of cases, and this does not fall when the number of context words grows from 50 to 5,000. SpellMapper reports up to 90% with 10 candidates on Spoken Wikipedia.

### 5.5 Acoustic evidence

The transcript loses information that the audio still has. Parakeet TDT-CTC 110M is in the model catalog, and the macOS engine already uses it as the vocabulary helper. Its CTC output gives a probability for each token in each audio frame.

New optional engine feature: `ctc_logprobs`. The engine returns the CTC log-probabilities of the helper model for the dictation. The refiner then does two things in Rust, once for all systems:

- **Spot:** find context words in the audio with CTC-based word spotting (CTC-WS, Andrusenko et al., Interspeech 2024).
- **Score:** compute how well each candidate fits the audio of its span, with forced alignment.

The engine gives evidence, and the refiner decides. Section 3 shows why the acoustic score must never decide alone.

Spike: the size of the data (about 375 frames of about 1,025 values for 30 s of audio; the top values of each frame can be enough), and how to get the log-probabilities from FluidAudio and from the ONNX model in the Rust engine. The Rust engine already loads NeMo CTC models through sherpa-onnx, but it may need `ort` directly for the raw output.

### 5.6 Resolvers

Some text has one correct written form that code can compute. A resolver gives that form as a candidate, not as a fixed change.

- **Paths:** match the spoken path against the real file tree, one segment at a time. A path that is not on disk is not a candidate.
- **Symbols:** match against the symbol index, with the exact spelling.
- **Numbers, dates, times, money, units, versions, phone numbers:** inverse text normalization (ITN) rules for each language. The NeMo text processing grammars are a reference.
- **Email addresses and URLs.**
- **Spelled words and spoken punctuation:** these steps exist today. They become resolvers, with the same behavior, and the existing tests prove it.
- **Voice commands:** "new paragraph", "scratch that".

### 5.7 The lattice

The lattice is the set of all outputs that the refiner allows for one dictation. It is a sequence of slots over the transcript. Each slot has alternatives:

- Keep the words, with or without a capital letter.
- A context word or a resolver output, with its features.
- Delete. This is only possible for filler words and for the words that a self-correction replaces ("Thursday, no wait, Friday").

Between slots, the output can add punctuation, spaces, new lines, and list markers.

The lattice compiles to a llama.cpp grammar (GBNF). The pinned `llama-cpp-2` 0.1.160 has `LlamaSampler::grammar`. So the model can only output a path through the lattice. It cannot invent a word, and it cannot drop a word that has no delete alternative. This is the guarantee: **for each change, Sayso can show where it came from.**

Each alternative has features: the sound match, the acoustic score, the spot score, the fit score, the source, the prior, the rarity, and the personal history.

### 5.8 The decision

Both ways use the same lattice.

1. **Without a model.** A calibrated classifier (logistic regression or small gradient-boosted trees) reads the features. The data factory trains it, and it runs in microseconds. It changes a slot only when the probability is over a threshold. It runs on every computer, also when no model is on disk. Then the style runs as it does today, and the refiner checks that each chosen entity is still in the output. If the style lost an entity, Sayso puts it back when the alignment is clear. Otherwise it inserts the text from before the style, with the reason.
2. **With a Sayso model (Solo, Quartet, or Choir).** The model walks the lattice and does the cleanup in the same pass: filler words, punctuation, capital letters, layout, and format.

The costs are not equal. A missed fix leaves the text as it is today. A wrong fix makes the user stop trusting Sayso. So the thresholds keep the false-correction rate under the target (section 8), even when this loses some fixes.

Fast path: when there is no suspect span and no resolver output, the refiner calls no model.

### 5.9 Memory

Code can learn at once and locally. A model cannot.

- **Learned words.** After the insertion, Sayso reads the field again for a short time. If the user changed a word to one that sounds like it, Sayso adds a learned word for that app. The user sees learned words in the Dictionary and can remove them.
- **Undo teaches.** If the user undoes a change of the refiner, that candidate gets a lower prior in that app.
- **Personal history.** A small n-gram model over the final texts in History gives one more feature: how often this user writes this word in this app. Clearing History clears it, and Incognito dictations do not go into it.

### 5.10 Explanations

Each change records its reason, for example "Kubernetes: from the screen, sound match 0.92". History shows the changes as marks, with **Undo this change**, **Always**, and **Never**. A model cannot explain its changes reliably. The refiner can, because each change is a lattice alternative with a source.

## 6. The models

### 6.1 Solo, Quartet, and Choir (text, phases 2, 3, and 5)

Three cleanup models in one family, in three sizes. Each one does what S1-mini and Cleanup 1 do, and also chooses between candidates, in one pass over the lattice.

| Model | Base size | For |
|---|---|---|
| Sayso Solo | Under 1 B | The default. Runs on the floor hardware. Target: a 4-bit file under about 1 GB |
| Sayso Quartet | About 1.7 to 2 B | Computers with more memory |
| Sayso Choir | About 4 B | Macs with 16 GB or more, and NVIDIA GPUs. It ships only if the bench shows a clear gain over Quartet. In every case, it is the teacher for distillation |

The names are a theme of voices: one voice, four voices, many voices. The order is clear with no knowledge of music. Each name has a version number, for example "Sayso Solo 1". A new version is a new training run of that size. Decision 8 has the identifiers.

- **Input:** the lattice (the transcript with the alternatives inline), the cleanup settings of the style (tone, layout, format), the app kind, a short part of the text before and after the cursor, and an optional short style prompt.
- **Output:** a path through the lattice. The grammar enforces it.
- **Base:** one family for all three sizes, because distillation from logits needs the same tokenizer. The Solo prototype (phase 2) trains on Qwen3-0.6B first, for three reasons: S1-mini and Cleanup 1 use the same base, so a gain comes from our method and data; the cleanup runtime already runs it; and it has only attention layers, so the input as the speculative draft is simple. Then the same data trains Qwen3.5-0.8B, and the better base goes on. Both families have three sizes with Apache 2.0 weights: Qwen3 (0.6 B, 1.7 B, 4 B) and Qwen3.5 (0.8 B, 2 B, 4 B, March 2026). The pinned llama.cpp of `llama-cpp-2` 0.1.160 has both the `qwen3` and the `qwen35` architectures (checked on 2026-11-05). Qwen3.6 is also Apache 2.0, and later Qwen versions exist. So the bake-off compares families, not single models. Gemma 4 E2B (April 2026, Apache 2.0) stays in the bake-off as a check at the size of Solo. If it is much better at that size, the plan must choose between it and distillation in one family. Check the sizes and licenses of each family when the bake-off starts. Also check that the pinned llama.cpp supports each architecture. Spike: Qwen3.5 has recurrent Gated DeltaNet layers, so the speculative draft must roll back their state when the model rejects a draft token. Check that llama.cpp can do this before Qwen3.5 becomes the base.

Training order:

1. **Solo first (phases 2 and 3).** Runs are fast on the 4080, so the recipe is found at the small size: the data mix, the lattice format, supervised fine-tuning, and then preference training.
2. **The same recipe on the 2 B and 4 B bases (phase 5).** Post-training does not add parameters, so each size starts from its own base model. The factory, the lattice format, the recipe, and the bench carry over.
3. **Distillation (phase 5).** Choir teaches Solo and Quartet from its logits, in addition to the text from the open-weight teachers (section 7, stage 3). This usually gives a better small model than training on text alone.

Why they can beat S1-mini and Cleanup 1:

- They see candidates, not raw context.
- They train on the errors of Sayso's own speech models.
- They can read a short prompt, so more styles run locally.
- They are multilingual from the start.
- The grammar removes the worst failure, invented words, by construction.

Speed: a cleanup output is mostly a copy of the input. The runtime uses the "keep" path as a draft and checks many tokens in one batch (speculative decoding with the input as the draft). The model spends sequential steps only where it changes something. Target for Solo: at p95, at most 100 ms more than the cleanup models of today on Apple silicon. This is an estimate. Phase 3 measures it.

Weights: open, under the license of S1-mini (section 13, decisions 1 and 2): Apache 2.0, a NOTICE file, and one added term. Each use must name the model by its full name with the exact capitals, for example "Sayso Solo 1", and credit "Sayso" as the publisher. The Apache 2.0 base models allow this. A model that reads a lattice is of little use without the refiner, so open weights help the name of Sayso more than they help a competitor.

### 6.2 The speech model (phase 6)

A speech model that gets context words in its prompt and hears them correctly, instead of a fix after recognition. This is the direction of the field. Canary-Qwen-2.5B (NVIDIA, CC-BY-4.0) was at the top of the Open ASR Leaderboard at its release, and Gemma 4 E2B (Apache 2.0) takes audio input of up to 30 s. Both are speech models with a language model inside.

Three starting points, in a bake-off:

1. **Fine-tune Canary-Qwen-2.5B.** English only. NVIDIA trained it on 234,500 hours with 32 A100 GPUs. A fine-tune needs a small part of that.
2. **Fine-tune Gemma 4 E2B.** Multilingual and Apache 2.0, but limited to 30 s per call.
3. **Build our own.** The Parakeet TDT v3 encoder (25 languages), with Solo or Quartet as the decoder.

The output of the speech model goes into the lattice as a second transcript. Parakeet stays the witness: each word of the speech model that is not in the Parakeet transcript must be a context word with acoustic support. Otherwise the refiner does not accept it. So the guarantee stays.

Limit: a model of 2 B parameters is slow on a computer without a GPU. The speech model is for Apple silicon and NVIDIA GPUs. On other computers, the phase 3 stack stays the default.

Name: the speech model gets a name from the same theme of voices if it passes Gate 6. Until then it has no name.

## 7. The data factory

The data is the asset that lasts. Base models change every few months. With the factory, a new base model is a new training run, not a new project.

All audio is real human speech (decision 10). Synthetic voices miss the patterns and the nuance of how people speak, so the factory uses no TTS. The audio comes from the collection page (section 7.1).

The factory lives in a separate repository (decision 6). It is Python, with versioned datasets and one command for each stage.

Stages:

1. **Scenarios.** About 40 scenario types: prompts to a coding agent, terminal commands, code review comments, chat messages, email, notes, documents, meeting notes, and more. Each has an app kind and the settings of a style. The collection page starts with about 10 of them.
2. **Context.** Each prompt gets a synthetic context: a file tree and symbols from a real repository with a permissive license, channel members, an email thread, a web page. Names come from lists that cover many cultures. Products and companies come from Wikidata (CC0). Each context also has distractors: many words that are not in the text.
3. **Prompts.** A teacher model writes the prompts for the collection page: the sentence to read or the situation to describe, with its context words and its reference text. The teachers are open-weight models (section 13, decision 5):

   | Teacher | Weights | License |
   |---|---|---|
   | GLM-5.3-Flash (Z.ai, August 2026) | 320B MoE, 18B active | MIT |
   | DeepSeek V4.1-Flash (October 2026), or V4-Pro | MoE | MIT |
   | Kimi K3 (Moonshot, July 2026) | 2.8T MoE, 104B active | Kimi K3 License: MIT-style, with extra terms above high revenue and user limits |

   The flagship GLM-5.3 has a custom license. Use the Flash model. Kimi K3.1 Pro may have weights soon, with a license that is not known yet.

   Rules for teachers:

   - Use more than one teacher, and record the teacher and its version in each example. One teacher gives one style of writing, and the student copies it.
   - The model license applies to every host. The API of the publisher adds its own terms. A neutral host or rented GPUs add none. Before you use an API, read its terms about training on its output.
   - Read the Kimi K3 License for terms on output and on models trained with it before you use Kimi.
   - These models are too large for the 4080. Use hosted APIs, or rent several H100 GPUs for large batches.
4. **Labels.** Each clip gets its reference text: the prompt text for the Read task, and the draft as the contributor fixed it for the other tasks (section 7.1). The factory keeps the draft too.
5. **Audio.** Real recordings only, from the collection page and the other sources of section 7.1. An option to test in phase 2: add noise, room echo, and microphone color to real clips. This changes the conditions, not the voice.
6. **Recognition.** Run every speech model of Sayso over the audio. The Parakeet models and Whisper run on the 4080. The Mac-only models (FluidAudio, Apple Speech) run on a Mac. Keep the transcript, the word times, and the CTC log-probabilities. So the training data has the real errors of Sayso's own models.
7. **Error model.** Learn the confusion table from stage 6: which phonemes the speech models of Sayso mix up on real speech. An option to test in phase 2: text-only examples with errors from this table. They need no artificial voice, but they have no real disfluency, so the bench decides whether they help.
8. **Lattices.** Build each lattice with the real `sayso-refine` code, through a CLI or a Python binding. Training and the app use the same code. Each clip gets several context lists (other distractors, with and without the correct word), so one clip gives several training examples.
9. **Hard cases.** Distractors that sound close. Examples where nothing must change (most of the data, because most real dictations need no fix). Prompt-injection text in the context. Very long lists of context words.
10. **Split.** The collection page puts each contributor and each prompt in one split (section 7.1). So test, dev, and train are disjoint by speaker and by entity from the first clip.

### 7.1 Speech collection

Real speech is the only audio (decision 10). It trains the models, and it is the whole test set. Sayso does not pay for recordings (decision 4). The main source is a collection page on the Sayso website. Chris's own dictations, public sets, and later donations from the app add to it.

**The collection page.** A new page on justsayso.app. The site is Astro, with a Cloudflare Worker for `/api/*`, so the page uses the same stack.

- Recording: the browser records the microphone with an AudioWorklet. Echo cancellation, noise suppression, and automatic gain are off, so the audio is close to what the app records. The page saves 16 kHz mono 16-bit WAV.
- Checks in the browser, before the upload: length, level, silence, and clipping. The contributor hears the clip and can record it again.
- Live words: while the contributor speaks, the page streams the audio through the Worker to Deepgram Nova-3 on Cloudflare Workers AI over a WebSocket, and shows the words as they come, as the Sayso overlay does. When the contributor stops, the last result is the draft for the free-speech tasks. Workers AI has no Parakeet model (checked on 2026-11-05). Nova-3 costs about $0.0092 per audio minute over a WebSocket, so the whole goal costs about $30. Flux, the other streaming model, takes key terms, so it can get the words of the card right more often. Test both on the first 100 clips, and keep the one with fewer errors on context words.
- The stream is only for the live words and the draft. The page still records the full 16 kHz clip and uploads it, and the training transcripts still come from every Sayso speech model in the factory (section 7, stage 6).
- parakeet.js (MIT) is not the default, because of its size. Parakeet TDT 0.6B v3 needs a 1.2 GB fp16 encoder on WebGPU (with the shader-f16 feature), or a 650 MB int8 encoder that runs on the CPU at about 3 times real time. Most volunteers come from a link, and many are on phones, so a download of that size before the first card loses them. Later, it can be an option for desktops with WebGPU ("run Sayso's own model in your browser"), and the page can also run `sayso-refine` in WebAssembly as a demo. Spike: an ONNX export of the TDT-CTC 110M for parakeet.js or the sherpa-onnx WebAssembly build. At about a tenth of the size, it could be the default. The full app does not run in a browser.

Tasks:

1. **Read.** A sentence with entities, in a mock app window (chat, terminal, editor, mail) that shows the context. The text is the label.
2. **Say it your way.** A situation with its context, for example "Tell Amira that the Kubernetes migration moved to Friday." The contributor speaks freely, sees the draft, and fixes it to the text they meant to write. The fixed text is the label. The fix itself (draft to label) is also data: real recognition errors with their corrections. This gives the natural speech that reading does not give. The situations are fictional, so contributors do not say private things.
3. **Change your mind.** Prompts that ask for a self-correction, for example "Give a meeting time, then correct it." The contributor fixes the draft to the final text.
4. **Spell it, punctuation, and commands.** Names that the contributor spells after a cue, spoken punctuation ("comma", "new paragraph"), and voice commands ("scratch that").
5. **Listen and vote** (later). Contributors check the clips of others, as in Common Voice.

**Hard names get a guide.** A card with a name that is hard to say shows how people usually say it, under the sentence (for example "Siobhán · shih-VAWN"). It is always visible, so it works on phones. The guides are in `src/lib/voices/pronounce.ts` in `sayso-website`, checked against Wiktionary, Wikipedia, and other sources on 2026-11-05. No source confirmed Kwabena, Mokoena, and Venkataraman, so a native speaker must check those three. Technical terms such as Nginx and SQL get no guide on purpose: the different ways that people say them are useful data. Each new card with a hard name needs its guide in that file.

**Drafts can anchor people.** A contributor can accept a draft that is almost right, and drafts fail most on the hard words: names, paths, and symbols. Those are the words that the labels must get right. So:

- The page marks the words in the draft that look wrong: words that sound like a context word of the card but are spelled differently, and filler words.
- The context words of the card appear under the field as chips with the exact spelling. One tap puts the word in the text.
- Points do not depend on the number of edits, so nobody gains from edits that are not needed.
- The daily verification flags a label that is the same as the draft when the other Sayso transcripts disagree with the draft on a context word.
- Every test clip of these tasks gets a second check by another person (Listen and vote) or by Chris before it counts.

**The server chooses useful prompts.** The Worker gives each contributor the next prompt from the categories that are furthest below their quota (section 7.2), and from the prompts that need more speakers. The contributor can skip a prompt. So the data follows the goal, not the preferences of the contributors.

**Splits from the first clip.** Each new contributor goes into one split: test, dev, or train. Test and dev contributors get prompts from their own pools, with entities that are not in the train pool. So the splits are disjoint by speaker and by entity from the start. About half of the first contributors go to test and dev, until those sets are full. After that, all new contributors go to train. Test prompts get 3 speakers each. Train prompts get 1 or 2.

**The game.** It rewards what the data needs.

- A community progress bar toward the goal and its milestones.
- Points for each clip. Free speech and self-corrections count more than reading. Categories that are below their quota show "needed now" and count double.
- Points stay pending until the daily verification, so a bad clip earns nothing.
- Streaks, levels, and a badge for each category.
- An optional nickname on a leaderboard. Optional credit by name in the public dataset.
- Share links at the end of a session (X, Bluesky, Threads, Facebook, LinkedIn, Reddit, email, and the system share sheet). Each link goes to the contributor's own share page, and the image of the post is their reader's card. The goal needs 500+ voices, and a new voice helps more than more cards from the same voice.

**Identity.** No account. When a contributor starts, the Worker makes a contributor key and keeps it in an HttpOnly cookie, so a contributor who comes back continues with the same key and does not need to remember it. The start screen asks for a key from before. A key that the contributor types goes into the cookie too. With the key, a contributor moves to another device or deletes their data. Optional facts: accent or region, age range, microphone type, and device type. The bench uses them to measure accuracy for each group.

**Storage.**

- Audio: a private Cloudflare R2 bucket, one WAV for each clip. 20,000 clips of about 9 s are about 6 GB. R2 gives 10 GB of storage free each month.
- Metadata: a Cloudflare D1 database with four tables: contributors (consent level and version, split, optional facts, hash of the key), prompts (task, text, context words, category, split), clips (contributor, prompt, R2 key, length, checks, draft, fixed text, state), and points.
- Upload: the Worker receives the clip, checks the size and the length, writes the audio to R2 and the row to D1. Turnstile and rate limits stop bots.
- Backup: each week, the factory exports R2 and D1 to the storage of `sayso-models` as a versioned dataset.

**Daily verification.** A batch job on the 4080 runs Parakeet over the new clips and compares each transcript with the reference text (for free speech, without filler words). A clip with a large difference goes to a manual check, or out. Verified clips turn pending points into points. Chris listens to a random sample each week.

**Consent and licenses.**

- Each contributor agrees to one of two levels: "train Sayso models" or "train Sayso models and publish my recording in the public dataset". The consent says that the weights are open, so a deletion removes the data from future training but not from released weights.
- The public dataset and the test set use CC-BY-4.0, so users of the data give credit, as with the code and the weights.
- Contributors must be 18 or older. Voice is personal data under the GDPR and other laws. Consent is the legal basis. A contributor deletes their data with the contributor key.
- The website has no privacy page yet. The collection page needs one.
- A lawyer reads the consent text, the privacy page, and the naming clause. The lawyer also checks the laws on biometric data that name voiceprints (for example in Illinois and Texas). Chris launched the page on 2026-11-05, before this review (decision 12). The consent page shows the version `2026-11-05-draft`, and each contributor row in D1 keeps the version that the contributor agreed to (`contributors.consent_version`). So when the reviewed text replaces the draft, the clips under the draft are easy to find. Decide then whether those contributors must agree again.

**Other sources.**

- **Chris's own dictations.** A debug build saves a context snapshot with each dictation. They are real and technical, but they have one speaker, so they go to train only.
- **Public sets.** AMI (CC-BY-4.0) for real disfluencies. Common Voice (CC0) for accents. ContextASR-Bench (MIT) for a third-party number.
- **Donations from the app** (phase 5, decision 3). The richest data is real dictation with real context and the user's own correction. It is also the most private. So there are two levels, and each send is a choice of the user:
  - **Share corrections.** Word pairs only: what the speech model heard, what the user wanted, the phonemes, and the app kind. No audio and no sentence. This feeds the confusion table and the sound match. Sayso shows the list before the send, and the user can remove items.
  - **Share a dictation.** The audio, the transcript, the final text, the user's later edit, and the matched context words. Never the context snapshot. The factory adds its own distractors. Sayso shows everything before the send, and the user can remove words or cancel. A scan for secrets (keys, tokens, passwords) blocks the send when it finds one.

  The details need more work before phase 5: the upload service, the review UI, the storage, and the deletion flow.

### 7.2 The collection goal

Proposed on 2026-11-05. Chris confirms the numbers.

The unit is a verified clip: one recording of one prompt, 3 to 30 s long, with consent, that passed the checks of the page and the daily verification.

| Split | Clips | Speakers | Most clips that count for one speaker | Hours (about 9 s a clip) |
|---|---|---|---|---|
| Test | 3,000 | 100 or more | 30 | About 7.5 |
| Dev | 1,000 | 25 or more | 40 | About 2.5 |
| Train | 16,000 | 375 or more | 200 | About 40 |
| Total | 20,000 | 500 or more | | About 50 |

Why these numbers:

- **Test.** 3,000 clips hold about 4,000 entity spans. That measures entity accuracy to about 1.5 points overall, and to about 4 points for each category, so a gain of 5 points is visible. With 100 speakers, no one voice decides a result. These margins ignore that the clips of one speaker are alike, so the real margins are wider.
- **Train.** Each clip gives several training examples: the transcripts of 3 or 4 speech models, times 2 or 3 context lists. So 16,000 clips give about 100,000 examples. The examples of one clip are alike, so the real variety is 16,000 utterances. The learning curve (below) checks whether that is enough.
- **The cap for each speaker** keeps the data from becoming the voice of a few people. Clips over the cap stay in the dataset, but they do not count toward the goal. The cap also applies to Chris.
- **Speakers are the hard part, not clips.** One person with 1,000 clips helps less than 25 people with 40 clips each.

Mix of tasks: Read 45%, Say it your way 30%, Change your mind 15%, and Spell it, punctuation, and commands 10%.

Mix of content: names of people and places from many cultures 25%, product names and technical terms 20%, paths and code symbols 15%, acronyms 10%, numbers, dates, times, and money 10%, email addresses and URLs 5%, and no rare words 15%.

Milestones:

| Milestone | Verified clips | What it starts |
|---|---|---|
| M1: bench | Test and dev full (4,000) | Phase 1: the bench, the baselines, and the measurement of the finder |
| M2: prototype | 12,000 (8,000 of them train) | Phase 2: the classifier and the Solo prototype |
| M3: goal | 20,000 | Training of Solo 1 in phase 3 |

The learning curve: at M2, train the prototype on 25%, 50%, and 100% of the train clips. If the results still rise steeply at 100%, raise the goal of M3. If they are flat, M3 can be smaller.

English comes first. Each later language gets its own goal (phase 7).

## 8. The Dictation Bench

Sayso publishes its own benchmark, as Aqua did with AISpeak-10. A public benchmark with a clear method is the proof for a claim such as "the most accurate local dictation".

- **Test set:** the test split of the collection (sections 7.1 and 7.2): real human speech only, each clip with its context. The speakers and the entities of the test set are not in the training data.
- **Categories:** entities (names, terms, paths, symbols), numbers and dates, self-corrections, filler words, lists, email, tone, and "nothing to change".
- **Metrics:**
  - WER and entity accuracy (entity recall and NE-WER).
  - **False-correction rate:** words that were correct in the transcript and wrong in the final text. This is the most important number.
  - Content loss: words that carry meaning and are not in the final text.
  - Format accuracy (lists, email).
  - Time at p50 and p95, and memory, on reference hardware: an Apple silicon laptop with 8 GB, the M5 Pro, a Windows laptop with no GPU, and the PC with the 4080.
- **Systems:** the raw transcript; Claude and ChatGPT with the full snapshot, as the ceiling; S1-mini; Cleanup 1; the untrained base model with the lattice input; the refiner without a model, followed by S1-mini; the refiner with Solo, Quartet, and Choir; the speech model; Apple Intelligence.
- **Third-party check:** the English part of ContextASR-Bench.

## 9. Compute

| Resource | Use |
|---|---|
| RTX 4080, 16 GB | The data factory (Parakeet and Whisper over every clip) and the daily verification. Full fine-tune of models up to about 1 B parameters, with 8-bit optimizer states and gradient checkpointing (estimate). LoRA for models of about 2 to 4 B. Choir logits for distillation. Evaluation |
| M5 Pro | Mac-only speech models, time tests, Chris's real dictations |
| Colab through Google AI Pro | About 200 compute units a month, according to third-party reports (Google gives no number). That is about 36 A100 hours when an A100 is free. Sessions stop after 12 hours, and no GPU is guaranteed. Good for sweeps and short runs |
| Hosted APIs for the teachers | Prompts for the collection page (section 7, stage 3), and the optional text-only examples (stage 7) |
| Rented H100 or A100 80 GB | Full fine-tunes of the 2 B and 4 B models, the speech model, and teacher batches if an API is not usable |

Rough estimates, not quotes:

| Item | Estimate |
|---|---|
| Prompts for the collection page, about 10,000 | Under $20 of teacher tokens |
| Text-only examples, 1 to 2 million (optional, stage 7) | $500 to $2,000, only if phase 2 shows that they help. For example, GLM-5.3-Flash costs about $0.15 per million input tokens and $0.50 per million output tokens |
| Solo, Quartet, and Choir training | Solo on the 4080. Up to $1,000 rented for the 2 B and 4 B runs |
| Speech model bake-off | $1,000 to $5,000 |
| Collection page: Workers, R2, D1, Turnstile, Workers AI for live words | About $0 to $5 a month for hosting. Live words for all 20,000 clips cost about $30. R2 has 10 GB of storage free each month, then $0.015 per GB-month. D1 is free up to 5 GB, with 100,000 written rows a day. Check the prices when you build |
| Donation uploads (phase 5) | Storage, a small monthly cost |

The speech model bake-off costs the most money, and it waits for its gate. The largest cost of this plan is now the time of contributors.

## 10. Changes to Sayso

- **Pipeline:** transcribe, refine, style, insert. Replacements, spelled words, and spoken punctuation become resolvers in the refiner.
- **Engine protocol:** a new optional engine feature, `ctc_logprobs` (section 5.5). Later, maybe `alternatives` (n-best transcripts).
- **Cleanup runtime:** decoding constrained by the lattice grammar, log-probability scoring, the input as the speculative draft, and reuse of the KV cache for the fixed start of the prompt.
- **Host API:** the capability `context.provide`. The SDK gets a type for context words.
- **Platform crates:** read the text around the cursor and the visible text (AX, UI Automation), OCR as a fallback, and the working folder of a terminal.
- **UI:** settings for each context source and each app, marks for changes in History, learned words in the Dictionary, and the review screen for donations.
- **Website (`sayso-website`):** the collection page, a Worker API for prompts, clips, and points, an R2 bucket, a D1 database, Turnstile, and a privacy page (section 7.1).
- **Licenses:** see decision 1.

## 11. Phases and gates

Each phase ships something that works alone, or ends with a decision. If a gate fails, the next phase stops or changes, and the earlier work keeps its value.

The order follows decisions 9 and 10. Real speech comes first, because every later step trains or tests on it. The milestones of the collection (section 7.2) start the phases that need data. Work that needs no data runs while the collection grows.

**Phase 0: the collection page.** The page is live since 2026-11-05. The pools, the verification, the export, and the review are not done. Section 15 has the details.

- The page, the tasks, the game, the Worker API, R2, D1, and Turnstile (section 7.1).
- The first prompt pools for test, dev, and train. A teacher model writes them. The live page has a seed deck of 140 cards, which a model wrote without the pipeline of section 7. A person checks every test prompt and a sample of the others. Entities must be hard in the right ways: names from many cultures, acronyms, paths, symbols, and words that sound like common words.
- The consent text and the privacy page, and a lawyer's review of them. The page went public before the review (decision 12).
- The daily verification and the weekly export.
- **Exit:** the page is public, and verified clips arrive each day.

Why first: every later step trains or tests on real speech, and collection takes the longest time. Each day that the page runs before the training starts is a day of data.

**While the collection grows.** This work needs no data, so it does not wait:

- The first code of `sayso-refine`: context words and spoken forms, the English G2P (CMUdict and a rule-based fallback), the phoneme n-gram index, matching in two directions, and a lattice with coarse features. A CLI and a Python binding, so the factory uses the same code as the app.
- The bench harness, with the metrics of section 8. The first verified clips test it.

**Phase 1: the bench and the finder.** It starts at M1.

- The bench on the test split. Baselines: the raw Parakeet transcript, the Dictionary of today, S1-mini, Cleanup 1, the untrained base model with the lattice input, and a cloud model with the context words as the ceiling.
- **Gate 1:** every baseline is measured, and the error types are known (which kinds of entity fail, and how often). The finder puts the correct word in the top 10 for at least 90% of the entity spans of the bench, and this does not fall from 50 to 5,000 context words. Then this file gets the numbers for Gates 2 and 3.

Why here: without the bench, no result means anything. The finder is the riskiest part (section 3). The model cannot choose a word that the finder did not offer, and each wrong candidate is a chance for a false correction.

**Phase 2: the classifier and the Solo prototype.** It starts at M2.

- Lattices from the train clips: every speech model of Sayso, and several context lists for each clip (section 7, stage 8).
- The classifier of section 5.8, trained on the features of the lattices.
- The Solo prototype: a full fine-tune of Qwen3-0.6B, then the same data on Qwen3.5-0.8B (section 6.1). It runs without the lattice grammar, and the bench counts invented words instead. That count shows how much the grammar will matter. The grammar changes only the decoding, not the training output, so to add it later needs no new training run.
  - The loss is on the output tokens only.
  - The features of each alternative go into the input as coarse labels ("sound: high", "source: Dictionary"), not as raw scores. Small models use labels better than numbers.
  - The data also has plain cleanup examples with no context words, because Solo must also replace S1-mini.
- Ablations: no candidates; raw context text instead of candidates; 5 context words against 500; with and without the options of stages 5 and 7; and the learning curve of section 7.2.
- **Gate 2:** on the test split, compare two paths:
  - The refiner path: the classifier, then S1-mini, then the check.
  - The model path: the Solo prototype.

  A path passes when its entity accuracy is clearly better than the raw transcript, S1-mini, and Cleanup 1; its false-correction rate on the clips that need no change is no higher than that of S1-mini; its cleanup is no worse than that of S1-mini; and its time at p95 is at most 100 ms more than S1-mini on Apple silicon.

  If both paths pass, both ship: Solo for local cleanup, and the refiner path for every other style (cloud providers and other cleanup models). If one passes, it ships first. If neither passes, the type of failure decides the fix: low recall goes back to the finder, false corrections need more "keep" data and DPO, and weak cleanup needs more data of that kind.

Why here: one set of lattices answers "model or refiner first" with numbers. It is also the cheapest point to learn whether the idea works, because no app code changes yet.

**Phase 3: into the app, the rest of the refiner, and Solo 1.** Solo 1 trains at M3.

- What passed Gate 2 goes into Sayso behind a setting, macOS first, with Dictionary words as the context. Solo needs a lattice input mode in the cleanup runtime. The refiner path needs the check after the style.
- Then the rest of the refiner, in the order that Gate 2 shows: the context snapshot and the adapters, the resolvers, `ctc_logprobs`, the lattice grammar, and the input as the speculative draft. The bench measures each part before and after.
- The full train set, DPO, the final base for Solo, and 4-bit files. The result is Sayso Solo 1.
- Windows follows macOS.
- **Gate 3:** Solo 1 is better than S1-mini and Cleanup 1 in every category of the bench. False corrections are no higher than at Gate 2, also with context from the screen. Solo's time is inside the target on the 8 GB laptop. Then Solo 1 ships, and its weights are published (decision 2).

Why this order: users get value early from a source that exists today (the Dictionary). Each later part of the refiner is a measured change, not a guess.

**Phase 4: memory.** Learned words, undo, and personal history.

- **Gate 4:** fewer repeated errors in Chris's real dictations over two weeks.

**Phase 5: Quartet, Choir, and donations.**

- The recipe of Solo 1 on the bigger bases of the same family, and distillation from Choir into Solo and Quartet (section 6.1).
- Donations from the app, both levels. Collection continues on the page.
- **Gate 5:** Choir ships only if it is clearly better than Quartet. A distilled Solo ships as a new version only if it beats Solo 1. The Dictation Bench becomes public.

**Phase 6: the speech model.** The bake-off of section 6.2.

- **Gate 6:** entity accuracy better than the phase 3 stack by enough to pay for its time and memory (proposed: 5 points or more), on Apple silicon and NVIDIA GPUs.

**Phase 7: more languages.** A collection goal for each language on the page, then G2P, ITN, and factory data for the languages of Cleanup 1 and more.

The logic of the refiner is the same on every system. Context capture is different on each system. Linux gets what AT-SPI allows.

## 12. Risks

- **Not enough speakers.** The goal needs 500 speakers, and a volunteer program can grow slowly. The game, the credit in the public dataset, and the community progress bar help. The milestones start work before the goal, and the learning curve shows what the data already gives.
- **Browser audio is not app audio.** The page turns off the processing of the browser and records the device type. The bench reports results by device type. Donations from the app (phase 5) add real app audio.
- **Voice data is legally sensitive.** Voice is personal data, and some laws treat voiceprints as biometric data. The page is public, and the lawyer's review is not done (section 7.1, decision 12). Until the review, the consent text is a draft, and each contributor row records the version that the contributor agreed to.
- **Wrong changes.** The thresholds favor "keep". Explanations and undo teaching catch the rest.
- **Some apps give poor accessibility text** (Electron apps, games, remote desktops). OCR is the fallback, and connected apps can send context words.
- **Trust.** Sayso reads the screen. Everything stays local. The user sees and controls each source. Sayso never reads password fields. Only matched words can reach a cloud provider.
- **Base models change fast.** The factory makes a new training run cheap, and each new base goes into the bake-off.
- **Computers without a GPU.** The fast path, the classifier-only mode, and the speculative draft keep the time low. Solo is the default for these computers. Choir and the speech model are not for them.
- **Licenses.** CC-BY-4.0 (the NVIDIA models) needs attribution. Each dataset and teacher needs a check. Contributors agree to a consent that allows training and, at the second level, publication.
- **Bad or hostile recordings.** The checks in the browser, the daily verification, Turnstile, rate limits, and later the votes of other contributors. Points stay pending until the verification, so a bad clip earns nothing.
- **Read speech is not dictation.** People read more cleanly than they dictate. The "say it your way" and "change your mind" tasks, and the donations, give natural speech. The bench reports read and natural speech apart.
- **Size of the work.** This is a program of many months. The gates let it stop with value at the end of each phase.

## 13. Decisions

Chris decided these on 2026-11-05.

1. **Licenses: free to use, with credit, like S1-mini.**
   - `sayso-refine`: Apache-2.0 with a NOTICE file. Apache 2.0 requires each distribution to keep the NOTICE, so users give credit. It stays a standard open-source license, so other projects can use the crate with no legal review. Apache-2.0 code can go into the GPL-3.0 app.
   - Model weights: Apache 2.0, a NOTICE file, and the naming clause of S1-mini (section 6.1).
   - Public data and the test set: CC-BY-4.0 (section 7.1).
   - `sayso-context` stays GPL-3.0-only, because it uses the GPL platform crates (section 5).
   - Write ADR 0006 when the first crate is made. It records these licenses next to ADR 0002.
2. **Model weights: open.**
3. **Donations from users: yes.** Section 7.1 has the first design. The details need more work before phase 5.
4. **No paid recordings.** Real speech comes from the collection page, Chris's dictations, public sets, and later donations (section 7.1).
5. **Teachers: open-weight models.** GLM-5.3-Flash and DeepSeek V4.1, maybe Kimi K3 (section 7, stage 3).
6. **The factory lives in a separate private repository**, `sayso-models`.
7. **Floor hardware:** an Apple silicon Mac with 8 GB, and a Windows laptop with 16 GB and no GPU.
8. **Model names: a theme of voices** (section 6.1).
   - Sayso Solo (under 1 B), Sayso Quartet (about 2 B), and Sayso Choir (about 4 B), each with a version number. The first release is "Sayso Solo 1".
   - Solo is trained first. The bigger sizes start from their own base models with the same recipe, and Choir teaches the smaller two.
   - The naming clause asks for the full model name and credits "Sayso" as the publisher. The copyright line in the LICENSE and NOTICE files names Watzon Ventures LLC, which owns the rights (ADR 0002).
   - Catalog IDs: `sayso-solo-1`, `sayso-quartet-1`, `sayso-choir-1`.
   - Hugging Face: the handle `sayso` belongs to an inactive user account (checked on 2026-11-05). `sayso-ai` was free on that day. Register it before the first upload. Repository names look like `sayso-ai/solo-1-gguf`.
   - The speech model gets a name from the same theme if it passes Gate 6.
   - Words that were rejected, and why: "Refine" (the same word as the refiner), "Hear" (sounds like "here"), single letters such as X1, D1, and H1 (other models use them: iFlytek Spark X1, Liquid AI d1, Falcon-H1), "Steno" (StenoAI), "Proof" (Every's Proof), "Bass" (sounds like "base"), "Chord" (sounds like "cord"), "Tenor" (Google's GIF service).
9. **Order after the collection: the core of the refiner, then the model, then the rest of the refiner** (section 11). The candidate finder comes first, because the model reads its candidates and cannot be better than them. Then the classifier and the Solo prototype train on the same lattices, and the bench decides which path ships first. The rest of the refiner follows in the order that this test shows.
10. **Real speech first, and only real speech** (sections 7.1 and 7.2). Phase 0 is a collection page on the Sayso website, with a game, where people record the speech that the models need. The factory uses no TTS audio, because synthetic voices miss real speech patterns. The collection goal and its milestones start the later phases.
11. **Live words from Deepgram on Workers AI** (section 7.1). The collection page streams each recording to Deepgram Nova-3 (or Flux, after a test on the first 100 clips) on Cloudflare Workers AI and shows the words live. The last result is the draft that the contributor fixes. parakeet.js is not the default, because of its download size. The design is on the Paper page "Website · Voices".
12. **Launch before the lawyer's review** (section 7.1). Chris deployed the collection page on 2026-11-05 and started to share it. The consent text stays a draft (`2026-11-05-draft`) until a lawyer reads it.

Still open:

- The numbers of the collection goal (section 7.2). The live page already shows them (20,000 clips, 500 voices, and the milestones), so a change must also go into `src/lib/voices/goal.ts` in `sayso-website`.

- The consent text, the privacy policy, and a lawyer's review of them, of the naming clause, and of the model names (a trademark search for Solo, Quartet, and Choir).
- The upload service, the storage, and the deletion flow for contributions and donations.
- The Kimi K3 License and the API terms of each teacher host, read in full.

## 14. Terms

Proposed for `CONTEXT.md`:

**Refiner**
The step and the crate (`sayso-refine`) that turn a transcript and its context into a lattice, and choose a path through it. It is not an engine.

**Context snapshot**
The context that Sayso collects at the start of one dictation. It stays in memory for that dictation only.

**Context adapter**
Code that knows where the useful words of one kind of app are.

**Context word**
A rare word from the context of one dictation, with its written form, kind, source, and spoken forms.

**Learned word**
A word that Sayso adds after the user corrects a dictation. The user can remove it in the Dictionary.

**Resolver**
Code that computes the one correct written form of a spoken span: a path, a symbol, a number, or an address.

**Lattice**
All the outputs that the refiner allows for one dictation.

**False correction**
A word that was correct in the transcript and wrong in the final text.

**Solo, Quartet, Choir**
The Sayso cleanup models of this plan, from small to large. Each name has a version number, for example Sayso Solo 1.

**Dictation Bench**
The benchmark of section 8.

**Collection page**
The page on the Sayso website where people record prompts for the training data (section 7.1).

**Contribution**
A recording that a person makes for a prompt on the collection page.

**Verified clip**
A contribution that passed the checks of the page and the daily verification. The collection goal counts verified clips.

**Donation**
Corrections or a dictation that a user sends to Sayso from the app, after a review. A donation never holds the context snapshot.

## 15. Where the work stands

Written on 2026-11-05. Update this section when the state changes.

### 15.1 Built and live: Phase 0

The collection page, Sayso Voices, is live at https://justsayso.app/voices since 2026-11-05.

| Part | Where |
|---|---|
| Code of the pages and the Worker API | `sayso-website` (`/Volumes/SHARED/Projects/personal/sayso-website`), branch `voices`, commit `4800f25`. It is not merged into `main` and not pushed. Production was deployed from this code before the commit (last Worker version `d0588c0b`). The README of `sayso-website`, section "Sayso Voices", has the pages, the API, the local test, and the Cloudflare setup. |
| Cards | `voices/prompts.json`: 140 cards (train 80, dev 20, test 40). `scripts/voices-seed.mjs` puts them in D1. |
| Goal, quotas, and points | `src/lib/voices/goal.ts` and `src/lib/voices/quota.ts` |
| Name guides | `src/lib/voices/pronounce.ts` (section 7.1) |
| Production data | D1 `sayso-voices` (id `815e551a-e870-47e1-9df7-b7937894e7e6`). The private R2 bucket `sayso-voices` has the audio at `clips/<split>/<yyyy-mm>/<id>.wav`. |
| Cloudflare | The Turnstile widget "Sayso Voices" (site key in `src/config.ts`), the secrets `TURNSTILE_SECRET_KEY` and `VOICES_SALT`, Workers AI (Nova-3) for the live words, and the Workers Paid plan (the image of a share page needs about 25 ms of CPU). |
| Design | Paper file `Sayso — v0.1 Design`, page "Website · Voices" |
| Promo video | `sayso-film/voices` (`/Volumes/SHARED/Projects/personal/sayso-film/voices`, not a git repository): `renders/sayso-voices-promo.mp4`, 25 s, 1080p. SPEC.md has the shots, REVIEW.md has the changes and the weak parts, and CREDITS.md has the sources. The backspace sound needs a CC BY 4.0 credit where the video is published. |
| Posts for X | `sayso-film/voices/X-POSTS.md`. None is posted yet. |

The page does what section 7.1 says, with two limits: Listen and vote does not exist, and points stay pending (section 15.2). The four other tasks run. Each session has ten cards. The contributor key is in an HttpOnly cookie. After a session, each contributor has a share page (`/voices/r/<id>`, a random id that is not the key) with their reader's card as the image.

Production has real contributors. Never reset the remote D1. The seed script changes the live deck, so run it with `--remote` only after a person checks the new cards.

### 15.2 Not built yet: needed before the exit of Phase 0

In order of importance:

1. **More cards.** The deck has 140 cards. The goal needs about 10,000 (section 9). Test cards get 3 speakers each, and dev and train cards get 2 (`SPLIT_GOALS` in `goal.ts`). So the goal needs about 1,000 test cards, 500 dev cards, and 8,000 train cards. When a card has all its speakers, the server still gives it, with a lower score (`choosePrompts` in `quota.ts`). So with 40 test cards, the test set fills with the same 40 sentences in many voices, and it measures too few entities. Also, a contributor who read every card of their pool gets no more cards. The teacher-written pools (section 7, stage 3) are the next step. A person checks every test card and a sample of the others. Each new card with a hard name gets its guide.
2. **The daily verification** (section 7.1). Until it runs, every clip and every point stays pending, and the leaderboard stays empty. `pnpm voices:verify-local` only simulates it on the local database. When the job rejects a clip, it must also lower `tallies`, `contributors.clips`, and `prompts.speakers`.
3. **The weekly export** of R2 and D1 (section 7.1). Until it runs, the R2 bucket is the only copy of the audio.
4. **The privacy page and the lawyer's review** (section 7.1, decision 12).
5. **Nova-3 or Flux.** Compare them on the first 100 clips, and keep the one with fewer errors on context words (section 7.1).
6. **A second check of the test clips** of the tasks where the contributor fixes the draft, by Chris or by Listen and vote (section 7.1).
7. Smaller items:
   - A native speaker checks three name guides (section 7.1).
   - The server accepts 10 new contributors from one network address each day (`LIMITS` in `worker/voices/api.ts`). At an event or in an office, this can stop people. Contributors who come back with a key are not affected.
   - Not built: Listen and vote, and credit by name in the public dataset.
   - Merge the branch `voices` into `main` in `sayso-website`. A deploy from `main` before the merge removes the Voices pages from production.
   - Not part of this plan: `/api/download` sends people to the GitHub release page and not to the file, because GitHub refuses the Worker (403) when no `GITHUB_TOKEN` is set.

### 15.3 Work that needs no data

This work can start at any time (section 11, "While the collection grows"):

- Make the private repository `sayso-models` (decision 6). It does not exist yet. The teacher pipeline for the cards, the daily verification, and the weekly export go there.
- The first code of `sayso-refine`, with a CLI and a Python binding. Write ADR 0006 when the crate is made (decision 1).
- The bench harness, with the metrics of section 8.
- Register `sayso-ai` on Hugging Face before the first upload (decision 8).
- Read the Kimi K3 License and the API terms of each teacher host (section 13, still open).

### 15.4 When the data arrives

The milestones of section 7.2 start the next phases. The ledger on `/voices` counts every clip that is not rejected, with the cap for each speaker, so it also counts pending clips. The milestones count verified clips, so the daily verification must run before a milestone counts.

- **M1, 4,000 verified clips (test and dev full):** export the clips, and run every speech model of Sayso over them (section 7, stage 6). Then Phase 1: the bench, the baselines, and Gate 1 for the finder. Before Phase 1, check that the test split has 100 or more speakers and that the cap of 30 clips for each speaker holds (section 7.2).
- **M2, 12,000:** Phase 2: the lattices, the classifier, the Solo prototype, the learning curve, and Gate 2.
- **M3, 20,000:** Phase 3: Solo 1.

## 16. Sources

Read on 2026-11-05.

- [SpellMapper, arXiv 2306.02317](https://arxiv.org/abs/2306.02317)
- [Retrieval Augmented Correction of Named Entity Speech Recognition Errors, arXiv 2409.06062](https://www.alphaxiv.org/resources/2409.06062v1)
- [Fast Context-Biasing with CTC-based Word Spotter, arXiv 2406.07096](https://arxiv.org/abs/2406.07096)
- [Canary-Qwen-2.5B model card](https://huggingface.co/nvidia/canary-qwen-2.5b)
- [Gemma 4 model card](https://ai.google.dev/gemma/docs/core/model_card_4)
- [Qwen3.5 small models](https://artificialanalysis.ai/articles/qwen3-5-small-models)
- [ContextASR-Bench, arXiv 2507.05727](https://arxiv.org/pdf/2507.05727) and [its repository](https://github.com/MrSupW/ContextASR-Bench)
- [Aqua Voice](https://aquavoice.com/) and [a third-party review of Avalon](https://www.getvoibe.com/resources/aqua-voice-review/)
- [Google AI Pro and Colab compute units, third-party check](https://explainx.ai/blog/google-ai-pro-colab-a100-h100-200-compute-units-claimed-vs-verified-2026)
- [S1-mini license](https://huggingface.co/superwhisper/s1-mini/blob/main/LICENSE) and [model card](https://huggingface.co/superwhisper/s1-mini)
- [parakeet.js](https://github.com/ysdede/parakeet.js) and [a report on Parakeet v3 in the browser](https://github.com/nyosegawa/speech.cpp/issues/136)
- [GLM-5.3-Flash release and license](https://codersera.com/blog/glm-5-3-flash-complete-guide-2026/), [GLM-5.3 license](https://aiwiz.uk/blog/glm-5-3/)
- [DeepSeek V4.1-Flash-1017 under MIT](https://ccleaks.com/news/deepseek-v4-1-flash-1017-open-weights-oct-2026)
- [Kimi K3 license terms](https://www.digitalapplied.com/blog/kimi-k3-open-weights-shipped-license-restrictions-2026)
- [Workers AI Nova-3](https://developers.cloudflare.com/workers-ai/models/nova-3/), [Workers AI Flux](https://developers.cloudflare.com/workers-ai/models/flux/), and [parakeet.js](https://github.com/ysdede/parakeet.js) with [its WebGPU sizes](https://github.com/hyperaudio/hyperaudio-lite-editor/blob/main/WEBGPU.md)
- [Cloudflare R2 pricing](https://developers.cloudflare.com/r2/pricing) and [D1 free tier limits](https://freetier.co/articles/cloudflare-d1-free-tier-limits-pricing-and-alternatives)
- Name checks: [Liquid AI d1](https://www.liquid.ai/blog/d1-open), [StenoAI](https://github.com/ruzin/stenoai), [Every's Proof](https://every.to/on-every/introducing-proof), [Qwen3.5 small model sizes](https://www.therundown.ai/tools/qwen3-5-small)
- In this repository: [cleanup-models.md](cleanup-models.md), `VocabularyBooster.swift`, `crates/sayso-engine/src/sherpa.rs`
