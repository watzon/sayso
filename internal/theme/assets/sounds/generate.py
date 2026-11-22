#!/usr/bin/env python3
"""Generate the Sayso UI sounds ("paper and ink" set).

Writes four short, quiet WAV files next to this script:

    start.wav   soft pen tap on paper
    stop.wav    slightly lower double tap
    cancel.wav  soft paper swish
    insert.wav  gentle stamp thud

Output format: 44.1 kHz, mono, 16-bit PCM, peak normalized to about -12 dBFS.
Output is deterministic: every sound uses a fixed RNG seed.

Dependencies: numpy and the standard library only.
"""

from __future__ import annotations

import wave
from pathlib import Path

import numpy as np

SAMPLE_RATE = 44_100
TARGET_PEAK = 0.25  # linear, about -12 dBFS
FADE_IN_S = 0.002
FADE_OUT_S = 0.008
SEED = 20240601


# --------------------------------------------------------------------------
# DSP helpers
# --------------------------------------------------------------------------


def time_axis(duration_s: float) -> np.ndarray:
    """Return sample times in seconds for a clip of the given length."""
    return np.arange(int(round(duration_s * SAMPLE_RATE))) / SAMPLE_RATE


def one_pole_lowpass(x: np.ndarray, cutoff_hz: float | np.ndarray) -> np.ndarray:
    """One-pole low-pass filter.

    `cutoff_hz` may be a scalar or a per-sample array, which allows a moving
    (swept) cutoff.
    """
    cutoff = np.broadcast_to(np.asarray(cutoff_hz, dtype=float), x.shape)
    alpha = 1.0 - np.exp(-2.0 * np.pi * cutoff / SAMPLE_RATE)
    y = np.empty_like(x)
    state = 0.0
    for i in range(x.size):
        state += alpha[i] * (x[i] - state)
        y[i] = state
    return y


def band_pass(x: np.ndarray, low_hz: float, high_hz: float) -> np.ndarray:
    """Cheap band-pass: low-pass at `high_hz` minus low-pass at `low_hz`."""
    return one_pole_lowpass(x, high_hz) - one_pole_lowpass(x, low_hz)


def exp_decay(t: np.ndarray, time_constant_s: float) -> np.ndarray:
    """Exponential decay envelope that starts at 1."""
    return np.exp(-t / time_constant_s)


def noise(n: int, rng: np.random.Generator) -> np.ndarray:
    """White noise in [-1, 1]."""
    return rng.uniform(-1.0, 1.0, n)


def tap(
    duration_s: float,
    body_hz: float,
    body_decay_s: float,
    click_band_hz: tuple[float, float],
    click_decay_s: float,
    click_level: float,
    rng: np.random.Generator,
) -> np.ndarray:
    """Single tap: band-passed noise click plus a decaying low sine body."""
    t = time_axis(duration_s)
    click = band_pass(noise(t.size, rng), *click_band_hz)
    click = click * exp_decay(t, click_decay_s) * click_level
    body = np.sin(2.0 * np.pi * body_hz * t) * exp_decay(t, body_decay_s)
    return body + click


def mix_at(target: np.ndarray, clip: np.ndarray, start_s: float, gain: float = 1.0) -> None:
    """Add `clip` into `target` in place, starting at `start_s` seconds."""
    start = int(round(start_s * SAMPLE_RATE))
    end = min(start + clip.size, target.size)
    target[start:end] += gain * clip[: end - start]


def finalize(x: np.ndarray) -> np.ndarray:
    """Apply fades and normalize so the peak equals TARGET_PEAK.

    The fade-in and fade-out guarantee that the first and last samples are
    (near) zero, so playback starts and ends without clicks.
    """
    x = x.copy()
    n_in = int(FADE_IN_S * SAMPLE_RATE)
    n_out = int(FADE_OUT_S * SAMPLE_RATE)
    x[:n_in] *= np.linspace(0.0, 1.0, n_in)
    x[-n_out:] *= np.linspace(1.0, 0.0, n_out)
    peak = np.max(np.abs(x))
    return x * (TARGET_PEAK / peak)


def write_wav(path: Path, x: np.ndarray) -> None:
    """Write a float signal in [-1, 1] as 16-bit mono PCM."""
    pcm = np.round(np.clip(x, -1.0, 1.0) * 32767.0).astype("<i2")
    with wave.open(str(path), "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(SAMPLE_RATE)
        wav.writeframes(pcm.tobytes())


# --------------------------------------------------------------------------
# Sounds: one function each
# --------------------------------------------------------------------------


def make_start() -> np.ndarray:
    """Soft pen tap on paper: short noise click plus ~220 Hz body, ~60 ms."""
    rng = np.random.default_rng(SEED)
    x = tap(
        duration_s=0.060,
        body_hz=220.0,
        body_decay_s=0.014,
        click_band_hz=(600.0, 3500.0),
        click_decay_s=0.004,
        click_level=0.8,
        rng=rng,
    )
    return finalize(x)


def make_stop() -> np.ndarray:
    """Lower double tap: two taps 45 ms apart, ~160 Hz body, ~120 ms."""
    rng = np.random.default_rng(SEED + 1)
    x = np.zeros(int(round(0.120 * SAMPLE_RATE)))
    for start_s, gain in ((0.0, 1.0), (0.045, 0.8)):
        clip = tap(
            duration_s=0.070,
            body_hz=160.0,
            body_decay_s=0.016,
            click_band_hz=(450.0, 2800.0),
            click_decay_s=0.004,
            click_level=0.7,
            rng=rng,
        )
        mix_at(x, clip, start_s, gain)
    return finalize(x)


def make_cancel() -> np.ndarray:
    """Soft paper swish: ~180 ms noise, sweeping low-pass, bell envelope."""
    rng = np.random.default_rng(SEED + 2)
    t = time_axis(0.180)
    progress = t / t[-1]
    # Cutoff sweeps down from bright to dull, like a sheet sliding away.
    cutoff = 5000.0 * np.exp(progress * np.log(900.0 / 5000.0))
    swish = one_pole_lowpass(noise(t.size, rng), cutoff)
    # Remove low rumble so it stays airy rather than boomy.
    swish -= one_pole_lowpass(swish, 300.0)
    # Smooth bell-shaped (Hann) amplitude envelope.
    envelope = np.sin(np.pi * progress) ** 2
    return finalize(swish * envelope)


def make_insert() -> np.ndarray:
    """Gentle stamp thud: ~110 Hz sine gliding down, tiny noise transient, ~90 ms."""
    rng = np.random.default_rng(SEED + 3)
    t = time_axis(0.090)
    # Frequency glides from 125 Hz to 95 Hz; integrate it to get phase.
    freq = 95.0 + 30.0 * np.exp(-t / 0.020)
    phase = 2.0 * np.pi * np.cumsum(freq) / SAMPLE_RATE
    body = np.sin(phase) * exp_decay(t, 0.025)
    transient = one_pole_lowpass(noise(t.size, rng), 1800.0)
    transient *= exp_decay(t, 0.003) * 0.35
    return finalize(body + transient)


# --------------------------------------------------------------------------
# Entry point
# --------------------------------------------------------------------------


def main() -> None:
    out_dir = Path(__file__).parent
    sounds = {
        "start.wav": make_start,
        "stop.wav": make_stop,
        "cancel.wav": make_cancel,
        "insert.wav": make_insert,
    }
    for name, make in sounds.items():
        write_wav(out_dir / name, make())
        print(f"wrote {out_dir / name}")


if __name__ == "__main__":
    main()
