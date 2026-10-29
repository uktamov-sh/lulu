"""LuLu's sounds, synthesised from scratch (no samples): python scripts/gen-sounds.py

Writes the 29 WAVs the app plays into windows/sounds/ (16-bit mono, 44.1 kHz).
Each sound is a few soft tones with an envelope; tweak the table and run again.
"""
import math
import os
import random
import struct
import wave

RATE = 44100
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "sounds")
random.seed(7)


def tone(f0, f1, dur, vol=0.5, shape="sine", vib=0.0, trem=0.0, attack=0.005, release=None):
    """A glide from f0 to f1 Hz over dur seconds with a soft attack and an exponential tail."""
    n = int(RATE * dur)
    release = dur * 0.6 if release is None else release
    out = []
    phase = 0.0
    for i in range(n):
        t = i / RATE
        k = i / max(1, n - 1)
        f = f0 * (f1 / f0) ** k if f0 > 0 and f1 > 0 else f0
        if vib:
            f *= 1 + vib * math.sin(2 * math.pi * 6 * t)
        phase += 2 * math.pi * f / RATE
        if shape == "sine":
            s = math.sin(phase) + 0.18 * math.sin(2 * phase)
        elif shape == "tri":
            s = 2 / math.pi * math.asin(math.sin(phase))
        elif shape == "bell":
            s = math.sin(phase) + 0.35 * math.sin(2.76 * phase) * math.exp(-6 * t) + 0.2 * math.sin(5.4 * phase) * math.exp(-10 * t)
        else:
            s = math.sin(phase)
        env = min(1.0, t / attack) if attack > 0 else 1.0
        tail = dur - t
        if tail < release:
            env *= (tail / release) ** 2
        if trem:
            env *= 1 - trem * (0.5 + 0.5 * math.sin(2 * math.pi * 9 * t))
        out.append(s * env * vol)
    return out


def noise(dur, vol=0.3, lowpass=0.2):
    n = int(RATE * dur)
    out, y = [], 0.0
    for i in range(n):
        y += lowpass * (random.uniform(-1, 1) - y)
        env = (1 - i / n) ** 3
        out.append(y * env * vol * 3)
    return out


def silence(dur):
    return [0.0] * int(RATE * dur)


def seq(*parts):
    out = []
    for p in parts:
        out.extend(p)
    return out


def mix(*parts):
    n = max(len(p) for p in parts)
    return [sum(p[i] for p in parts if i < len(p)) for i in range(n)]


def notes(freqs, each, gap=0.0, vol=0.45, shape="bell"):
    parts = []
    for f in freqs:
        parts.append(tone(f, f, each, vol, shape))
        if gap:
            parts.append(silence(gap))
    return seq(*parts)


C5, D5, E5, F5, G5, A5, B5, C6, E6 = 523.25, 587.33, 659.25, 698.46, 783.99, 880.0, 987.77, 1046.5, 1318.5

SOUNDS = {
    "blip": lambda: tone(880, 1100, 0.07, 0.35),
    "tick": lambda: tone(2000, 1800, 0.025, 0.25, attack=0.001),
    "pop": lambda: tone(380, 950, 0.09, 0.45),
    "hover": lambda: tone(660, 700, 0.05, 0.18),
    "open": lambda: notes([C5, G5], 0.08, vol=0.35),
    "close": lambda: notes([G5, C5], 0.08, vol=0.35),
    "peek": lambda: notes([E5, G5, B5], 0.06, vol=0.3),
    "send": lambda: mix(tone(300, 1200, 0.2, 0.3, "tri"), noise(0.2, 0.12, 0.08)),
    "attach": lambda: seq(tone(1500, 1400, 0.02, 0.25, attack=0.001), silence(0.04), tone(1500, 1400, 0.02, 0.25, attack=0.001), tone(E5, E5, 0.12, 0.3, "bell")),
    "approve": lambda: notes([E5, A5], 0.12, vol=0.4),
    "approval": lambda: seq(tone(B5, B5, 0.16, 0.45, "bell"), silence(0.06), tone(B5, B5, 0.22, 0.45, "bell")),
    "question": lambda: tone(500, 820, 0.3, 0.38, "sine", vib=0.01),
    "error": lambda: seq(tone(330, 300, 0.14, 0.38, "tri"), tone(250, 210, 0.2, 0.38, "tri")),
    "finish": lambda: notes([C5, E5, G5, C6], 0.1, vol=0.38),
    "greet": lambda: seq(tone(D5, D5, 0.12, 0.38, "bell"), tone(A5, A5, 0.26, 0.38, "bell", vib=0.006)),
    "greeting": lambda: notes([C5, E5, G5, E5, A5], 0.12, vol=0.35),
    "work": lambda: tone(C5, C5, 0.12, 0.25, "bell"),
    "think": lambda: tone(392, 380, 0.32, 0.3, "sine", vib=0.015),
    "search": lambda: seq(tone(600, 900, 0.15, 0.3), tone(900, 600, 0.15, 0.3)),
    "rate": lambda: tone(440, 320, 0.42, 0.33, "tri", vib=0.02),
    "sleep": lambda: seq(tone(C5, C5, 0.25, 0.25, "sine"), tone(392, 392, 0.4, 0.22, "sine")),
    "yawn": lambda: seq(tone(400, 250, 0.4, 0.3, "sine", vib=0.02), tone(250, 300, 0.3, 0.25, "sine", vib=0.02)),
    "dizzy": lambda: tone(600, 500, 0.5, 0.33, "sine", vib=0.08),
    "annoyed": lambda: seq(tone(210, 200, 0.08, 0.35, "tri"), silence(0.03), tone(190, 180, 0.1, 0.35, "tri")),
    "slap": lambda: mix(noise(0.08, 0.35, 0.5), tone(140, 90, 0.09, 0.4)),
    "love": lambda: seq(tone(G5, G5, 0.16, 0.33, "sine", trem=0.3), tone(B5, B5, 0.26, 0.33, "sine", trem=0.3)),
    "proud": lambda: seq(mix(tone(C5, C5, 0.18, 0.25, "bell"), tone(E5, E5, 0.18, 0.2, "bell"), tone(G5, G5, 0.18, 0.2, "bell")), tone(C6, C6, 0.32, 0.38, "bell")),
    "wink": lambda: tone(880, 1320, 0.12, 0.33),
    "gulp": lambda: tone(300, 150, 0.15, 0.4),
}


def write(name, samples):
    peak = max(1e-9, max(abs(s) for s in samples))
    scale = min(1.0, 0.9 / peak)
    data = b"".join(struct.pack("<h", int(max(-1, min(1, s * scale)) * 32767)) for s in samples)
    with wave.open(os.path.join(OUT, f"{name}.wav"), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(data)


if __name__ == "__main__":
    os.makedirs(OUT, exist_ok=True)
    for name, make in SOUNDS.items():
        write(name, make())
    print(f"{len(SOUNDS)} sounds in {os.path.normpath(OUT)}")
