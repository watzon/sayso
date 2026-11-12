import { EventHub } from "./events.js";
import { bridge, native, settle, unwrap, type NativeClient } from "./native.js";
import type { Capability, ErrorReply, Event as Message, HelloReply } from "./protocol/index.js";

/** Something that the client gets from Sayso without asking. */
export type ClientEvent =
	/** An event of the Host API. `id` is the id of the request that caused it, when a request caused it. */
	| { kind: "host"; id: string | null; event: Message }
	/** An `error` without an `id`: a `session_audio` of `send` failed, or Sayso could not read a line. */
	| { kind: "error"; error: ErrorReply }
	/** Sayso quit or restarted, or the connection broke. Requests fail until `reconnect()`. */
	| { kind: "host_gone" };

/** How `Client.connect` connects. */
export interface ClientOptions {
	/** The socket (the pipe on Windows). The default is the rule of the protocol (`SAYSO_HOST_SOCKET`, or the path of this system). */
	path?: string;
	/** What the app tells Sayso in `hello`, for the approval question. */
	app?: { name?: string; capabilities?: Capability[] };
	/** Start Sayso when it does not run, and wait for it. Default false. */
	start?: boolean;
	/** How long to wait for Sayso after the start. Default `START_WAIT_MS`. */
	waitMs?: number;
	/** How to start Sayso. The default is `startSayso()`. */
	startSayso?: () => void | Promise<void>;
}

interface Constants {
	appId: string;
	sdk: string;
	startWaitMs: number;
}

let constants: Constants | undefined;
function clientConstants(): Constants {
	constants ??= JSON.parse(native().clientConstants()) as Constants;
	return constants;
}

/** The bundle id of Sayso on macOS, and its app id on Linux. */
export function appId(): string {
	return clientConstants().appId;
}

/** The name and version of this SDK, in `hello`. */
export function sdkName(): string {
	return clientConstants().sdk;
}

/** How long `Client.connect` with `start` waits for Sayso, in milliseconds. */
export function startWaitMs(): number {
	return clientConstants().startWaitMs;
}

/** One way to start Sayso: a program and its arguments. */
export interface Launcher {
	program: string;
	args: string[];
}

/** The ways to start Sayso on this system, in the order to try them. Empty when Sayso is not installed in a place that the rules know. */
export function launchers(): Launcher[] {
	return JSON.parse(native().launchers()) as Launcher[];
}

/** Start the installed Sayso. It does not wait for it. */
export function startSayso(): void {
	unwrap<null>(native().startSayso());
}

/**
 * The plain client of the Host API: one connection to Sayso, with no local mode. Most apps
 * use `Speech`. Use this when you need the messages of the Host API as they are.
 */
export class Client implements AsyncDisposable {
	readonly #native: NativeClient;
	readonly #events = new EventHub<ClientEvent>();
	#closed: Promise<void> | undefined;

	private constructor(handle: NativeClient) {
		this.#native = handle;
	}

	/**
	 * Connect and send `hello`. When Sayso has no answer about this app yet, this waits until
	 * the user answers the approval question.
	 */
	static async connect(options: ClientOptions = {}): Promise<Client> {
		const { startSayso: start, ...rest } = options;
		const handle = new (native().NativeClient)();
		const startFn = start ? bridge({ start: async () => void (await start()) }) : undefined;
		await settle<HelloReply>(handle.connect(JSON.stringify(rest), startFn));
		const client = new Client(handle);
		handle.listen((text) => {
			try {
				client.#events.emit(unwrap<ClientEvent>(text));
			} catch {
				// A value that JavaScript cannot keep. The event cannot reach the app.
			}
		});
		return client;
	}

	/** The reply of Sayso to `hello`: its version, the granted capabilities, and the refused ones. */
	hello(): HelloReply {
		return unwrap<HelloReply>(this.#native.hello());
	}

	/** The events from now on. Each iterator and each listener gets every event. */
	events(options?: { signal?: AbortSignal }): AsyncIterableIterator<ClientEvent> {
		return this.#events.iterate(options?.signal);
	}

	/** Call `listener` for each event from now on. The result removes it. */
	on(listener: (event: ClientEvent) => void): () => void {
		return this.#events.on(listener);
	}

	/** Send the request `kind` with `fields`, and wait for its reply: the fields of `ok`. */
	request<T = Record<string, unknown>>(kind: string, fields: Record<string, unknown> = {}): Promise<T> {
		return settle<T>(this.#native.request(kind, JSON.stringify(fields)));
	}

	/**
	 * Send the request `kind` with `fields` and no `id`, and do not wait. Sayso sends no `ok`
	 * for it; an `error` comes as an event. For `session_audio`, which goes many times each second.
	 */
	send(kind: string, fields: Record<string, unknown> = {}): void {
		unwrap<null>(this.#native.send(kind, JSON.stringify(fields)));
	}

	/** Connect again after `host_gone`, and send `hello` again. Jobs and live sessions do not survive a restart of Sayso. */
	reconnect(): Promise<HelloReply> {
		return settle<HelloReply>(this.#native.reconnect());
	}

	/** Send `bye` and close the connection. */
	close(): Promise<void> {
		this.#closed ??= (async () => {
			try {
				await settle<null>(this.#native.close());
			} finally {
				this.#events.end();
			}
		})();
		return this.#closed;
	}

	[Symbol.asyncDispose](): Promise<void> {
		return this.close();
	}
}
