// Each event goes to every listener and to every iterator.

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
