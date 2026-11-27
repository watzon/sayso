# Audio helper protocol, version 1

`sayso-audio` is a small program that captures audio and decodes audio files for an SDK that cannot do it itself (the TypeScript SDK). The SDK starts it as a child process, one process for each command.

- Output is NDJSON on stdout: one JSON object on one line, with `"v": 1` and a string `type`.
- Stderr is free text for people.
- A capture runs until stdin closes or the process gets `SIGTERM`. Other commands exit when they are done.
- The exit code is 0 after `done` or a clean stop, and 1 after `error` or `ended`.

## Audio

All audio is 16 kHz mono little-endian 16-bit PCM in base64, in frames of 20 ms (320 samples):

```json
{"v":1,"type":"audio","t_ms":120,"pcm":"AAABAP7/..."}
```

`t_ms` is the time of the first sample of the frame, in milliseconds from the start of the command. Captures of one machine share one clock, so two captures started at the same time can be compared.

## Commands

### `devices`

```
sayso-audio devices
{"v":1,"type":"devices","devices":[{"id":"BuiltInMicrophoneDevice","name":"MacBook Pro Microphone","default":true}]}
```

### `permission`

```
sayso-audio permission [--request] mic|system
{"v":1,"type":"permission","state":"granted"}
```

`state` is `granted`, `denied`, or `unknown` (the user was not asked). With `--request`, the helper asks the user when the state is `unknown`, and gives the answer.

### `capture`

```
sayso-audio capture --source mic|system [--device ID] [--exclude-pid N]
{"v":1,"type":"started","excludes":["sayso","app"]}
{"v":1,"type":"audio","t_ms":0,"pcm":"..."}
{"v":1,"type":"gap","ms":180}
{"v":1,"type":"ended","reason":"permission","message":"the user took back the permission"}
```

- `started` comes once, before the first audio. `excludes` lists what a `system` capture leaves out: `app` (the process of `--exclude-pid`) and `sayso`. It is empty for `mic`.
- `gap` says that the capture lost `ms` of audio, for example at a change of the device.
- `ended` says that the capture stopped before stdin closed. `reason` is `permission` or `failed`.
- A capture that cannot start writes `error` and exits.

### `decode`

```
sayso-audio decode /absolute/path/meeting.m4a
{"v":1,"type":"audio","t_ms":0,"pcm":"..."}
{"v":1,"type":"done","ms":61240}
```

`done` gives the length of the audio. The helper resamples and mixes to 16 kHz mono.

## Errors

```json
{"v":1,"type":"error","code":"unsupported","message":"this system cannot decode AAC"}
```

| `code` | Meaning |
|---|---|
| `unsupported` | this system or this build cannot do it |
| `permission` | the system refused the permission |
| `bad_request` | a wrong command or argument |
| `failed` | anything else; `message` says what |

## Tests

With `SAYSO_AUDIO_FAKE_WAV` set to a WAV file, `capture` plays that file as the device, at the speed of real time, and then sends silence. `devices` then lists one device `fake`.
