// A streaming sample-rate converter with a windowed sinc: polyphase when the ratio is small, direct otherwise
// (a port of the resampler of sayso-speech).

const ZERO_CROSSINGS = 10;
const MAX_PHASES = 1024;

function gcd(a: number, b: number): number {
	while (b !== 0) [a, b] = [b, a % b];
	return a;
}

function sinc(x: number): number {
	return Math.abs(x) < 1e-12 ? 1 : Math.sin(Math.PI * x) / (Math.PI * x);
}

function hann(t: number): number {
	return 0.5 * (1 + Math.cos(Math.PI * t));
}

interface Phase {
	first: number;
	at: number;
	len: number;
}

function append(a: Float32Array, b: Float32Array): Float32Array {
	const out = new Float32Array(a.length + b.length);
	out.set(a);
	out.set(b, a.length);
	return out;
}

export class StreamResampler {
	#kind: 0 | 1 | 2 = 0;
	#buf: Float32Array = new Float32Array(0);
	// polyphase
	#p = 0;
	#q = 0;
	#phases: Phase[] = [];
	#weights: Float32Array = new Float32Array(0);
	#index = 0;
	#phase = 0;
	// direct
	#step = 0;
	#scale = 0;
	#halfWidth = 0;
	#pos = 0;

	constructor(inRate: number, outRate: number) {
		if (!(inRate > 0 && outRate > 0)) throw new Error("sample rates must be positive");
		if (inRate === outRate) return;
		const d = gcd(inRate, outRate);
		const p = inRate / d;
		const q = outRate / d;
		if (q <= MAX_PHASES) this.#polyphase(p, q);
		else this.#direct(inRate, outRate);
	}

	#polyphase(p: number, q: number): void {
		this.#kind = 1;
		this.#p = p;
		this.#q = q;
		const half = ZERO_CROSSINGS * Math.max(p, q);
		const scale = Math.max(p / q, 1);
		const halfWidth = half / q;
		const weights: number[] = [];
		for (let phase = 0; phase < q; phase++) {
			const first = -Math.floor((half - phase) / q);
			const last = Math.floor((phase + half) / q);
			const at = weights.length;
			const exact: number[] = [];
			for (let tap = first; tap <= last; tap++) {
				const x = tap - phase / q;
				exact.push(sinc(x / scale) * hann(x / halfWidth));
			}
			const norm = exact.reduce((a, b) => a + b, 0);
			for (const w of exact) weights.push(Math.abs(norm) > 1e-9 ? w / norm : 0);
			this.#phases.push({ first, at, len: exact.length });
		}
		this.#weights = Float32Array.from(weights);
		const lead = Math.floor((half + q - 1) / q);
		this.#index = lead;
		this.#buf = new Float32Array(lead);
	}

	#direct(inRate: number, outRate: number): void {
		this.#kind = 2;
		this.#step = inRate / outRate;
		this.#scale = Math.max(this.#step, 1);
		this.#halfWidth = ZERO_CROSSINGS * this.#scale;
		const lead = Math.ceil(this.#halfWidth);
		this.#pos = lead;
		this.#buf = new Float32Array(lead);
	}

	/** The resampled samples of `input`. The converter keeps the rest for the next call. */
	process(input: Float32Array): Float32Array {
		if (this.#kind === 0) return input;
		this.#buf = append(this.#buf, input);
		const out: number[] = [];
		const keep = this.#kind === 1 ? this.#runPolyphase(out) : this.#runDirect(out);
		this.#buf = this.#buf.slice(keep);
		return Float32Array.from(out);
	}

	#runPolyphase(out: number[]): number {
		const buf = this.#buf;
		for (;;) {
			const ph = this.#phases[this.#phase] as Phase;
			const first = this.#index + ph.first;
			if (first < 0 || first + ph.len > buf.length) break;
			let acc = 0;
			for (let i = 0; i < ph.len; i++) acc += (buf[first + i] as number) * (this.#weights[ph.at + i] as number);
			out.push(acc);
			this.#phase += this.#p;
			this.#index += Math.floor(this.#phase / this.#q);
			this.#phase %= this.#q;
		}
		const keep = Math.min(Math.max(this.#index + (this.#phases[this.#phase] as Phase).first, 0), buf.length);
		this.#index -= keep;
		return keep;
	}

	#runDirect(out: number[]): number {
		const buf = this.#buf;
		for (;;) {
			const first = Math.ceil(this.#pos - this.#halfWidth);
			const last = Math.floor(this.#pos + this.#halfWidth);
			if (last >= buf.length) break;
			let acc = 0;
			let norm = 0;
			for (let i = first; i <= last; i++) {
				const x = i - this.#pos;
				const w = sinc(x / this.#scale) * hann(x / this.#halfWidth);
				acc += (buf[i] ?? 0) * w;
				norm += w;
			}
			out.push(Math.abs(norm) > 1e-9 ? acc / norm : 0);
			this.#pos += this.#step;
		}
		const keep = Math.min(Math.max(Math.floor(this.#pos - this.#halfWidth), 0), buf.length);
		this.#pos -= keep;
		return keep;
	}
}
