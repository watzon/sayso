# How to react to mode changes in TypeScript

With `mode: "auto"`, the SDK uses Sayso when Sayso runs, and your own engine when it does not. It changes by itself. Your app learns about a change from an event with `kind: "mode_changed"`. This is an event of the SDK, and not a message of the Host API.

The rules are the same as in [How to react to mode changes](react-to-mode-changes.md). That page says when the mode changes and what happens to the work. This page shows what your TypeScript app does.

## Read the event

The event is `{ kind: "mode_changed", mode, features }`. `mode` is `"sayso"` or `"local"`. `features` is the `Features` object of the new mode. The SDK sends the event only when your app has no active work.

Then do this:

1. Read `features`. The mode decides which parts of the API work. Show or hide the buttons of those parts.
2. In Sayso mode, call `speech.subscribe` again if you use `dictation_done`. A subscription belongs to one connection.
3. Do not reuse old ids. Ids of jobs and sessions come from the SDK, and no id comes back after a change.
4. If work ended with `connection_lost` or `host_closing`, decide with the user whether to start it again. The SDK does not.

<!-- check: verbatim packages/sdk/examples/react-to-mode-changes.ts -->
```ts
export async function watchMode(speech: Speech, signal: AbortSignal): Promise<void> {
	for await (const event of speech.events({ signal })) {
		switch (event.kind) {
			case "mode_changed":
				if (event.mode === "sayso") {
					console.error(`Sayso runs now. Enhance: ${event.features.enhance}`);
					if (event.features.subscribe) await speech.subscribe(["dictation_done"]);
				} else {
					console.error("Sayso quit. The app uses its own engine.");
				}
				break;
			case "disconnected":
				console.error(`The connection to Sayso ended: ${event.code}`);
				break;
			case "message":
				if (event.message.type === "dictation_done") console.log(`Dictated: ${event.message.text}`);
				break;
		}
	}
}
```

The `signal` ends the loop. `speech.events()` also ends when you call `speech.close()`.

The loop does not keep Node running. Only active work does that. A program that only waits for mode changes ends when it has no other work. See [How to handle errors and ends](ts-handle-errors-and-ends.md#know-what-keeps-node-running).

## What differs between the modes

`features` tells you. In Sayso mode, `enhance`, `subscribe`, and `dictation_done` work if the user granted them. In local mode they reject with `unsupported`. The full table is in the [Rust page](react-to-mode-changes.md#what-differs-between-the-modes).

## Reconnect in Sayso mode

With `mode: "sayso"`, the SDK does not change mode. A lost connection gives an event with `kind: "disconnected"` and a `code`, and every request rejects with `host_not_running`. The SDK does not connect again by itself. Call `speech.reconnect()` when your app decides to. Work does not survive the end of a connection.

<!-- check: verbatim packages/sdk/examples/react-to-mode-changes.ts -->
```ts
export async function reconnectWhenAsked(speech: Speech): Promise<void> {
	await speech.reconnect();
	console.error(`Connected again. Mode: ${speech.mode()}`);
}
```

## Test it

Start your app with `mode: "auto"` and no Sayso running. `speech.mode()` is `"local"`. Start Sayso. When your app has no work, you get a `mode_changed` event with `mode: "sayso"`. Quit Sayso, and you get `mode_changed` with `mode: "local"`.

We did not run this check by hand with a real Sayso. The tests of the package run it against a real host in a child process.
