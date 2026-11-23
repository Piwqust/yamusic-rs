# Сверка анимации «Моей волны» с оригиналом

Пятно на главной — порт шейдера и воркера официального приложения (`src/vibe.rs`). Здесь лежит
всё, чем порт сверялся с оригиналом. Код оригинала в репозиторий не кладётся: `extract.py`
достаёт его из установленной Яндекс Музыки в `target/ref/`.

Нужны Python 3 (для картинок — `pip install moderngl numpy pillow`) и Node.js.

## 1. Достать воркер и шейдер

```
python tools/ref/extract.py                  # target/ref/worker.js из %LOCALAPPDATA%\Programs\YandexMusic
node tools/ref/dumpfrag.js target/ref/worker.js target/ref/original.frag
```

## 2. Логика: время, цвета, звук, лайк

`trace.js` запускает настоящий воркер в Node с подставным WebGL и виртуальными часами
(кадры монитора 60 Гц, `Math.random() = 0.5`) и пишет юниформы каждого кадра. Тот же сценарий
проигрывает тест `vibe::tests::trace`.

```
node tools/ref/trace.js target/ref/worker.js target/ref/trace.json
VIBE_TRACE=target/ref/trace.json cargo test --release vibe::tests::trace -- --nocapture
```

Ожидается «наибольшее расхождение: время 0e0, цвета 0e0, звук 0e0, лайк 0e0». Контрольные
кадры из этой записи зашиты в тест `state_matches_original_worker`; если приложение обновилось
и запись изменилась, `VIBE_TRACE_PRINT=20` печатает новые строки для таблицы `ORIGINAL`.

## 3. Картинка: шейдер попиксельно

`ref.py` рисует оригинальный GLSL на настоящем OpenGL вне экрана, тест `dump` — тот же кадр
портом. Параметры: время, уровень звука, сила вспышки лайка.

```
python tools/ref/ref.py target/ref/original.frag target/ref/gl.png 3141.7 0.4 0.7
VIBE_DUMP=target/ref/rust.png VIBE_TIME=3141.7 VIBE_AUDIO=0.4 VIBE_REACT=0.7 cargo test --release vibe::tests::dump -- --nocapture
python tools/ref/cmp.py target/ref/gl.png target/ref/rust.png target/ref/diff.png
```

Ожидается расхождение не больше 4 из 255 на канал (сетка шума 4 px вместо расчёта в каждом
пикселе). `diff.png` — оригинал, порт и разница ×8 рядом.

## 4. Уровни звука

`audio.html` спрашивает `AnalyserNode` Chromium (окно 32, как у оригинала) на шести сигналах.
Страницу открывает Edge без окна, ответ сверяет `audio_cmp.py`:

```
msedge --headless=new --disable-gpu --virtual-time-budget=8000 --dump-dom file:///…/tools/ref/audio.html > dom.txt
# содержимое <pre id="d">…</pre> из dom.txt сохранить как audio_ref.json
python tools/ref/audio_cmp.py audio_ref.json
```
