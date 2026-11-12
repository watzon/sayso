// Each event goes to every listener and every iterator (docs/sdk-typescript.md, "Events").

/** Gives each event to every listener and to every open iterator, from the time it started. */
export class EventHub<T> {
	#listeners = new Set<(event: T) => void>();
	#ended = false;

	/** Give an event to all. A listener that throws does not stop the others. */
	emit(event: T): void {
		for (const listener of [...this.#listeners]) {
			try {
				listener(event);
			} catch (e) {
				queueMicrotask(() => {
					throw e;
				});
			}
		}
	}

	/** No more events come: the iterators end after the events that wait. */
	end(): void {
		if (this.#ended) return;
		this.#ended = true;
		for (const listener of [...this.#listeners]) {
			(listener as ((event: T) => void) & { end?: () => void }).end?.();
		}
		this.#listeners.clear();
	}

	get ended(): boolean {
		return this.#ended;
	}

	/** Call `listener` for each event from now on. The result removes it. */
	on(listener: (event: T) => void): () => void {
		if (this.#ended) return () => {};
		this.#listeners.add(listener);
		return () => {
			this.#listeners.delete(listener);
		};
	}

	/**
	 * The events from now on, in order. They wait in the iterator until it reads them. A
	 * `break` out of `for await`, or the signal, stops it.
	 */
	iterate(signal?: AbortSignal): AsyncIterableIterator<T> {
		const queue: T[] = [];
		let wake: (() => void) | undefined;
		let done = this.#ended;
		const listener = Object.assign(
			(event: T) => {
				queue.push(event);
				wake?.();
			},
			{
				end: () => {
					done = true;
					wake?.();
				},
			},
		);
		const stop = () => {
			done = true;
			queue.length = 0;
			this.#listeners.delete(listener);
			signal?.removeEventListener("abort", stop);
			wake?.();
		};
		if (!done) this.#listeners.add(listener);
		if (signal?.aborted) stop();
		signal?.addEventListener("abort", stop, { once: true });
		const iterator: AsyncIterableIterator<T> = {
			next: async (): Promise<IteratorResult<T>> => {
				for (;;) {
					if (queue.length > 0) return { value: queue.shift() as T, done: false };
					if (done) return { value: undefined, done: true };
					await new Promise<void>((resolve) => {
						wake = resolve;
					});
					wake = undefined;
				}
			},
			return: async (): Promise<IteratorResult<T>> => {
				stop();
				return { value: undefined, done: true };
			},
			[Symbol.asyncIterator]() {
				return iterator;
			},
		};
		return iterator;
	}
}

/**
 * Holds the event loop of Node while work is active, so a process does not end before the
 * last event of its work. Without work, the process can end.
 */
export class LoopHold {
	#count = 0;
	#timer: ReturnType<typeof setInterval> | undefined;

	add(): void {
		this.#count += 1;
		this.#timer ??= setInterval(() => {}, 1 << 30);
	}

	remove(): void {
		if (this.#count === 0) return;
		this.#count -= 1;
		if (this.#count === 0) this.clear();
	}

	clear(): void {
		this.#count = 0;
		if (this.#timer !== undefined) {
			clearInterval(this.#timer);
			this.#timer = undefined;
		}
	}
}

/**
 * The active work of one connection: jobs and live sessions, by id. Each counts in the
 * `LoopHold` from its start to its end. An end that arrives before the start reply is kept,
 * so the work does not count.
 */
export class WorkSet {
	readonly #hold: LoopHold;
	readonly #active = new Set<string>();
	readonly #endedEarly = new Set<string>();

	constructor(hold: LoopHold) {
		this.#hold = hold;
	}

	started(key: string): void {
		if (this.#endedEarly.delete(key) || this.#active.has(key)) return;
		this.#active.add(key);
		this.#hold.add();
	}

	ended(key: string): void {
		if (this.#active.delete(key)) {
			this.#hold.remove();
		} else {
			this.#endedEarly.add(key);
			// The set stays small: an early end is rare, and its start reply comes soon.
			if (this.#endedEarly.size > 1000) this.#endedEarly.clear();
		}
	}

	clear(): void {
		for (const _ of this.#active) this.#hold.remove();
		this.#active.clear();
		this.#endedEarly.clear();
	}
}
