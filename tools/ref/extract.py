"""Достаёт из установленной Яндекс Музыки воркер анимации «Моей волны» и его шейдер.

    python tools/ref/extract.py [путь к app.asar] [папка для результата]

Пишет worker.js (код воркера как есть) — его читают trace.js и dumpfrag.js. Сам код оригинала
в репозиторий не кладётся: он достаётся на месте из приложения, которое стоит у вас.
"""
import base64
import json
import os
import re
import struct
import sys

DEFAULT_ASAR = os.path.join(os.environ.get("LOCALAPPDATA", ""), "Programs", "YandexMusic", "resources", "app.asar")


def read_asar(path):
    """Файлы архива: (имя, содержимое) для всего, что лежит в app/_next/static/chunks."""
    with open(path, "rb") as f:
        _, header_size, _, json_size = struct.unpack("<4I", f.read(16))
        header = json.loads(f.read(json_size))
        base = 8 + header_size

        def walk(node, prefix):
            for name, entry in node.get("files", {}).items():
                full = prefix + name
                if "files" in entry:
                    yield from walk(entry, full + "/")
                elif "offset" in entry:
                    yield full, int(entry["offset"]), entry["size"]

        for name, offset, size in walk(header, ""):
            if name.startswith("app/_next/static/chunks/") and name.endswith(".js"):
                f.seek(base + offset)
                yield name, f.read(size).decode("utf-8", "replace")


def main():
    asar = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_ASAR
    out = sys.argv[2] if len(sys.argv) > 2 else os.path.join("target", "ref")
    os.makedirs(out, exist_ok=True)
    for name, text in read_asar(asar):
        # Воркеры зашиты в чанки строкой base64 (rollup-plugin-web-worker-loader).
        for blob in re.findall(r'"([A-Za-z0-9+/=]{2000,})"', text):
            try:
                code = base64.b64decode(blob).decode("utf-8")
            except (ValueError, UnicodeDecodeError):
                continue
            if "vibe-animation-worker-init" in code and "vScreenSize" in code:
                target = os.path.join(out, "worker.js")
                # Без перевода концов строк: файл должен совпадать с воркером байт в байт.
                with open(target, "w", encoding="utf-8", newline="") as f:
                    f.write(code)
                print(f"{name}: воркер анимации, {len(code)} символов -> {target}")
                return
    sys.exit("воркер анимации в app.asar не найден: приложение обновилось и устроено иначе?")


if __name__ == "__main__":
    main()
