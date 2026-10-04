# sayso-enhance

`Enhancer` implementations for `sayso_core::enhance`. All calls block.

## API

- `build_enhancer(&Provider, &dyn SecretStore, &Ai, Option<Arc<dyn LanguageModel>>) -> Box<dyn Enhancer>`. Never fails. A provider that cannot work (missing API key, missing binary) returns `NotConfigured` from `enhance`. `Ai` timeouts are the default for requests with a zero timeout.
- `SecretStore` with `KeychainStore` (service `dev.sayso.Sayso`) and `MemoryStore`.
- `OpenAiCompatible`: `POST {base_url}/chat/completions`. Mode ladder `json_schema`, `json_object`, `tool_call`. Next mode on HTTP 400/422 or a reply that does not match the schema. Working mode cached per `(base_url, model)` for the process (`clear_mode_cache()`). One whole-call deadline. OpenRouter extras: `provider.require_parameters` (json_schema and tool_call modes), `zdr` and `data_collection: "deny"` with `zero_data_retention`. Builder: `with_api_key`, `with_zero_data_retention`, `with_cloud`, `with_openrouter`, `with_default_timeout`.
- `ClaudeCli::new(id, path: Option<PathBuf>, model)`. Transcript on stdin, schema in `--json-schema`, `MAX_THINKING_TOKENS=0`, temp cwd, kill on timeout. Reads `structured_output`. Fails on `is_error` or a non-success `subtype`.
- `CodexCli::new(id, path, model)`. Prompt (instructions plus transcript) on stdin with `-`, schema and answer files in a temp dir, kill on timeout.
- `EngineModel::new(id, Option<Arc<dyn LanguageModel>>)`: the language model that the engine runs on this computer (Apple Intelligence on macOS, provider kind `apple_intelligence`). It sends the style prompt as instructions and the tagged transcript as the prompt, then applies the same checks to the text as the other providers. Not cloud. Without an engine it returns `NotConfigured`. This crate does not depend on the engine client: the app passes the handle in.
- Detection: `detect_claude_cli`, `detect_codex_cli`, `detect_ollama(timeout)`, `detect_ollama_at(url, timeout)`, `list_models(base_url, key)`, `test_provider(&dyn Enhancer)`.

## Tested (no network, no keys)

Mock `tiny_http` server: strict json_schema, fallback after 400 and 422, fallback when the reply ignores the format, forced tool call, mode cache, invalid JSON, wrong schema, timeout (also across the ladder and with the default timeout), OpenRouter extras, no extras for other hosts, Authorization header from the secret store, 401 not retried, refusal, unreachable server, `list_models`, `detect_ollama_at`, `test_provider`.
Fake `claude` and `codex` scripts: arguments, environment, stdin, temp cwd removed, success, error envelope, non-success subtype, crash with stderr, invalid output, timeout with child killed, missing binary, a helper that keeps the pipe open.
Unit tests: request bodies, reply parsing, envelope parsing, executable search, factory.

## Not tested

Real providers, the real `claude` and `codex` commands (the `claude -p` stdin input and the `codex exec -` stdin input follow the CLI help and docs, but no model call was made), `KeychainStore` (it would prompt for Keychain access), HTTPS and TLS, streaming.
