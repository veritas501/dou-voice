#!/usr/bin/env python3
"""Generate short 8-bit voice input feedback sounds."""

from __future__ import annotations

import math
import wave
from pathlib import Path

SAMPLE_RATE = 22050
SILENCE_MS = 18
INTER_TONE_GAP_MS = 5
FADE_MS = 4
BASE_AMPLITUDE = 42
GAIN_DB = -10.0
AMPLITUDE = BASE_AMPLITUDE * (10 ** (GAIN_DB / 20))
CENTER = 128

ROOT = Path(__file__).resolve().parents[1]
OUTPUT_DIR = ROOT / "crates" / "dou-voice-platform" / "assets" / "sounds"


def square_sample(phase: float, level: float) -> int:
    value = AMPLITUDE if math.sin(phase) >= 0 else -AMPLITUDE
    return max(0, min(255, round(CENTER + value * level)))


def render_tone(frequency_hz: float, duration_ms: int) -> bytearray:
    sample_count = round(SAMPLE_RATE * duration_ms / 1000)
    fade_count = max(1, round(SAMPLE_RATE * FADE_MS / 1000))
    samples = bytearray()
    for index in range(sample_count):
        attack = min(1.0, index / fade_count)
        release = min(1.0, (sample_count - index - 1) / fade_count)
        envelope = min(attack, release)
        phase = 2.0 * math.pi * frequency_hz * index / SAMPLE_RATE
        samples.append(square_sample(phase, envelope))
    return samples


def render_clip(tones: list[tuple[float, int]]) -> bytes:
    samples = bytearray()
    for index, (frequency_hz, duration_ms) in enumerate(tones):
        samples.extend(render_tone(frequency_hz, duration_ms))
        if index + 1 < len(tones):
            samples.extend([CENTER] * round(SAMPLE_RATE * INTER_TONE_GAP_MS / 1000))
    samples.extend([CENTER] * round(SAMPLE_RATE * SILENCE_MS / 1000))
    return bytes(samples)


def write_wav(path: Path, samples: bytes) -> None:
    with wave.open(str(path), "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(1)
        wav.setframerate(SAMPLE_RATE)
        wav.writeframes(samples)


def main() -> None:
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)

    sounds = {
        # Short upward confirmation: recording has started.
        "voice-start.wav": [(659.25, 62), (1046.50, 92)],
        # Short downward closure: recording has ended and recognition can proceed.
        "voice-stop.wav": [(659.25, 58), (440.00, 88)],
        # Compact success chime: recognized text has been typed.
        "voice-complete.wav": [(880.00, 48), (1174.66, 78)],
        # Low warning chirp: recording, recognition, or typing failed.
        "voice-error.wav": [(246.94, 62), (196.00, 92)],
    }
    for name, tones in sounds.items():
        write_wav(OUTPUT_DIR / name, render_clip(tones))


if __name__ == "__main__":
    main()
