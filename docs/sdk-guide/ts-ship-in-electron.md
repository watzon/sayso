# How to ship the SDK in an Electron app

Use `@justsayso/sdk` in the main process of an Electron app. This page shows how to package the addon and the engine, how to set up the permissions of macOS, and how to sign the app.

We tested Electron 45.2.3 on macOS with Apple silicon: in development, and packaged with `@electron/packager` and asar. We did not test Windows or Linux in Electron yet.

## Use the SDK in the main process only

The SDK is a native addon. A renderer cannot load it. Call the SDK in the main process, and send the results to the renderer over IPC.

The events of the SDK are plain objects, so `webContents.send` can carry them. This example connects when the app is ready, answers three IPC calls, and closes the `Speech` before the app quits:

<!-- check: file packages/sdk/examples/electron-main.ts -->
```ts
import { app, BrowserWindow, ipcMain } from "electron";
import { resolve } from "node:path";
import { Engine, Speech, microphonePermission } from "@justsayso/sdk";

let speech: Speech | undefined;

function running(): Speech {
	if (!speech) throw new Error("the speech features are not ready");
	return speech;
}

await app.whenReady();

speech = await Speech.connect({
	mode: "auto",
	app: { name: app.getName() },
	capabilities: ["models.download", "transcribe.file", "session.live", "audio.mic"],
	engine: Engine.fromPackage(),
});

speech.on((event) => {
	for (const window of BrowserWindow.getAllWindows()) {
		if (!window.isDestroyed()) window.webContents.send("speech:event", event);
	}
});

ipcMain.handle("speech:transcribe", (_event, file: string) => running().transcribe({ path: resolve(file) }));

ipcMain.handle("speech:start", async () => {
	if (!(await microphonePermission.request())) return null;
	const { session } = await running().startSession({ sources: ["mic"] });
	return session;
});

ipcMain.handle("speech:stop", (_event, session: number) => running().stopSession(session));

app.on("will-quit", (event) => {
	if (!speech) return;
	event.preventDefault();
	const closing = speech;
	speech = undefined;
	void closing.close().finally(() => app.quit());
});
```

The example is an ES module, because it uses `await` at the top level. A CommonJS main file puts the same code in an `async` function.

The example declares the few Electron types that it uses in `packages/sdk/examples/electron.d.ts`. Your app uses the types of the `electron` package.

`app.on("will-quit")` stops the quit, closes the `Speech`, and quits again. This ends the work and stops the engine before the process ends.

## Install the packages

Put the SDK and the engine in `dependencies`, so your packager includes them:

```sh
npm install @justsayso/sdk @justsayso/engine
```

Add `@justsayso/cleanup` if you use the [local cleanup](ts-use-the-local-cleanup.md).

## Unpack the addon and the engine

A program inside an `app.asar` file cannot start. The engine is a program, so unpack it. Unpack the addon too. Add this to the configuration of `electron-builder`:

```json
{
  "asarUnpack": [
    "**/node_modules/@justsayso/sdk-*/**",
    "**/node_modules/@justsayso/engine-*/**",
    "**/node_modules/@justsayso/cleanup-*/**"
  ]
}
```

Electron Packager and Electron Forge name the option `asar.unpack`, and it takes one glob.

`Engine.fromPackage()` finds the engine with `releaseFolder()` of `@justsayso/engine`. In a packaged app, it changes `app.asar` in the path to `app.asar.unpacked`. If the app says that there is no engine at a path in `app.asar.unpacked`, the packager did not unpack the engine package. Check the globs.

## Add the usage texts to Info.plist

On macOS, the system asks the user for the microphone and the system audio in the name of the app. The capture runs in the main process of your app, so your app needs two keys in its `Info.plist`. Write the texts for your own app, and say why you record:

```xml
<key>NSMicrophoneUsageDescription</key>
<string>Notetaker records your voice to write the notes of a meeting.</string>
<key>NSAudioCaptureUsageDescription</key>
<string>Notetaker records the sound of a call, so it can write down what the others say.</string>
```

- `NSMicrophoneUsageDescription` is for the source `mic`.
- `NSAudioCaptureUsageDescription` is for the source `system`. Without it, the system does not ask in the normal way, and macOS can end a program that asks.

In `electron-builder`, put the keys in `mac.extendInfo`. In Electron Packager, use the option `extendInfo`.

## Add the entitlement

With the hardened runtime, your app needs the entitlement `com.apple.security.device.audio-input` for the microphone. Put it in the entitlements file of your app:

```xml
<key>com.apple.security.device.audio-input</key>
<true/>
```

Turn the hardened runtime on and name the file in the configuration of `electron-builder`:

```json
{
  "mac": {
    "hardenedRuntime": true,
    "entitlements": "build/entitlements.mac.plist",
    "entitlementsInherit": "build/entitlements.mac.plist"
  }
}
```

## Sign the app

npm keeps the Developer ID signature of the engine as it is in the engine release file. A tool that signs your Electron app, for example `@electron/osx-sign`, signs each program in the bundle again with the identity of your app. It does this for the engine too. That is correct. The engine records nothing, and it runs as a child of your app.

## Know the identity of your app

Sayso knows a process by its executable. In Sayso mode, the identity of your app decides what an approval covers:

| How the app runs | Identity on macOS | What an approval covers |
|---|---|---|
| A packaged Electron app, signed by its author | The team and the identifier of the app | This app only, also after updates. |
| A packaged Electron app, not signed (ad hoc) | The path of its executable | This app, at this path. |
| `node app.js` with official Node | `signed:HX7739G8FX/node`, the interpreter flag | Every script of every official Node of this user, also after Node updates. |
| `bun app.ts` | `signed:7FRXF46ZSN/bun`, the interpreter flag | The same for Bun. |
| `deno run app.ts` from Homebrew, or Node from a build that is not signed | The path of the program, the interpreter flag | Every script of that program, at that path. |

An approval for an interpreter is wider than an approval for one app. Ship an app for other people with its own executable, as Electron does.

## Check the packaged app

Build the package and run the app. Transcribe a short file. If the transcription works, the addon and the engine load from `app.asar.unpacked`. On Linux, the machine of the user needs `libvulkan1` and `libasound2`, and glibc 2.35 or newer.
