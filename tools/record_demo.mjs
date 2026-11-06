// Record the page's ?demo loop frame by frame in headless Chromium, for the README GIF.
//   node tools/record_demo.mjs URL OUTDIR [frames] [stepsPerFrame] [size]
// URL is the page with ?demo (e.g. http://127.0.0.1:8799/?demo=20). Each frame advances
// the demo by stepsPerFrame fixed 1/30 s steps, then takes a screenshot (OUTDIR/f0000.png...).
// PV_SETTINGS='{"pluto.ice":"0.8","pluto.flow":"2"}' presets the page's stored settings.
// Then: ffmpeg -framerate 15 -i OUTDIR/f%04d.png ... (see README "Rebuilding the images").
import { spawn } from 'node:child_process';
import { mkdirSync, writeFileSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const [url, out, nFrames = '300', perFrame = '2', size = '540'] = process.argv.slice(2);
if (!url || !out) { console.error('usage: record_demo.mjs URL OUTDIR [frames] [stepsPerFrame] [size]'); process.exit(2); }
mkdirSync(out, { recursive: true });
const browser = process.env.CHROME || 'chromium';
const port = 9333;
const proc = spawn(browser, [
  '--headless=new', `--remote-debugging-port=${port}`, `--window-size=${size},${size}`,
  `--user-data-dir=${mkdtempSync(join(tmpdir(), 'pv-rec-'))}`, '--ignore-gpu-blocklist',
  '--enable-unsafe-swiftshader', '--hide-scrollbars', 'about:blank',
], { stdio: 'ignore' });
const sleep = ms => new Promise(r => setTimeout(r, ms));

let target;
for (let i = 0; i < 50 && !target; i++) {
  await sleep(200);
  try { target = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find(t => t.type === 'page'); } catch {}
}
if (!target) { console.error('no browser'); proc.kill(); process.exit(1); }
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise(r => ws.addEventListener('open', r, { once: true }));
let id = 0; const pending = new Map();
ws.addEventListener('message', e => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
});
const cdp = (method, params = {}) => new Promise(r => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const ev = async expr => (await cdp('Runtime.evaluate', { expression: expr, returnByValue: true })).result?.result?.value;

await cdp('Page.navigate', { url });
if (process.env.PV_SETTINGS) {
  await sleep(500);
  for (const [k, v] of Object.entries(JSON.parse(process.env.PV_SETTINGS)))
    await ev(`localStorage.setItem(${JSON.stringify(k)}, ${JSON.stringify(v)})`);
  await cdp('Page.navigate', { url });
}
for (let i = 0; i < 300; i++) {
  await sleep(200);
  if (await ev("typeof ready !== 'undefined' && ready && !!window.demoStep")) break;
}
console.error('renderer:', await ev("(() => { const g = document.querySelector('canvas').getContext('webgl2'); const x = g.getExtension('WEBGL_debug_renderer_info'); return x ? g.getParameter(x.UNMASKED_RENDERER_WEBGL) : '?'; })()"));
const t0 = Date.now();
for (let f = 0; f < +nFrames; f++) {
  const vt = await ev(`demoStep(${+perFrame})`);
  const shot = await cdp('Page.captureScreenshot', { format: 'png' });
  writeFileSync(join(out, `f${String(f).padStart(4, '0')}.png`), Buffer.from(shot.result.data, 'base64'));
  if (f % 30 === 0) console.error(`frame ${f} t=${vt?.toFixed(1)} s  ${((Date.now() - t0) / 1000).toFixed(0)} s`);
}
ws.close(); proc.kill();
