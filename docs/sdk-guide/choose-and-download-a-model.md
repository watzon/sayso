# How to choose a model and download it

Use `list_models` to see which models this computer can run. Use `download_model` to put one on disk. Then tell the SDK which model to use.

You need a connected `Speech`. In Sayso mode, `list_models` needs no grant, and `download_model` needs the capability `models.download`.

## List the models

`list_models` takes an optional role. The roles are `Role::Final` (the final pass on a recording), `Role::Streaming` (text while the audio comes in), `Role::Vad` (voice detection), and `Role::Diarization` (who speaks when). A model can have more than one role.

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
fn show_models(speech: &Speech) -> Result<(), Error> {
    for model in speech.list_models(Some(Role::Final))? {
        let chosen: Vec<String> = model.choice.iter().map(|role| role.to_string()).collect();
        println!("{} ({}): {}, {} MB, chosen for: {}", model.id, model.name, model.state, model.size_bytes / 1_000_000, chosen.join(", "));
    }
    Ok(())
}
```

Each `ModelInfo` has:

- `id` and `name`. The id is stable and goes into requests.
- `roles`, `languages` (empty when the model has no list), `timestamps` (`word`, `segment`, or `none`), and `size_bytes`.
- `state`: `not_downloaded`, `downloading`, `downloaded`, `optimizing` (a load runs), `ready` (in memory), or `failed`. A model is on disk when its state is `downloaded`, `optimizing`, or `ready`.
- `choice`: the roles for which the SDK uses this model when a request names none.

On a Mac with the Swift engine, the list has more than 20 models. It has local models only: cloud models are not in the list, and Basic VAD (`basic`) is not a model.

## Download a model

Call `download_model` with the id and a reason. In local mode, the download starts at once, and the call returns when the model is on disk. If the model is on disk already, the call returns at once. `model_state` and `download_progress` events arrive on `speech.events()` while it runs.

In Sayso mode, Sayso asks the user first and shows your `reason`. Write one sentence that the user understands. If the user says no, the call returns the error code `declined`. In local mode there is no question, so ask your own user before you call it.

This function shows the progress while the download runs. It reads the events on a thread of its own, because `download_model` blocks:

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
fn download(speech: &Speech, model: &str) -> Result<(), Error> {
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let events = speech.events();
            while !done.load(Ordering::SeqCst) {
                if let Ok(Event::Message(protocol::Event::DownloadProgress(progress))) = events.recv_timeout(Duration::from_millis(100)) {
                    eprintln!("{}: {:.0} %", progress.model, progress.fraction * 100.0);
                }
            }
        });
        let result = speech.download_model(model, "To transcribe your recordings on this computer");
        done.store(true, Ordering::SeqCst);
        result
    })
}
```

The events of a download are `model_state { model, state: "downloading" }`, then `download_progress { model, fraction, bytes_done, bytes_total }`, then `model_state { model, state: "downloaded" }`. If the download fails, the last events are `model_state { state: "failed", message, reason }`, and the call returns an error with the same `reason` and no `code`.

An unknown id returns `unknown_model`.

## Choose a model for requests

A request chooses its models in this order:

1. The field of the request, for example `model` in `TranscribeFile` or `SessionStart`.
2. In Sayso mode, the choice of the user in Sayso. Your app cannot change it, and the model fields of `Options::models` do not count.
3. In local mode, `Options::models`. Then the first suitable model on disk.

`ModelChoices` sets the choice of the app for local mode:

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
fn options_with_models(engine: Engine) -> Options {
    let models = ModelChoices { final_model: Some("whisper-tiny".into()), vad_model: Some("basic".into()), language: Some("en".into()), ..Default::default() };
    Options::new(Mode::Local, App::new("Notetaker")).engine(engine).models(models)
}
```

For `vad_model`, `basic` means Basic VAD. If the model that you chose is not on disk, local mode falls back to Basic VAD for the voice detection. For `final_model`, the request returns `model_not_downloaded` instead.

A request that names a model checks it before the work starts:

| Code | When |
|---|---|
| `unknown_model` | The id is not a model of this computer. |
| `wrong_role` | The model cannot do the role, for example a voice detection model as `model`. |
| `model_not_downloaded` | The model is not on disk. Call `download_model`. |
| `unsupported` | The engine lacks a feature that the request needs, for example `vad` or `diarization`. |
| `model_retired` | The model left the catalog. The message says what to use instead. |
