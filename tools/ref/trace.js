// Запускает настоящий воркер анимации оригинала (worker0.js из app.asar) с подставным WebGL
// и виртуальными часами и пишет юниформы каждого кадра. Сценарий — как у страницы волны.
const fs = require('fs');
const src = fs.readFileSync(process.argv[2], 'utf8');
const out = process.argv[3];

let now = 0;
let rafQueue = [];
let timers = [];
const listeners = [];
const frames = [];
let current = {};

const UNIFORMS = [
  ['vScreenSize', 35664, 1], ['vTime', 5126, 1], ['vScale', 5126, 1], ['vColorBackground', 35666, 1],
  ['vColor[0]', 35665, 6], ['vRotation[0]', 35665, 3], ['vAudio[0]', 5126, 3], ['vReact[0]', 5126, 3],
  ['vInteractionPoint', 35664, 1], ['vInteraction', 5126, 1],
];
const special = { LINK_STATUS: 1001, ACTIVE_UNIFORMS: 1002, ACTIVE_ATTRIBUTES: 1003, COMPILE_STATUS: 1004 };
const setU = (loc, v) => { current[loc.name] = v && v.length !== undefined ? Array.from(v) : v; };
const impl = {
  getParameter: () => 16,
  getExtension: () => null,
  getShaderInfoLog: () => '',
  getProgramInfoLog: () => '',
  getShaderParameter: () => true,
  getProgramParameter: (p, what) => what === special.LINK_STATUS ? true : what === special.ACTIVE_UNIFORMS ? UNIFORMS.length : what === special.ACTIVE_ATTRIBUTES ? 1 : 0,
  getActiveUniform: (p, i) => ({ name: UNIFORMS[i][0], type: UNIFORMS[i][1], size: UNIFORMS[i][2] }),
  getUniformLocation: (p, name) => ({ name }),
  getActiveAttrib: () => ({ name: 'position', type: 35666, size: 1 }),
  getAttribLocation: () => 0,
  uniform1f: setU, uniform1fv: setU, uniform2fv: setU, uniform3fv: setU, uniform4fv: setU, uniform1i: setU, uniform1iv: setU,
  drawElements: () => frames.push({ t: now, u: JSON.parse(JSON.stringify(current)) }),
  drawArrays: () => frames.push({ t: now, u: JSON.parse(JSON.stringify(current)) }),
};
const canvas = { width: 0, height: 0, style: {} };
const gl = new Proxy({ canvas }, {
  get(target, prop) {
    if (prop in target) return target[prop];
    if (prop in impl) return impl[prop];
    if (prop in special) return special[prop];
    if (typeof prop === 'string' && /^[A-Z0-9_]+$/.test(prop)) {
      let h = 7; for (const c of prop) h = (h * 31 + c.charCodeAt(0)) % 100000;
      return 200000 + h;
    }
    return (target[prop] = () => ({}));
  },
  set(target, prop, value) { target[prop] = value; return true; },
});
canvas.getContext = () => gl;

const self = {
  addEventListener: (type, fn) => { if (type === 'message') listeners.push(fn); },
  postMessage: (m) => console.log('worker says', JSON.stringify(m)),
};
const sandbox = {
  self, console,
  performance: { now: () => now },
  requestAnimationFrame: (fn) => { rafQueue.push(fn); return rafQueue.length; },
  cancelAnimationFrame: () => { rafQueue = []; },
  setTimeout: (fn, ms) => { timers.push({ at: now + ms, fn }); return timers.length; },
  clearTimeout: () => {},
  Math: Object.assign(Object.create(Math), { random: () => 0.5 }),
};
new Function(...Object.keys(sandbox), src)(...Object.values(sandbox));

const T = (name) => 'vibe-animation-worker-' + name;
const send = (type, payload) => listeners.forEach((fn) => fn({ data: { source: 'vibe', type, payload } }));

// Виртуальное время: кадры монитора 60 Гц; таймеры срабатывают перед кадром.
function run(ms) {
  const end = now + ms;
  while (now + 1000 / 60 <= end + 1e-9) {
    now += 1000 / 60;
    const due = timers.filter((t) => t.at <= now); timers = timers.filter((t) => t.at > now);
    due.forEach((t) => t.fn());
    const q = rafQueue; rafQueue = [];
    q.forEach((fn) => fn(now));
  }
}

const hsl = (h, s, l) => ({ h, s: s / 100, l: l / 100 });
const DEFAULT = { top: hsl(50, 100, 50), middle: hsl(330, 100, 50), bottom: hsl(300, 100, 50) };
// #7C35C0 → hsl(271, 56.7%, 48%)
const TRACK = { top: hsl(271, 56.7 + 15, 48 + 10), middle: hsl(271, 56.7 + 15, 48 + 5), bottom: hsl(271, 56.7, 48) };

const steps = [];
const step = (name, fn) => { steps.push({ at: now, name }); fn(); };

step('init', () => send(T('init'), { canvas, state: 'DEFAULT', isShaderV3Enabled: false, collectionHue: undefined, shaderOptions: { transparent: true } }));
step('palette default', () => send(T('apply-settings'), { customColors: DEFAULT }));
step('idle', () => send(T('idle-animation')));
step('enable', () => send(T('enable')));
step('layout', () => send(T('update-layout'), { isMobile: false }));
run(2000);
step('palette track', () => send(T('apply-settings'), { customColors: TRACK }));
step('play', () => send(T('play-animation'), {}));
// Звук приходит 25 раз в секунду, как от тикера страницы.
for (let i = 0; i < 100; i++) {
  const v = 0.5 + 0.5 * Math.sin(i * 0.37);
  send(T('audio-analyzer-frequencies'), { low: v, middle: v * 0.8, high: v * 0.5 });
  run(40);
  if (i === 40) step('like', () => send(T('like-animation')));
}
step('idle', () => send(T('idle-animation')));
step('palette default', () => send(T('apply-settings'), { customColors: DEFAULT }));
run(3000);

fs.writeFileSync(out, JSON.stringify({ steps, frames }));
console.log('frames', frames.length, 'last', JSON.stringify(frames[frames.length - 1]));
console.log('first', JSON.stringify(frames[0]));
