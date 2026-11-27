# Wire corpus

One message per line. Each message line follows a comment line, `# <case>`, that names it.

- `host/`: Host API. `requests.ndjson` goes from the app to the host; `replies.ndjson`, `events.ndjson` go from the host to the app; `edge.ndjson` has names and fields that this version does not know, and large numbers.
- `engine/`: engine protocol (`docs/engine-protocol.md`).
- `cleanup/`: cleanup runtime protocol.

A test of an SDK decodes each message line and encodes it again, and compares the two as JSON values. A field whose value is `null` counts as missing. A case whose name starts with `decode:` only needs to decode without an error: its encoding may differ (a number speaker becomes a string, an unknown field is dropped).

`names.json` lists every name of the protocols. `paths.json` is the table of `../paths.md`. The Go test `sdk/go/protocol` writes `names.json` with `-update` and fails when it is out of date.
