import { SaysoError } from "./errors.js";
import { EventHub } from "./events.js";
import { APP_ID, launchers as defaultLaunchers, startSayso as startInstalled, type Launcher } from "./launch.js";
import { clientSocketPath } from "./paths.js";
import { decodeHost, encodeMessage, type UnknownEvent } from "./protocol/codec.js";
import type { Capability, ErrorCode, ErrorReply, Event as Message, HelloReply } from "./protocol/index.js";
import { dial, type Transport } from "./transport.js";
import { VERSION } from "./version.js";

export type { Launcher } from "./launch.js";

/** An event of the Host API: a known one, or one of a type that this version does not know. */
export type HostEvent = Message | UnknownEvent;

/** Something that the client gets from Sayso without asking. */
export type ClientEvent =
	/** An event of the Host API. `id` is the id of the request that caused it, when a request caused it. */
	| { kind: "host"; id: string | null; event: HostEvent }
	/** An `error` without an `id`: a `session_audio` of `send` failed, or Sayso could not read a line. */
	| { kind: "error"; error: ErrorReply }
	/** Sayso quit or restarted, or the connection broke. Requests fail until `reconnect()`. `code` is `host_closing` or `connection_lost`. */
	| { kind: "host_gone"; code?: ErrorCode };

/** How `Client.connect` connects. */
export interface ClientOptions {
	/** The socket (the pipe on Windows). The default is the rule of the protocol (`SAYSO_HOST_SOCKET`, or the path of this system). */
	path?: string;
	/** What the app tells Sayso in `hello`, for the approval question. */
	app?: { name?: string; capabilities?: Capability[] };
	/** Start Sayso when it does not run, and wait for it. Default false. */
	start?: boolean;
	/** How long to wait for Sayso after the start. Default `startWaitMs()`. */
	waitMs?: number;
	/** How to start Sayso. The default tries `launchers`. */
	startSayso?: () => void | Promise<void>;
	/** The ways to start Sayso for the default `startSayso`. The default is `launchers()`. */
	launchers?: Launcher[];
	/** How long the first reply to `hello` can take, unless Sayso said that it asks the user. Default 10000. */
	helloLimitMs?: number;
	/** Called when Sayso asks the user about this app. */
	onApprovalWaiting?: () => void;
}

/** The bundle id of Sayso on macOS, and its app id on Linux. */
export function appId(): string {
	return APP_ID;
}

/** The name and version of this SDK, in `hello`. */
export function sdkName(): string {
	return `sayso-ts/${VERSION}`;
}

/** How long `Client.connect` with `start` waits for Sayso, in milliseconds. */
export function startWaitMs(): number {
	return 10_000;
}

export { startSayso, launchers } from "./launch.js";

interface Pending {
	kind: string;
	fields: Record<string, unknown>;
	resolve(fields: Record<string, unknown>): void;
	reject(error: SaysoError): void;
	onEvent?: (event: HostEvent) => void;
}

/** One connection to the host, with the work that it started. */
class Conn {
	next = 0;
	readonly pending = new Map<string, Pending>();
	readonly jobs = new Set<number>();
	readonly sessions = new Set<number>();
	closing = false;
	local = false;
	ended = false;
	transport!: Transport;
	hello!: HelloReply;
}

const DEFAULT_HELLO_LIMIT_MS = 10_000;

/**
 * The plain client of the Host API: one connection to Sayso, with no local mode. Most apps use `Speech`.
 * Use this when you need the messages of the Host API as they are.
 *
 * A connection that ends fails its open requests with `host_closing` (after the event `host_closing`) or
 * `connection_lost`, and gives each job and live session that it started one end: `job_failed` or
 * `session_ended` with that code. Then `host_gone` comes.
 */
export class Client implements AsyncDisposable {
	readonly #options: ClientOptions;
	readonly #events = new EventHub<ClientEvent>();
	#conn: Conn | undefined;
	#closed: Promise<void> | undefined;

	private constructor(options: ClientOptions) {
		this.#options = options;
	}

	/**
	 * Connect and send `hello`. When Sayso has no answer about this app yet, this waits until the user answers
	 * the approval question.
	 */
	static async connect(options: ClientOptions = {}): Promise<Client> {
		const client = new Client(options);
		try {
			await client.#open(options.start === true);
		} catch (e) {
			client.#events.end();
			throw e;
		}
		return client;
	}

	#socketPath(): string {
		if (this.#options.path !== undefined && this.#options.path !== "") return this.#options.path;
		try {
			return clientSocketPath();
		} catch (e) {
			throw SaysoError.of("host_not_running", `cannot find the socket of Sayso: ${(e as Error).message}`);
		}
	}

	async #open(start: boolean): Promise<void> {
		const path = this.#socketPath();
		let conn: Conn;
		try {
			conn = await this.#dial(path);
		} catch (e) {
			if (!start || !(e instanceof SaysoError) || e.code !== "host_not_running") throw e;
			await this.#startAndWait(path, e);
			conn = await this.#dial(path);
		}
		try {
			conn.hello = await this.#hello(conn);
		} catch (e) {
			conn.local = true;
			conn.transport.destroy();
			throw e;
		}
		this.#conn = conn;
	}

	async #startAndWait(path: string, first: SaysoError): Promise<void> {
		const options = this.#options;
		if (options.startSayso) await options.startSayso();
		else await startInstalled(options.launchers ?? defaultLaunchers());
		const deadline = Date.now() + (options.waitMs ?? startWaitMs());
		let last = first;
		for (;;) {
			try {
				(await this.#dial(path)).transport.destroy();
				return;
			} catch (e) {
				last = e instanceof SaysoError ? e : first;
				if (last.code !== "host_not_running" || Date.now() >= deadline) throw last;
			}
			await new Promise((resolve) => setTimeout(resolve, 100));
		}
	}

	async #dial(path: string): Promise<Conn> {
		const conn = new Conn();
		conn.transport = await dial(path, {
			onLine: (line) => this.#line(conn, line),
			onClose: () => this.#finish(conn),
		});
		return conn;
	}

	async #hello(conn: Conn): Promise<HelloReply> {
		const options = this.#options;
		const app = options.app;
		const fields: Record<string, unknown> = { sdk: sdkName(), protocol: 1, features: [] };
		if (app?.name) fields.app = { name: app.name };
		if (app?.capabilities && app.capabilities.length > 0) fields.capabilities = app.capabilities;
		let limit: NodeJS.Timeout | undefined;
		const timed = new Promise<never>((_, reject) => {
			limit = setTimeout(() => reject(SaysoError.of("host_not_running", "Sayso did not answer hello in time")), options.helloLimitMs ?? DEFAULT_HELLO_LIMIT_MS);
		});
		try {
			const reply = this.#send<HelloReply>(conn, "hello", fields, (event) => {
				if (event.type === "approval_waiting") {
					clearTimeout(limit);
					options.onApprovalWaiting?.();
				}
			});
			return (await Promise.race([reply, timed])) as HelloReply;
		} finally {
			clearTimeout(limit);
		}
	}

	#endError(conn: Conn): SaysoError {
		if (conn.closing) return SaysoError.of("host_closing", "Sayso quit");
		return SaysoError.of("connection_lost", conn.local ? "the connection is closed" : "the connection to Sayso ended");
	}

	#send<T>(conn: Conn, kind: string, fields: Record<string, unknown>, onEvent?: (event: HostEvent) => void): Promise<T> {
		if (conn.ended) return Promise.reject(this.#endError(conn));
		const id = String(++conn.next);
		return new Promise<T>((resolve, reject) => {
			conn.pending.set(id, { kind, fields, resolve: resolve as (fields: Record<string, unknown>) => void, reject, ...(onEvent ? { onEvent } : {}) });
			try {
				conn.transport.write(encodeMessage(id, { type: kind, ...fields }));
			} catch (e) {
				conn.pending.delete(id);
				reject(SaysoError.from(e));
			}
		});
	}

	#line(conn: Conn, line: string): void {
		let decoded;
		try {
			decoded = decodeHost(line);
		} catch {
			return;
		}
		switch (decoded.kind) {
			case "ok": {
				const pending = decoded.id === undefined ? undefined : conn.pending.get(decoded.id);
				if (!pending || decoded.id === undefined) return;
				conn.pending.delete(decoded.id);
				const job = decoded.fields.job;
				const session = decoded.fields.session;
				if (pending.kind === "transcribe_file" && typeof job === "number") conn.jobs.add(job);
				if (pending.kind === "session_start" && typeof session === "number") conn.sessions.add(session);
				if (pending.kind === "session_stop" && typeof pending.fields.session === "number") conn.sessions.delete(pending.fields.session);
				pending.resolve(decoded.fields);
				return;
			}
			case "error": {
				if (decoded.id === undefined) {
					this.#events.emit({ kind: "error", error: decoded.error });
					return;
				}
				const pending = conn.pending.get(decoded.id);
				if (!pending) return;
				conn.pending.delete(decoded.id);
				pending.reject(new SaysoError(decoded.error));
				return;
			}
			case "event":
				this.#event(conn, decoded.id, decoded.event);
				return;
			case "unknown":
				this.#event(conn, decoded.id, decoded.event);
				return;
		}
	}

	#event(conn: Conn, id: string | undefined, event: HostEvent): void {
		switch (event.type) {
			case "job_done":
			case "job_failed":
				conn.jobs.delete((event as { job: number }).job);
				break;
			case "session_ended":
				conn.sessions.delete((event as { session: number }).session);
				break;
			case "host_closing":
				conn.closing = true;
				break;
		}
		const pending = id === undefined ? undefined : conn.pending.get(id);
		pending?.onEvent?.(event);
		this.#events.emit({ kind: "host", id: id ?? null, event });
	}

	#finish(conn: Conn): void {
		if (conn.ended) return;
		conn.ended = true;
		const error = this.#endError(conn);
		const code = conn.closing ? "host_closing" : conn.local ? undefined : "connection_lost";
		for (const pending of conn.pending.values()) pending.reject(error);
		conn.pending.clear();
		if (code === undefined) return;
		for (const job of conn.jobs) this.#events.emit({ kind: "host", id: null, event: { type: "job_failed", job, message: error.message, code } });
		for (const session of conn.sessions) this.#events.emit({ kind: "host", id: null, event: { type: "session_ended", session, text: "", code } });
		conn.jobs.clear();
		conn.sessions.clear();
		this.#events.emit({ kind: "host_gone", code });
	}

	#live(): Conn {
		const conn = this.#conn;
		if (!conn) throw SaysoError.of("connection_lost", "the client is not connected");
		return conn;
	}

	/** The reply of Sayso to `hello`: its version, the features, the granted capabilities, and the refused ones. */
	hello(): HelloReply {
		return this.#live().hello;
	}

	/** The optional parts of the Host API that this Sayso serves. Empty for a Sayso older than the list. */
	hostFeatures(): string[] {
		return [...(this.#live().hello.features ?? [])];
	}

	/** True when this Sayso serves the feature. A Sayso with no feature list is older than the list, so it counts as serving all. */
	supports(feature: string): boolean {
		const features = this.#live().hello.features ?? [];
		return features.length === 0 || features.includes(feature);
	}

	/** True while the connection is open. */
	get connected(): boolean {
		return this.#conn !== undefined && !this.#conn.ended;
	}

	/** The events from now on. Each iterator and each listener gets every event. */
	events(options?: { signal?: AbortSignal }): AsyncIterableIterator<ClientEvent> {
		return this.#events.iterate(options?.signal);
	}

	/** Call `listener` for each event from now on. The result removes it. */
	on(listener: (event: ClientEvent) => void): () => void {
		return this.#events.on(listener);
	}

	/**
	 * Send the request `kind` with `fields`, and wait for its reply: the fields of `ok`. `onEvent` gets the
	 * events with the `id` of this request, before the reply.
	 */
	request<T = Record<string, unknown>>(kind: string, fields: Record<string, unknown> = {}, onEvent?: (event: HostEvent) => void): Promise<T> {
		let conn: Conn;
		try {
			conn = this.#live();
		} catch (e) {
			return Promise.reject(e);
		}
		return this.#send<T>(conn, kind, fields, onEvent);
	}

	/**
	 * Send the request `kind` with `fields` and no `id`, and do not wait. Sayso sends no `ok` for it; an `error`
	 * comes as an event. For `session_audio`, which goes many times each second.
	 */
	send(kind: string, fields: Record<string, unknown> = {}): void {
		const conn = this.#live();
		if (conn.ended) throw this.#endError(conn);
		conn.transport.write(encodeMessage(undefined, { type: kind, ...fields }));
	}

	/** Tell the client that a session ended with `session_stop`, so a later end of the connection does not end it again. */
	forgetSession(session: number): void {
		this.#conn?.sessions.delete(session);
	}

	/** Connect again after `host_gone`, and send `hello` again. Jobs and live sessions do not survive a restart of Sayso. */
	async reconnect(): Promise<HelloReply> {
		const old = this.#conn;
		if (old && !old.ended) {
			old.local = true;
			old.transport.destroy();
		}
		this.#conn = undefined;
		await this.#open(false);
		return this.#live().hello;
	}

	/** Send `bye` and close the connection. */
	close(): Promise<void> {
		this.#closed ??= (async () => {
			const conn = this.#conn;
			try {
				if (conn && !conn.ended) {
					conn.local = true;
					await Promise.race([this.#send(conn, "bye", {}).catch(() => undefined), new Promise((resolve) => setTimeout(resolve, 2000).unref())]);
					conn.transport.destroy();
				}
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
