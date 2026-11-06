# pluto-viz — agent notes

Pluto tholin-snow music visualiser. User-facing description, keys and running: `README.md`.

## Layout
- `index.html` — the whole page: WebGL2, two shaders (`SNOW` simulation step, `SHOW` display), keys, OSD.
- `bridge/` — Rust (edition 2024) binary `pluto-viz`: audio capture, band analysis, HTTP + WebSocket.
  Embeds `index.html`, `pluto.jpg`, `pluto_sim.png`, `pluto_view.json` at build time.
- `tools/` — `fetch_data.sh` (source data → `data/`, git-ignored), `fit_photo.py` (photo geometry),
  `build_view.py` (the three embedded images/JSON).
- `start-kiosk.sh` — JACK-connected kiosk launcher (Brave app window, own profile).

## Build / check
```
cargo build --release --manifest-path bridge/Cargo.toml
cargo clippy --manifest-path bridge/Cargo.toml --release -- -D warnings
cargo fmt --manifest-path bridge/Cargo.toml --check
cargo clippy --manifest-path bridge/Cargo.toml --target x86_64-pc-windows-gnullvm -- -D warnings  # Windows check from Linux
```
CI (`.github/workflows/ci.yml`): fmt, clippy, build, `--help` on Linux/Windows/macOS every push.
Release (`release.yml`): a `v*` tag builds Linux x86_64, Windows x86_64, macOS arm64/x86_64 archives.
Linux build deps: `libpipewire-0.3-dev libjack-jackd2-dev libasound2-dev libclang-dev`.

## Bridge
- Capture: Linux PipeWire via cpal (`pipewire` feature; default = the default sink's monitor) or JACK
  (`--jack`, implied by `--name`/`--connect`; libjack is dlopened). Windows WASAPI loopback of the default
  output, macOS default input. All backends push mono samples into an `rtrb` ring from the audio callback:
  no allocation, locks or syscalls there.
- Analysis at 60 Hz: RMS over the last 2048 samples, plus bass 25–160 / mid 160–2000 / high 2k–12k Hz from
  RBJ Butterworth biquads (time domain: exact edges at any rate, no FFT bin smearing).
- Wire: 20-byte LE frame = sample rate u32, RMS dBFS f32, bass/mid/high dBFS f32. WebSocket on port+1.
- The bridge never outputs audio.

## Page model
- `pluto_sim.png` per sim cell: R = rank order in which the ground reddens (0 first), G = red already in the
  photo (pool), B = Sputnik Planitia renewing-ice mask. No alpha (browsers premultiply it).
- Snow sim (RGBA32F ping-pong): `t += dt*fall*flake; t *= exp(-BURY*dt)`, flake = integer hash per cell
  (p 0.2, weight 5), `FALL = BURY = 1/8` so t settles at the loudness level. Steps per frame
  `ceil(dt*flow*60)`, max 16.
- Display: threshold `thr = order + detail/raggedness terms`; `amt = smoothstep(thr±0.08, t)·(1−pool)·(1−0.75·renew)`;
  thickness grades dust (ice × stain^0.3, peach) to the photo's tholin red × photo texture, shaded by the
  photo's local luminance so lit/shadowed ground stays readable.
- Loudness: −64…−8 dBFS mapped to 0…1 (56 dB), smoothing up `dt·1.5` / down `dt·0.8`, beat flurry `(1+beat)`.
- Test hooks: `?level=X` pins loudness, `?warm=N` runs N sim steps on the first frame.

## Design decisions (settled with the operator — keep)
- The red is tholin snow, never a liquid/flow; it grows from the photo's own red, never from scratch.
- Coverage is a rank order so it is linear in loudness over the whole range (dynamic range was the main
  complaint before). Avoid smooth airbrush gradients and large patches switching at once.
- Sputnik Planitia stays white (convecting N2 ice renews).
- Photo geometry fit: lat 33.134, lon 165.184, rot 5.925°, disc cx 2405.9 cy 2367.6 r 2110.0 in the
  5000² PIA19952. Albedo fit on blurred maps with the disc fixed from the limb; high-pass/fine-detail
  fits failed (the map mixes lighting).
