"""Сверяет формулу уровней звука из src/vibe.rs с ответом AnalyserNode настоящего Chromium.

    python tools/ref/audio_cmp.py audio_ref.json

audio_ref.json — содержимое <pre id="d"> страницы tools/ref/audio.html (см. README.md).
"""
import cmath
import json
import math
import sys

N = 32


def byte_frequency_data(x):
    """То же, что `byte_frequency_data` в src/vibe.rs: окно Блэкмана, БПФ, децибелы в байт."""
    w = [x[i] * (0.42 - 0.5 * math.cos(2 * math.pi * i / N) + 0.08 * math.cos(4 * math.pi * i / N)) for i in range(N)]
    out = []
    for k in range(N // 2):
        mag = abs(sum(w[n] * cmath.exp(-2j * math.pi * k * n / N) for n in range(N))) / N
        db = -100.0 if mag == 0 else 20 * math.log10(mag)
        out.append(int(max(0, min(255, 255 * (db + 100) / 70))))
    return out


worst = 0
for case in json.loads(open(sys.argv[1], encoding="utf-8-sig").read()):
    ours = byte_frequency_data(case["mono"])
    diff = max(abs(a - b) for a, b in zip(case["bytes"], ours))
    worst = max(worst, diff)
    print("chromium", case["bytes"][:8], "| порт", ours[:8], "| расхождение", diff)
sys.exit(1 if worst else 0)
