//! Анимация «Моей волны»: шейдер официального приложения, посчитанный на процессоре.
//!
//! У оригинала это web-worker с WebGL (`vibe-animation-worker` внутри app.asar): три шумовых
//! круга, искры по краю, цвета из обложки трека, отклик на звук и на лайк. Окно здесь рисует
//! Skia без видеокарты (см. main.rs), поэтому тот же фрагментный шейдер считается в своём
//! потоке и отдаётся интерфейсу готовой картинкой 25 раз в секунду — как тикер оригинала.
//!
//! Формулы, константы и порядок действий перенесены из воркера без изменений. Отличия только
//! в способе счёта: шум берётся с сетки в 4 пикселя (оригинал в режиме ShaderV3 — с половинной
//! текстуры), а искра, которая зависит лишь от угла, — из таблицы по углу.

use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Сторона холста: `desktopSizePx` оригинала. Браузер растягивает его на 250% ширины блока.
pub const SIZE: usize = 650;
/// `desktopScale`: во сколько раз сцена меньше холста.
const SCALE: f32 = 0.35;
/// Частота тикера воркера.
const FPS: u64 = 25;
/// `.VibeWidgetAnimation_root_visible { opacity: .8 }`.
const OPACITY: f32 = 0.8;
/// `transition: opacity 1s ease-in-out`.
const FADE_MS: f32 = 1000.0;
/// Шаг сетки шума в пикселях холста.
const GRID: usize = 4;
/// Отсчётов искры на оборот и долей между ними в таблице углов.
const LUT: usize = 2048;
const LUT_SUB: usize = 32;

const CIRCLE_WIDTH_BASE: f32 = 0.8;
const CIRCLE_WIDTH_STEP: f32 = 0.2;
const SPARK_STRENGTH_BASE: f32 = 1.0;
const SPARK_STRENGTH_STEP: f32 = 0.3;
const CIRCLE_RADIUS_BASE: f32 = 0.95;
const CIRCLE_RADIUS_STEP: f32 = 0.15;
const CIRCLE_OFFSET_BASE: f32 = 0.0;
const CIRCLE_OFFSET_STEP: f32 = 1.57;
/// Пи шейдера: в оригинале именно 3.1415, от этого зависит шов искр.
#[allow(clippy::approx_constant)]
const PI_GL: f32 = 3.1415;
/// `rotation` из состояния воркера: смещение шума каждого круга и скорость его вращения.
const ROTATION: [[f32; 3]; 3] = [[-0.3, 0.3, 0.2], [-0.3, -0.3, -0.2], [-0.3, -0.3, 0.2]];

// ---------- звук ----------

/// Размер окна анализатора: `analyser.fftSize = 32`.
const FFT: usize = 32;

fn clock_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Последние сэмплы, ушедшие в звуковую карту. Пишет аудиопоток, читает поток анимации.
pub struct Tap {
    ring: [AtomicU32; FFT],
    pos: AtomicUsize,
    stamp: AtomicU64,
    gain: AtomicU32,
    rate: AtomicU32,
}

impl Default for Tap {
    fn default() -> Self {
        Tap {
            ring: Default::default(),
            pos: AtomicUsize::new(0),
            stamp: AtomicU64::new(0),
            gain: AtomicU32::new(1.0f32.to_bits()),
            rate: AtomicU32::new(44100),
        }
    }
}

impl Tap {
    /// Громкость плеера: у оригинала анализатор стоит после неё.
    pub fn set_gain(&self, gain: f32) {
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    /// Средние уровни трёх полос, как `getAverageFrequencies` оригинала: 0–250, 500–2000 и
    /// 2000–4000 Гц. Ширину полосы оригинал считает как `sampleRate / frequencyBinCount`,
    /// то есть вдвое шире настоящей, — так и оставлено, иначе отклик был бы другим.
    pub fn bands(&self) -> [f64; 3] {
        let mut x = [0f32; FFT];
        // Звук не идёт дольше 150 мс — в кольце старые сэмплы, для анализатора это тишина.
        if clock_ms().saturating_sub(self.stamp.load(Ordering::Relaxed)) < 150 {
            let gain = f32::from_bits(self.gain.load(Ordering::Relaxed));
            let pos = self.pos.load(Ordering::Relaxed);
            for (i, v) in x.iter_mut().enumerate() {
                *v = f32::from_bits(self.ring[(pos + i) % FFT].load(Ordering::Relaxed)) * gain;
            }
        }
        let bytes = byte_frequency_data(&x);
        let rate = self.rate.load(Ordering::Relaxed).max(8000) as f64;
        let width = rate / (FFT / 2) as f64;
        let mut out = [0f64; 3];
        for (o, (low, high)) in out.iter_mut().zip([(0.0, 250.0), (500.0, 2000.0), (2000.0, 4000.0)]) {
            let (a, b) = ((low / width).floor() as usize, (high / width).floor() as usize);
            let sum: f64 = (a..=b).map(|i| bytes.get(i).copied().unwrap_or(0) as f64 / 256.0).sum();
            *o = sum / (b - a + 1) as f64;
        }
        out
    }
}

/// `AnalyserNode.getByteFrequencyData` при `smoothingTimeConstant = 0`: окно Блэкмана, БПФ,
/// модуль, делённый на размер окна, децибелы от −100 до −30 в байт.
fn byte_frequency_data(x: &[f32; FFT]) -> [u8; FFT / 2] {
    use std::f64::consts::PI;
    let mut w = [0f64; FFT];
    for (i, v) in w.iter_mut().enumerate() {
        let p = i as f64 / FFT as f64;
        let window = 0.42 - 0.5 * (2.0 * PI * p).cos() + 0.08 * (4.0 * PI * p).cos();
        *v = x[i] as f64 * window;
    }
    let mut out = [0u8; FFT / 2];
    for (k, o) in out.iter_mut().enumerate() {
        let (mut re, mut im) = (0f64, 0f64);
        for (n, v) in w.iter().enumerate() {
            let a = 2.0 * PI * (k * n) as f64 / FFT as f64;
            re += v * a.cos();
            im -= v * a.sin();
        }
        let mag = (re * re + im * im).sqrt() / FFT as f64;
        let db = if mag == 0.0 { -100.0 } else { 20.0 * mag.log10() };
        *o = (255.0 * (db + 100.0) / 70.0).clamp(0.0, 255.0) as u8;
    }
    out
}

/// Источник звука, который по пути складывает сэмплы в [`Tap`].
pub struct Tapped<S> {
    inner: S,
    tap: Arc<Tap>,
    channels: u16,
    at: u16,
    acc: f32,
    since_stamp: u32,
}

impl<S: rodio::Source<Item = f32>> Tapped<S> {
    pub fn new(inner: S, tap: Arc<Tap>) -> Self {
        let channels = inner.channels().max(1);
        tap.rate.store(inner.sample_rate(), Ordering::Relaxed);
        Tapped { inner, tap, channels, at: 0, acc: 0.0, since_stamp: u32::MAX }
    }
}

impl<S: rodio::Source<Item = f32>> Iterator for Tapped<S> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let x = self.inner.next()?;
        self.acc += x;
        self.at += 1;
        if self.at >= self.channels {
            // Анализатор сводит каналы в моно средним.
            let mono = self.acc / self.channels as f32;
            let pos = self.tap.pos.load(Ordering::Relaxed);
            self.tap.ring[pos % FFT].store(mono.to_bits(), Ordering::Relaxed);
            self.tap.pos.store((pos + 1) % FFT, Ordering::Relaxed);
            self.at = 0;
            self.acc = 0.0;
            self.since_stamp = self.since_stamp.saturating_add(1);
            if self.since_stamp >= 256 {
                self.since_stamp = 0;
                self.tap.stamp.store(clock_ms(), Ordering::Relaxed);
            }
        }
        Some(x)
    }
}

impl<S: rodio::Source<Item = f32>> rodio::Source for Tapped<S> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.inner.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        self.at = 0;
        self.acc = 0.0;
        self.inner.try_seek(pos)
    }
}

// ---------- состояние воркера ----------

/// Случайные числа воркера: начало времени и разброс оттенков. Точность не важна.
struct Rng(u64);

impl Rng {
    fn new() -> Rng {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
        Rng(nanos | 1)
    }

    fn unit(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    /// `Math.floor(Math.random() * (max - min + 1)) + min`.
    fn range(&mut self, min: f64, max: f64) -> f64 {
        (self.unit() * (max.floor() - min + 1.0)).floor() + min
    }
}

/// Плавное значение воркера: каждый кадр подходит к цели на долю, растущую со временем.
#[derive(Clone, Copy)]
struct Tween {
    cur: f64,
    target: f64,
    elapsed: f64,
    duration: f64,
}

impl Tween {
    fn new(v: f64, duration: f64) -> Tween {
        Tween { cur: v, target: v, elapsed: 0.0, duration }
    }

    fn update(&mut self, target: f64) {
        self.target = target;
        self.elapsed = 0.0;
    }

    fn next(&mut self, dt: f64) {
        let k = (self.elapsed / self.duration).clamp(0.0, 1.0);
        self.elapsed += dt;
        self.cur += (self.target - self.cur) * k;
    }
}

/// HSL в RGB, формула воркера. `h` в градусах, `s` и `l` от 0 до 1.
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> [f64; 3] {
    let f = |n: f64| {
        let k = (n + h / 30.0) % 12.0;
        let a = s * l.min(1.0 - l);
        l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    };
    [f(0.0), f(8.0), f(4.0)]
}

/// Цвет, все три канала которого идут к цели за 3 секунды.
#[derive(Clone, Copy)]
struct Tint([Tween; 3]);

impl Tint {
    fn new(hue: f64) -> Tint {
        Tint(hsl_to_rgb(hue, 1.0, 0.5).map(|c| Tween::new(c, 3000.0)))
    }

    fn update(&mut self, h: f64, s: f64, l: f64) {
        for (t, c) in self.0.iter_mut().zip(hsl_to_rgb(h, s, l)) {
            t.update(c);
        }
    }

    fn next(&mut self, dt: f64) {
        self.0.iter_mut().for_each(|t| t.next(dt));
    }

    fn value(&self) -> [f32; 3] {
        self.0.map(|t| t.cur as f32)
    }
}

/// Оттенок, насыщенность и светлота круга: `customColors` оригинала.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Hsl {
    pub h: f64,
    pub s: f64,
    pub l: f64,
}

/// Цвета трёх кругов: верхнего (малого), среднего и нижнего (большого).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Palette {
    pub top: Hsl,
    pub middle: Hsl,
    pub bottom: Hsl,
}

impl Palette {
    /// Цвета «Моей волны», пока трек не играет: жёлтый, розовый и пурпурный.
    pub const DEFAULT: Palette = Palette {
        top: Hsl { h: 50.0, s: 1.0, l: 0.5 },
        middle: Hsl { h: 330.0, s: 1.0, l: 0.5 },
        bottom: Hsl { h: 300.0, s: 1.0, l: 0.5 },
    };

    /// Цвета по основному цвету трека: чем меньше круг, тем он ярче и насыщеннее.
    pub fn from_rgb(c: [u8; 3]) -> Palette {
        let (h, s, l) = rgb_to_hsl(c);
        let hsl = |ds: f64, dl: f64| Hsl { h: h.clamp(0.0, 360.0), s: ((s + ds) / 100.0).clamp(0.0, 1.0), l: ((l + dl) / 100.0).clamp(0.0, 1.0) };
        Palette { top: hsl(15.0, 10.0), middle: hsl(15.0, 5.0), bottom: hsl(0.0, 0.0) }
    }
}

/// Оттенок целым числом градусов, насыщенность и светлота в процентах с одним знаком —
/// так считает оригинал.
fn rgb_to_hsl(c: [u8; 3]) -> (f64, f64, f64) {
    let [r, g, b] = c.map(|v| v as f64 / 255.0);
    let (min, max) = (r.min(g).min(b), r.max(g).max(b));
    let d = max - min;
    let l = (min + max) / 2.0;
    let mut h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d) % 6.0
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h = (h * 60.0).round();
    if h < 0.0 {
        h += 360.0;
    }
    let s = if d == 0.0 { 0.0 } else { d / (1.0 - (2.0 * l - 1.0).abs()) };
    let tenth = |v: f64| (v * 1000.0).round() / 10.0;
    (h, tenth(s), tenth(l))
}

/// Шесть цветов шейдера: начало и конец градиента каждого круга.
struct Colors {
    top: [Tint; 2],
    middle: [Tint; 2],
    bottom: [Tint; 2],
}

impl Colors {
    /// Начальные цвета воркера: `collectionHue` по умолчанию 10.
    fn new(rng: &mut Rng) -> Colors {
        let shifted = |hue: f64| (hue + 280.0) % 360.0;
        let wrap = |t: f64, e: f64| if (280.0..360.0).contains(&e) { t % 360.0 } else { t };
        let e = shifted(10.0);
        let i = wrap(e + rng.range(30.0, 40.0), e);
        Colors { top: [Tint::new(50.0), Tint::new(50.0)], middle: [Tint::new(300.0), Tint::new(320.0)], bottom: [Tint::new(e), Tint::new(i)] }
    }

    fn update(&mut self, p: Palette) {
        for (pair, c) in [(&mut self.top, p.top), (&mut self.middle, p.middle), (&mut self.bottom, p.bottom)] {
            pair.iter_mut().for_each(|t| t.update(c.h, c.s, c.l));
        }
    }

    fn next(&mut self, dt: f64) {
        for pair in [&mut self.top, &mut self.middle, &mut self.bottom] {
            pair.iter_mut().for_each(|t| t.next(dt));
        }
    }

    /// Порядок `vColor`: начала нижнего, среднего и верхнего круга, затем их концы.
    fn value(&self) -> [[f32; 3]; 6] {
        [self.bottom[0].value(), self.middle[0].value(), self.top[0].value(), self.bottom[1].value(), self.middle[1].value(), self.top[1].value()]
    }
}

/// Юниформы шейдера на один кадр.
#[derive(Clone, Copy, Debug)]
pub struct Uniforms {
    pub time: f32,
    pub colors: [[f32; 3]; 6],
    pub audio: [f32; 3],
    pub react: [f32; 3],
}

/// Состояние воркера между кадрами.
struct State {
    energy: Tween,
    time: f64,
    colors: Colors,
    audio: [f64; 3],
    audio_ratio: Tween,
    freq: Option<[f64; 3]>,
    react: [Tween; 3],
    /// Отложенные шаги анимации лайка: (когда, какой круг, к какому значению).
    timers: Vec<(Instant, usize, f64)>,
}

impl State {
    fn new(rng: &mut Rng) -> State {
        State {
            energy: Tween::new(0.2, 1000.0),
            time: (3600.0 * rng.unit()).floor(),
            colors: Colors::new(rng),
            audio: [0.0; 3],
            audio_ratio: Tween::new(0.0, 1000.0),
            freq: None,
            react: [Tween::new(0.0, 600.0); 3],
            timers: Vec::new(),
        }
    }

    /// `playAnimation({})`: энергия по умолчанию 0,6, в шейдер идёт `0.4 * (energy + 1)`.
    fn play(&mut self) {
        self.audio_ratio.update(1.0);
        self.energy.update(0.4 * (0.6 + 1.0));
    }

    fn idle(&mut self) {
        self.energy.update(0.2);
        self.audio_ratio.update(0.0);
    }

    /// Лайк: круги вспыхивают по очереди и гаснут через 0,85–1,05 секунды.
    fn like(&mut self, now: Instant) {
        self.react[0].update(0.7);
        for (ms, circle, value) in [(100, 1, 0.7), (150, 2, 0.7), (850, 0, 0.0), (950, 1, 0.0), (1050, 2, 0.0)] {
            self.timers.push((now + Duration::from_millis(ms), circle, value));
        }
    }

    fn update(&mut self, dt: f64, now: Instant) {
        let mut due = Vec::new();
        self.timers.retain(|&(at, circle, value)| {
            let fire = at <= now;
            if fire {
                due.push((circle, value));
            }
            !fire
        });
        for (circle, value) in due {
            self.react[circle].update(value);
        }
        self.energy.next(dt);
        self.colors.next(dt);
        self.react.iter_mut().for_each(|t| t.next(dt));
        self.time = (self.time + self.energy.cur * dt / 1000.0) % 86400.0;
        if let Some(freq) = self.freq {
            self.audio_ratio.next(dt);
            for (a, f) in self.audio.iter_mut().zip(freq) {
                *a = audio_param(*a, f) * self.audio_ratio.cur;
            }
        }
    }

    fn uniforms(&self) -> Uniforms {
        Uniforms { time: self.time as f32, colors: self.colors.value(), audio: self.audio.map(|v| v as f32), react: self.react.map(|t| t.cur as f32) }
    }
}

/// Уровень полосы растёт сразу, а спадает не быстрее 0,02 за кадр.
fn audio_param(prev: f64, new: f64) -> f64 {
    let (s, n) = (prev.clamp(0.0, 1.0), new.clamp(0.0, 1.0));
    if n > s {
        n
    } else if n < s {
        s - (s - n).min(0.02)
    } else {
        s
    }
}

// ---------- шейдер ----------

#[inline]
fn fract(x: f32) -> f32 {
    x - x.floor()
}

#[inline]
fn mod289(x: f32) -> f32 {
    x - 289.0 * (x / 289.0).floor()
}

#[inline]
fn permute(x: f32) -> f32 {
    mod289((x * 34.0 + 1.0) * x)
}

/// `smoothstep` GLSL. Края могут идти в обратном порядке — шейдер этим пользуется.
#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Симплекс-шум Ashima из шейдера.
fn snoise3(v: [f32; 3]) -> f32 {
    const CX: f32 = 0.166_666_7;
    const CY: f32 = 0.333_333_3;
    let s = (v[0] + v[1] + v[2]) * CY;
    let mut i = [(v[0] + s).floor(), (v[1] + s).floor(), (v[2] + s).floor()];
    let t = (i[0] + i[1] + i[2]) * CX;
    let x0 = [v[0] - i[0] + t, v[1] - i[1] + t, v[2] - i[2] + t];

    let step = |edge: f32, x: f32| if x < edge { 0.0f32 } else { 1.0 };
    let g = [step(x0[1], x0[0]), step(x0[2], x0[1]), step(x0[0], x0[2])];
    let l = [1.0 - g[0], 1.0 - g[1], 1.0 - g[2]];
    let i1 = [g[0].min(l[2]), g[1].min(l[0]), g[2].min(l[1])];
    let i2 = [g[0].max(l[2]), g[1].max(l[0]), g[2].max(l[1])];

    let x1 = [x0[0] - i1[0] + CX, x0[1] - i1[1] + CX, x0[2] - i1[2] + CX];
    let x2 = [x0[0] - i2[0] + 2.0 * CX, x0[1] - i2[1] + 2.0 * CX, x0[2] - i2[2] + 2.0 * CX];
    let x3 = [x0[0] - 1.0 + 3.0 * CX, x0[1] - 1.0 + 3.0 * CX, x0[2] - 1.0 + 3.0 * CX];

    i = [mod289(i[0]), mod289(i[1]), mod289(i[2])];
    let oz = [0.0, i1[2], i2[2], 1.0];
    let oy = [0.0, i1[1], i2[1], 1.0];
    let ox = [0.0, i1[0], i2[0], 1.0];

    const N: f32 = 0.142_857_15;
    let ns = [2.0 * N, 0.5 * N - 1.0, N];
    let corners = [x0, x1, x2, x3];
    let mut sum = 0.0;
    for k in 0..4 {
        let p = permute(permute(permute(i[2] + oz[k]) + i[1] + oy[k]) + i[0] + ox[k]);
        let j = p - 49.0 * (p * ns[2] * ns[2]).floor();
        let x_ = (j * ns[2]).floor();
        let y_ = (j - 7.0 * x_).floor();
        let x = x_ * ns[0] + ns[1];
        let y = y_ * ns[0] + ns[1];
        let h = 1.0 - x.abs() - y.abs();
        let sh = if h > 0.0 { 0.0 } else { -1.0 };
        let mut grad = [x + (x.floor() * 2.0 + 1.0) * sh, y + (y.floor() * 2.0 + 1.0) * sh, h];
        let norm = 1.792_842_9 - 0.853_734_73 * (grad[0] * grad[0] + grad[1] * grad[1] + grad[2] * grad[2]);
        grad = [grad[0] * norm, grad[1] * norm, grad[2] * norm];
        let c = corners[k];
        let m = (0.6 - (c[0] * c[0] + c[1] * c[1] + c[2] * c[2])).max(0.0);
        let m = m * m;
        sum += m * m * (grad[0] * c[0] + grad[1] * c[1] + grad[2] * c[2]);
    }
    42.0 * sum
}

#[inline]
fn tri(x: f32) -> f32 {
    (fract(x) - 0.5).abs()
}

fn tri_noise(mut p: [f32; 3], time: f32) -> f32 {
    let mut z = 0.4f32;
    let mut rz = 0.1f32;
    let mut bp = p;
    // `vTime * .1 * spd` при spd = 0.1.
    let shift = time * 0.1 * 0.1;
    for _ in 0..5 {
        let q = [bp[0] * 0.01, bp[1] * 0.01, bp[2] * 0.01];
        let dg = [tri(q[2] + tri(q[1] * 20.0)), tri(q[2] + tri(q[0])), tri(q[1] + tri(q[0]))];
        for a in 0..3 {
            p[a] += dg[a] + shift;
            bp[a] *= 4.0;
        }
        z *= 0.9;
        for v in &mut p {
            *v *= 1.6;
        }
        rz += tri(p[2] + tri(0.6 * p[0] + 0.1 * tri(p[1]))) / z;
    }
    smoothstep(0.0, 8.0, rz + (rz + z.sin() * 2.8).sin() * 2.2)
}

/// Искра в направлении `angle`: зависит только от угла и времени.
fn spark(angle: f32, time: f32) -> f32 {
    let idx = (angle / PI_GL) / 2.0;
    // Тот же шум, повёрнутый на 3.1415: у шва угла первый шум рвётся, второй нет.
    let (s, c) = PI_GL.sin_cos();
    let (x, y) = (angle.cos(), angle.sin());
    let pa1 = (x * s + y * c).atan2(x * c - y * s);
    let idx1 = (pa1 / PI_GL) / 2.0;
    let idx21 = (pa1 / PI_GL + 1.0) / 2.0 * PI_GL;
    let mut v = tri_noise([idx, 0.0, 0.0], time);
    let seam = smoothstep(0.9, 1.0, idx21.sin());
    if seam > 0.0 {
        v += (tri_noise([idx1, 0.0, idx1], time) - v) * seam;
    }
    v = v * 0.2 + v.powi(10);
    smoothstep(0.0, v.max(1e-5), 0.3) * v
}

/// Кривая времени CSS: `cubic-bezier(x1, y1, x2, y2)`.
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let at = |a: f32, b: f32, t: f32| ((1.0 - 3.0 * b + 3.0 * a) * t + (3.0 * b - 6.0 * a)) * t * t + 3.0 * a * t;
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    let mut t = x;
    for _ in 0..24 {
        let v = at(x1, x2, t);
        if (v - x).abs() < 1e-5 {
            break;
        }
        if v < x {
            lo = t;
        } else {
            hi = t;
        }
        t = (lo + hi) / 2.0;
    }
    at(y1, y2, t)
}

/// Таблицы, которые не меняются от кадра к кадру, и рабочие буферы.
pub struct Renderer {
    /// Координата сцены по центру пикселя: по x слева направо, по y снизу вверх.
    axis: Vec<f32>,
    /// Место пикселя в таблице искры: отсчёт × `LUT_SUB` + доля.
    angle: Vec<u16>,
    lut: Vec<f32>,
    grid_n: usize,
    /// Общий шум и шум трёх кругов в узлах сетки. NaN — узел в этом кадре ещё не считали.
    grids: [Vec<f32>; 4],
    /// Холст целиком и прямоугольник, закрашенный прошлым кадром: чистить нужно только его.
    canvas: Vec<Rgba8Pixel>,
    painted: [usize; 4],
}

/// Кадр: закрашенный прямоугольник холста (x, y, ширина, высота) и его пиксели.
pub struct Frame {
    pub rect: [usize; 4],
    pub pixels: SharedPixelBuffer<Rgba8Pixel>,
}

/// Постоянные одного круга на кадр.
#[derive(Clone, Copy, Default)]
struct Layer {
    radius: f32,
    width: f32,
    spark_k: f32,
    like: f32,
    react_k: f32,
    strength_base: f32,
    offset: f32,
    scale: f32,
    shift: [f32; 2],
    sin_t: f32,
    c1: [f32; 3],
    dc: [f32; 3],
}

impl Default for Renderer {
    fn default() -> Self {
        Renderer::new()
    }
}

impl Renderer {
    pub fn new() -> Renderer {
        let axis: Vec<f32> = (0..SIZE).map(|i| ((i as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0) / SCALE).collect();
        let mut angle = vec![0u16; SIZE * SIZE];
        let span = (LUT * LUT_SUB) as f64;
        for row in 0..SIZE {
            let y = axis[SIZE - 1 - row] as f64;
            for col in 0..SIZE {
                let a = y.atan2(axis[col] as f64);
                let pos = ((a + std::f64::consts::PI) / std::f64::consts::TAU * span).round();
                angle[row * SIZE + col] = (pos as usize % (LUT * LUT_SUB)) as u16;
            }
        }
        let grid_n = (SIZE - 1) / GRID + 2;
        Renderer {
            axis,
            angle,
            lut: vec![0.0; LUT + 1],
            grid_n,
            grids: std::array::from_fn(|_| vec![f32::NAN; grid_n * grid_n]),
            canvas: vec![Rgba8Pixel { r: 0, g: 0, b: 0, a: 0 }; SIZE * SIZE],
            painted: [0; 4],
        }
    }

    /// Нарисовать кадр и отдать только его закрашенную часть с прозрачной каймой в 2 пикселя:
    /// интерфейсу тогда перерисовывать не весь холст, а одно пятно.
    pub fn frame(&mut self, u: &Uniforms, circles: usize, opacity: f32, crop: [usize; 4]) -> Frame {
        let [x0, y0, x1, y1] = self.render(u, circles, opacity, crop);
        if x1 <= x0 || y1 <= y0 {
            return Frame { rect: [0, 0, 1, 1], pixels: SharedPixelBuffer::new(1, 1) };
        }
        let (x0, y0) = (x0.saturating_sub(2), y0.saturating_sub(2));
        let (x1, y1) = ((x1 + 2).min(SIZE), (y1 + 2).min(SIZE));
        let (w, h) = (x1 - x0, y1 - y0);
        let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(w as u32, h as u32);
        for (row, line) in pixels.make_mut_slice().chunks_exact_mut(w).enumerate() {
            let at = (y0 + row) * SIZE + x0;
            line.copy_from_slice(&self.canvas[at..at + w]);
        }
        Frame { rect: [x0, y0, w, h], pixels }
    }

    /// Холст последнего кадра целиком.
    #[cfg(test)]
    pub fn canvas(&self) -> &[Rgba8Pixel] {
        &self.canvas
    }

    /// Шум круга `which` (0 — общий) в точке пикселя, билинейно по узлам сетки.
    #[inline]
    fn noise(&mut self, which: usize, col: usize, row: usize, layer: &Layer, time: f32) -> f32 {
        let (gx, gy) = (col / GRID, row / GRID);
        let (tx, ty) = ((col % GRID) as f32 / GRID as f32, (row % GRID) as f32 / GRID as f32);
        let mut v = [0f32; 4];
        for (k, (dx, dy)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
            let at = (gy + dy) * self.grid_n + gx + dx;
            let mut n = self.grids[which][at];
            if n.is_nan() {
                // Узел стоит на центре своего пикселя; за краем холста координата продолжается.
                let node = |i: usize| (((i * GRID) as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0) / SCALE;
                let (ux, uy) = (node(gx + dx), -node(gy + dy));
                n = if which == 0 {
                    snoise3([ux * 1.2, uy * 1.2, time * 0.5])
                } else {
                    let (lx, ly) = (ux * layer.scale + layer.shift[0], uy * layer.scale + layer.shift[1]);
                    snoise3([lx * 1.2 + layer.offset, ly * 1.2 + layer.offset, time * 0.5 + layer.offset]) * 0.5 + 0.5
                };
                self.grids[which][at] = n;
            }
            v[k] = n;
        }
        let top = v[0] + (v[1] - v[0]) * tx;
        let bottom = v[2] + (v[3] - v[2]) * tx;
        top + (bottom - top) * ty
    }

    /// Нарисовать кадр на холсте (`SIZE`×`SIZE`, альфа уже умножена). Вне `crop`
    /// (x0, y0, x1, y1 в пикселях холста) и вне кругов пиксели остаются прозрачными.
    /// Возвращает закрашенный прямоугольник: x0, y0, x1, y1.
    pub fn render(&mut self, u: &Uniforms, circles: usize, opacity: f32, crop: [usize; 4]) -> [usize; 4] {
        let [px0, py0, px1, py1] = self.painted;
        for row in py0..py1 {
            self.canvas[row * SIZE + px0..row * SIZE + px1].fill(Rgba8Pixel { r: 0, g: 0, b: 0, a: 0 });
        }
        let mut painted = [SIZE, SIZE, 0, 0];
        let time = u.time;
        for (k, v) in self.lut.iter_mut().take(LUT).enumerate() {
            let a = -std::f32::consts::PI + std::f32::consts::TAU * k as f32 / LUT as f32;
            *v = spark(a, time);
        }
        self.lut[LUT] = self.lut[0];
        for g in &mut self.grids {
            g.fill(f32::NAN);
        }

        let mut layers = [Layer::default(); 3];
        for (i, l) in layers.iter_mut().enumerate() {
            let fi = i as f32;
            let like = u.react[i];
            let audio = u.audio[i];
            let offset = CIRCLE_OFFSET_BASE + CIRCLE_OFFSET_STEP * fi;
            let (s, c) = (time * ROTATION[i][2]).sin_cos();
            let strength = like.max(audio);
            *l = Layer {
                radius: CIRCLE_RADIUS_BASE - CIRCLE_RADIUS_STEP * fi,
                width: CIRCLE_WIDTH_BASE - CIRCLE_WIDTH_STEP * fi,
                spark_k: SPARK_STRENGTH_BASE - SPARK_STRENGTH_STEP * fi,
                like,
                react_k: like.max(audio * 0.6) * 50.0,
                strength_base: 0.15 * (1.0 + 1.5 * (-(time * 2.0 + offset * 0.5).sin() * 0.5)) + 0.3 * strength,
                offset,
                scale: 1.0 - like * 0.5,
                shift: [ROTATION[i][0] * c - ROTATION[i][1] * s, ROTATION[i][0] * s + ROTATION[i][1] * c],
                sin_t: (time + offset).sin(),
                c1: u.colors[i],
                dc: [u.colors[i + 3][0] - u.colors[i][0], u.colors[i + 3][1] - u.colors[i][1], u.colors[i + 3][2] - u.colors[i][2]],
            };
        }
        let circles = circles.min(3);
        let [x0, y0, x1, y1] = [crop[0].min(SIZE), crop[1].min(SIZE), crop[2].min(SIZE), crop[3].min(SIZE)];

        for row in y0..y1 {
            let uy = self.axis[SIZE - 1 - row];
            for col in x0..x1 {
                let ux = self.axis[col];
                let len = (ux * ux + uy * uy).sqrt();
                let pos = self.angle[row * SIZE + col] as usize;
                let (k, f) = (pos / LUT_SUB, (pos % LUT_SUB) as f32 / LUT_SUB as f32);
                let spark = self.lut[k] + (self.lut[k + 1] - self.lut[k]) * f;

                // Край круга без общего шума: шум добавит не больше 0,3.
                let mut edge = [0f32; 3];
                let mut reach = 0f32;
                for i in 0..circles {
                    let l = &layers[i];
                    let base = l.spark_k * spark;
                    edge[i] = l.radius + l.width * 0.5 + base * (1.0 + l.react_k * base);
                    reach = reach.max(edge[i]);
                }
                if len >= reach + 0.3 {
                    continue;
                }
                let n0 = self.noise(0, col, row, &layers[0], time);

                let mut rgb = [0f32; 3];
                let mut alpha = 0f32;
                for i in 0..circles {
                    let l = layers[i];
                    let outer = edge[i] + 0.3 * n0;
                    if len >= outer {
                        continue;
                    }
                    let (lx, ly) = (ux * l.scale + l.shift[0], uy * l.scale + l.shift[1]);
                    let llen = (lx * lx + ly * ly).sqrt();
                    let r0 = self.noise(i + 1, col, row, &l, time);
                    let v0 = smoothstep(r0 + 0.1 + (l.sin_t + 1.0), r0, llen);
                    let a = v0 * smoothstep(outer, 0.5, len);
                    if a <= 0.0 {
                        continue;
                    }
                    let d0 = (llen - r0).abs();
                    let glow = l.strength_base / (1.0 + d0 + d0 * 10.0);
                    let flash = 0.6 * l.like * (1.0 - smoothstep(0.2, outer * 0.8, len));
                    let m = ly * 2.0;
                    for ch in 0..3 {
                        let col = (l.c1[ch] + l.dc[ch] * m + glow).clamp(0.0, 1.0) + flash;
                        rgb[ch] += (col - rgb[ch]) * a;
                    }
                    alpha = alpha.max(a);
                }
                if alpha <= 0.0 {
                    continue;
                }
                // Холст оригинала без premultipliedAlpha: браузер сам умножает цвет на альфу.
                let a = alpha.min(1.0) * opacity;
                let px = |c: f32| (c.clamp(0.0, 1.0) * a * 255.0 + 0.5) as u8;
                let alpha8 = (a * 255.0 + 0.5) as u8;
                if alpha8 == 0 {
                    continue;
                }
                self.canvas[row * SIZE + col] = Rgba8Pixel { r: px(rgb[0]), g: px(rgb[1]), b: px(rgb[2]), a: alpha8 };
                painted = [painted[0].min(col), painted[1].min(row), painted[2].max(col + 1), painted[3].max(row + 1)];
            }
        }
        self.painted = if painted[2] > painted[0] { painted } else { [0; 4] };
        self.painted
    }
}

// ---------- поток анимации ----------

pub enum Msg {
    /// Страница волны показана или скрыта. При показе пятно проявляется заново за секунду.
    Mount(bool),
    /// Окно видно или свёрнуто: `enable` и `disable` оригинала.
    Visible(bool),
    Playing(bool),
    Like,
    Palette(Palette),
    /// Видимая часть холста в его пикселях: x0, y0, x1, y1.
    Crop([usize; 4]),
}

/// Что поток анимации сообщает интерфейсу.
pub enum Event {
    Frame(Frame),
    /// Кадры не успевают даже с двумя кругами: дальше показывать неподвижную картинку.
    Fallback,
}

#[derive(Clone)]
pub struct Vibe {
    tx: Sender<Msg>,
    /// Интерфейс ещё не забрал прошлый кадр: новый рисовать незачем.
    busy: Arc<AtomicBool>,
}

impl Vibe {
    pub fn spawn(tap: Arc<Tap>, emit: impl Fn(Event) + Send + 'static) -> Vibe {
        let (tx, rx) = channel();
        let busy = Arc::new(AtomicBool::new(false));
        let flag = busy.clone();
        std::thread::Builder::new().name("vibe".into()).spawn(move || run(rx, tap, flag, emit)).expect("vibe thread");
        Vibe { tx, busy }
    }

    pub fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
    }

    /// Интерфейс показал кадр.
    pub fn frame_shown(&self) {
        self.busy.store(false, Ordering::Release);
    }
}

fn run(rx: Receiver<Msg>, tap: Arc<Tap>, busy: Arc<AtomicBool>, emit: impl Fn(Event)) {
    let mut rng = Rng::new();
    let mut state = State::new(&mut rng);
    let mut renderer: Option<Renderer> = None;
    let (mut mounted, mut visible, mut playing) = (false, true, false);
    let mut crop = [0, 0, SIZE, SIZE];
    let mut circles = 3;
    let mut fallback = false;
    let frame = Duration::from_millis(1000 / FPS);
    let mut last = Instant::now();
    let mut next = last;
    let mut shown_at = last;
    // Замер скорости, как у оригинала: среднее за 10 секунд, три плохих замера подряд.
    let (mut spent, mut frames, mut slow) = (Duration::ZERO, 0u32, 0u32);

    loop {
        let active = mounted && visible && !fallback;
        let msg = if active {
            match rx.recv_timeout(next.saturating_duration_since(Instant::now())) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return,
            }
        };
        if let Some(msg) = msg {
            let now = Instant::now();
            match msg {
                Msg::Mount(on) => {
                    if on && !mounted {
                        shown_at = now;
                        last = now;
                        next = now;
                        fallback = false;
                    }
                    if !on {
                        renderer = None;
                    }
                    mounted = on;
                }
                Msg::Visible(on) => {
                    if on && !visible {
                        last = now;
                        next = now;
                    }
                    visible = on;
                }
                Msg::Playing(on) => {
                    playing = on;
                    if on {
                        state.play();
                    } else {
                        state.idle();
                    }
                }
                Msg::Like => state.like(now),
                Msg::Palette(p) => state.colors.update(p),
                Msg::Crop(c) => crop = c,
            }
            continue;
        }

        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f64() * 1000.0;
        last = now;
        next += frame;
        if next < now {
            next = now + frame;
        }
        if playing {
            state.freq = Some(tap.bands());
        }
        state.update(dt, now);
        if busy.swap(true, Ordering::AcqRel) {
            continue;
        }
        let fade = (now.duration_since(shown_at).as_secs_f32() * 1000.0 / FADE_MS).min(1.0);
        let opacity = OPACITY * cubic_bezier(0.42, 0.0, 0.58, 1.0, fade);
        let frame = renderer.get_or_insert_with(Renderer::new).frame(&state.uniforms(), circles, opacity, crop);
        emit(Event::Frame(frame));

        spent += now.elapsed();
        frames += 1;
        if frames >= 250 {
            let per_frame = spent / frames;
            (spent, frames) = (Duration::ZERO, 0);
            // Меньше 20 кадров в секунду — кадр считается дольше 50 мс.
            if per_frame > Duration::from_millis(50) {
                slow += 1;
            } else {
                slow = 0;
            }
            if slow >= 3 {
                slow = 0;
                if circles == 3 {
                    circles = 2;
                } else {
                    fallback = true;
                    renderer = None;
                    emit(Event::Fallback);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_palette_matches_original() {
        // Жёлтый hsl(50, 100%, 50%) — #ffd500.
        let [r, g, b] = hsl_to_rgb(50.0, 1.0, 0.5);
        assert!((r - 1.0).abs() < 1e-9 && (g - 0.8333).abs() < 1e-3 && b.abs() < 1e-9);
        // #7C35C0 (plum) — hsl(271, 56.7%, 48%).
        assert_eq!(rgb_to_hsl([0x7C, 0x35, 0xC0]), (271.0, 56.7, 48.0));
    }

    #[test]
    fn noise_stays_in_range() {
        let mut max = 0f32;
        for i in 0..4000 {
            let f = i as f32 * 0.137;
            let v = snoise3([f.sin() * 3.0, f.cos() * 3.0, f * 0.31]);
            assert!(v.is_finite());
            max = max.max(v.abs());
        }
        assert!(max > 0.5 && max <= 1.01, "{max}");
    }

    #[test]
    fn silence_and_full_scale() {
        assert_eq!(byte_frequency_data(&[0.0; FFT]), [0; FFT / 2]);
        // Постоянный уровень 1: нулевая полоса на пределе шкалы.
        assert_eq!(byte_frequency_data(&[1.0; FFT])[0], 255);
    }

    #[test]
    fn bezier_ends() {
        assert!(cubic_bezier(0.42, 0.0, 0.58, 1.0, 0.0).abs() < 1e-4);
        assert!((cubic_bezier(0.42, 0.0, 0.58, 1.0, 1.0) - 1.0).abs() < 1e-4);
        assert!((cubic_bezier(0.42, 0.0, 0.58, 1.0, 0.5) - 0.5).abs() < 1e-3);
    }

    /// Кадр для сверки с шейдером оригинала и замер скорости:
    /// `VIBE_DUMP=out.png cargo test --release vibe::tests::dump -- --nocapture`.
    #[test]
    fn dump() {
        let Ok(path) = std::env::var("VIBE_DUMP") else { return };
        let get = |name: &str, default: f32| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
        let palette = Palette::from_rgb([0x7C, 0x35, 0xC0]);
        let tint = |c: Hsl| hsl_to_rgb(c.h, c.s, c.l).map(|v| v as f32);
        let (top, middle, bottom) = (tint(palette.top), tint(palette.middle), tint(palette.bottom));
        let u = Uniforms {
            time: get("VIBE_TIME", 1234.5),
            colors: [bottom, middle, top, bottom, middle, top],
            audio: [get("VIBE_AUDIO", 0.0); 3],
            react: [get("VIBE_REACT", 0.0); 3],
        };
        let mut r = Renderer::new();
        let started = Instant::now();
        let rounds = 20;
        let mut rect = [0; 4];
        for _ in 0..rounds {
            rect = r.frame(&u, 3, 1.0, [0, 0, SIZE, SIZE]).rect;
        }
        println!("кадр: {:?}, пятно {rect:?}", started.elapsed() / rounds);
        // Как в браузере: поверх фона страницы #121212.
        let mut img = image::RgbImage::new(SIZE as u32, SIZE as u32);
        for (p, o) in r.canvas().iter().zip(img.pixels_mut()) {
            let over = |c: u8| (c as f32 + 18.0 * (1.0 - p.a as f32 / 255.0) + 0.5) as u8;
            *o = image::Rgb([over(p.r), over(p.g), over(p.b)]);
        }
        img.save(path).unwrap();
    }
}
