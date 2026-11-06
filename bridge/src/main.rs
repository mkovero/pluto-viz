//! Audio bridge for the Pluto visualiser.
//!
//! Captures audio (JACK on Linux; on Windows and macOS the system audio API through cpal),
//! mixes it to mono and analyses it ~60 times a second: the RMS level over the last 2048
//! samples, and the bass / mid / high band levels since the last frame from time-domain
//! filters (exact band edges at any sample rate, one frame of latency). The levels go out
//! over a WebSocket. The same binary serves the page and the images over HTTP, so the
//! browser needs nothing else.

use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const N: usize = 2048;
/// Capture ring: ~0.7 s at 96 kHz, far more than the 17 ms the analysis drains per frame.
const RING: usize = 1 << 16;

static INDEX: &str = include_str!("../../index.html");
static PLUTO: &[u8] = include_bytes!("../../pluto.jpg");
static SIM: &[u8] = include_bytes!("../../pluto_sim.png");
static VIEW: &[u8] = include_bytes!("../../pluto_view.json");

struct Args {
    bind: String,
    port: u16,
    name: String,
    connect: Vec<String>,
    device: Option<String>,
    list_devices: bool,
    jack: bool,
}

#[cfg(target_os = "linux")]
const SOURCE_HELP: &str = "\
  --device   PipeWire device, part of its name; an output (sink) is recorded as what it
             plays. Prefix in: or out: when a source and a sink share the name.
             Default: the default output, i.e. the desktop audio.
  --list-devices  list the PipeWire devices
  --jack     capture from JACK instead (implied by --name and --connect)
  --name     JACK client name (default pluto-viz)
  --connect  JACK source port to feed in (repeat; 1st -> in_1, 2nd -> in_2).
             Default: the first two physical capture ports.";
#[cfg(target_os = "windows")]
const SOURCE_HELP: &str = "\
  --device   capture device, part of its name; an output device is recorded as loopback
             (what it plays). Prefix in: or out: when an input and an output share the
             name. Default: the default output device, i.e. the desktop audio.
  --list-devices  list the capture and output devices";
#[cfg(target_os = "macos")]
const SOURCE_HELP: &str = "\
  --device   capture device, part of its name. Default: the default input device. For
             desktop audio route it through a loopback device such as BlackHole.
  --list-devices  list the capture devices";

fn usage() -> ! {
    help(2)
}

fn help(code: i32) -> ! {
    eprintln!(
        "usage: pluto-viz [--bind ADDR] [--port N] [source options]\n\
         \n  --bind     listen address (default 127.0.0.1)\
         \n  --port     HTTP port; the WebSocket uses PORT+1 (default 8765)\n  {SOURCE_HELP}"
    );
    std::process::exit(code)
}

fn parse_args() -> Args {
    let mut a = Args {
        bind: "127.0.0.1".into(),
        port: 8765,
        name: "pluto-viz".into(),
        connect: vec![],
        device: None,
        list_devices: false,
        jack: false,
    };
    let linux = cfg!(target_os = "linux");
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().unwrap_or_else(|| usage());
        match k.as_str() {
            "--bind" => a.bind = v(),
            "--port" => a.port = v().parse().unwrap_or_else(|_| usage()),
            "--jack" if linux => a.jack = true,
            "--name" if linux => {
                a.name = v();
                a.jack = true;
            }
            "--connect" if linux => {
                a.connect.push(v());
                a.jack = true;
            }
            "--device" => a.device = Some(v()),
            "--list-devices" => a.list_devices = true,
            "-h" | "--help" => help(0),
            _ => usage(),
        }
    }
    if a.jack && (a.device.is_some() || a.list_devices) {
        fail("--device and --list-devices are for PipeWire, not JACK".into())
    }
    a
}

/// A running capture: samples (mono) arrive in `rx`; dropping `_keep` stops it.
struct Capture {
    sr: u32,
    rx: rtrb::Consumer<f32>,
    _keep: Box<dyn std::any::Any>,
}

fn fail(msg: String) -> ! {
    eprintln!("pluto-viz: {msg}");
    std::process::exit(1)
}

fn start_capture(args: &Args) -> Capture {
    #[cfg(target_os = "linux")]
    if args.jack {
        return start_jack(args);
    }
    start_cpal(args)
}

#[cfg(target_os = "linux")]
fn start_jack(args: &Args) -> Capture {
    let (client, _) = jack::Client::new(&args.name, jack::ClientOptions::NO_START_SERVER)
        .unwrap_or_else(|e| fail(format!("cannot reach a JACK server: {e}")));
    let sr = client.sample_rate();
    let in1 = client
        .register_port("in_1", jack::AudioIn::default())
        .expect("register in_1");
    let in2 = client
        .register_port("in_2", jack::AudioIn::default())
        .expect("register in_2");
    let dests = [
        in1.name().expect("port name"),
        in2.name().expect("port name"),
    ];

    // The process callback only pushes samples into a lock-free ring: no allocation, locks
    // or syscalls on the audio thread. When the ring is full, samples are dropped.
    let (mut tx, rx) = rtrb::RingBuffer::new(RING);
    let process = jack::contrib::ClosureProcessHandler::new(
        move |_: &jack::Client, ps: &jack::ProcessScope| -> jack::Control {
            for (x, y) in in1.as_slice(ps).iter().zip(in2.as_slice(ps)) {
                let _ = tx.push(0.5 * (x + y));
            }
            jack::Control::Continue
        },
    );
    let active = client
        .activate_async((), process)
        .expect("activate JACK client");

    let sources: Vec<String> = if args.connect.is_empty() {
        active.as_client().ports(
            None,
            Some(jack::jack_sys::FLOAT_MONO_AUDIO),
            jack::PortFlags::IS_PHYSICAL | jack::PortFlags::IS_OUTPUT,
        )
    } else {
        args.connect.clone()
    };
    for (src, dst) in sources.iter().zip(dests.iter()) {
        match active.as_client().connect_ports_by_name(src, dst) {
            Ok(()) => eprintln!("pluto-viz: {src} -> {dst}"),
            Err(e) => eprintln!("pluto-viz: connect {src} -> {dst} failed: {e}"),
        }
    }
    if sources.len() == 1 {
        let _ = active
            .as_client()
            .connect_ports_by_name(&sources[0], &dests[1]);
    }
    Capture {
        sr,
        rx,
        _keep: Box::new(active),
    }
}

fn start_cpal(args: &Args) -> Capture {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    // WASAPI and PipeWire record an output device as what it plays (loopback / sink
    // monitor), so by default the bridge follows the desktop audio. Core Audio cannot.
    let loopback = !cfg!(target_os = "macos");
    let host = cpal::default_host();
    let name = |d: &cpal::Device| {
        d.description()
            .map(|x| x.name().to_owned())
            .unwrap_or_default()
    };
    let outputs: Vec<cpal::Device> = if loopback {
        host.output_devices()
            .map(|d| d.collect())
            .unwrap_or_default()
    } else {
        vec![]
    };
    // PipeWire lists each sink as an input too; keep it once, as an output.
    let inputs: Vec<cpal::Device> = host
        .input_devices()
        .map(|d| {
            d.filter(|i| !outputs.iter().any(|o| o.id().ok() == i.id().ok()))
                .collect()
        })
        .unwrap_or_default();
    if args.list_devices {
        for d in &inputs {
            println!("input   {}", name(d));
        }
        for d in &outputs {
            println!("output  {}  (recorded as loopback)", name(d));
        }
        std::process::exit(0)
    }

    let (device, as_output) = match &args.device {
        Some(want) => {
            // "in:" / "out:" pick the side when a capture and an output device share a name
            // (PipeWire lists a card's source and sink under one name); otherwise the output
            // wins, since recording what plays is the usual aim.
            let want = want.to_lowercase();
            let (side, want) = match want.split_once(':') {
                Some(("in", w)) => (Some(false), w.trim()),
                Some(("out", w)) => (Some(true), w.trim()),
                _ => (None, want.as_str()),
            };
            let hit = |d: &&cpal::Device| name(d).to_lowercase().contains(want);
            let out = (side != Some(false))
                .then(|| outputs.iter().find(hit))
                .flatten();
            let inp = (side != Some(true))
                .then(|| inputs.iter().find(hit))
                .flatten();
            match (out, inp) {
                (Some(d), _) => (d.clone(), true),
                (None, Some(d)) => (d.clone(), false),
                _ => fail(format!(
                    "no device matching \"{want}\" (see --list-devices)"
                )),
            }
        }
        None if loopback => (
            host.default_output_device()
                .unwrap_or_else(|| fail("no default output device".into())),
            true,
        ),
        None => (
            host.default_input_device()
                .unwrap_or_else(|| fail("no default input device".into())),
            false,
        ),
    };
    let supported = if as_output {
        device.default_output_config()
    } else {
        device.default_input_config()
    }
    .unwrap_or_else(|e| fail(format!("{}: no usable format: {e}", name(&device))));
    let sr = supported.sample_rate();
    let ch = supported.channels().max(1) as usize;
    eprintln!(
        "pluto-viz: capturing {}{} ({ch} ch, {sr} Hz)",
        name(&device),
        if as_output { " (loopback)" } else { "" }
    );

    let (tx, rx) = rtrb::RingBuffer::new(RING);
    let config: cpal::StreamConfig = supported.config();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build::<f32>(&device, &config, ch, tx),
        cpal::SampleFormat::I16 => build::<i16>(&device, &config, ch, tx),
        cpal::SampleFormat::I32 => build::<i32>(&device, &config, ch, tx),
        cpal::SampleFormat::U16 => build::<u16>(&device, &config, ch, tx),
        f => fail(format!("unsupported sample format {f:?}")),
    }
    .unwrap_or_else(|e| fail(format!("cannot open {}: {e}", name(&device))));
    stream
        .play()
        .unwrap_or_else(|e| fail(format!("cannot start {}: {e}", name(&device))));
    Capture {
        sr,
        rx,
        _keep: Box::new(stream),
    }
}

/// Input stream whose callback only mixes each frame to mono and pushes it into the ring:
/// no allocation or locks on the audio thread. When the ring is full, samples are dropped.
fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    ch: usize,
    mut tx: rtrb::Producer<f32>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    use cpal::traits::DeviceTrait;
    let k = 1.0 / ch as f32;
    device.build_input_stream::<T, _, _>(
        *config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            for frame in data.chunks_exact(ch) {
                let s: f32 = frame.iter().map(|x| x.to_sample::<f32>()).sum();
                let _ = tx.push(s * k);
            }
        },
        |e| eprintln!("pluto-viz: audio stream error: {e}"),
        None,
    )
}

type Clients = Arc<Mutex<Vec<Sender<Vec<u8>>>>>;

fn main() {
    let args = parse_args();
    let mut cap = start_capture(&args);

    let clients: Clients = Arc::new(Mutex::new(Vec::new()));

    let ws_clients = clients.clone();
    let ws_addr = format!("{}:{}", args.bind, args.port + 1);
    let ws = TcpListener::bind(&ws_addr).unwrap_or_else(|e| fail(format!("bind {ws_addr}: {e}")));
    thread::spawn(move || {
        for stream in ws.incoming().flatten() {
            let (tx, rx) = mpsc::channel();
            ws_clients.lock().unwrap().push(tx);
            thread::spawn(move || serve_ws(stream, rx));
        }
    });

    let http_addr = format!("{}:{}", args.bind, args.port);
    let http = tiny_http::Server::http(&http_addr)
        .unwrap_or_else(|e| fail(format!("bind {http_addr}: {e}")));
    thread::spawn(move || serve_http(http));
    eprintln!("pluto-viz: open http://{http_addr}/  ({} Hz)", cap.sr);

    // The capture stays on this thread (some backends' streams cannot move between threads).
    analyse_loop(&mut cap.rx, cap.sr, &clients);
}

fn serve_ws(stream: std::net::TcpStream, rx: Receiver<Vec<u8>>) {
    let _ = stream.set_nodelay(true);
    let Ok(mut ws) = tungstenite::accept(stream) else {
        return;
    };
    for msg in rx {
        if ws.send(tungstenite::Message::Binary(msg.into())).is_err() {
            return;
        }
    }
}

fn serve_http(server: tiny_http::Server) {
    for req in server.incoming_requests() {
        let path = req.url().split('?').next().unwrap_or("/").to_owned();
        let (body, ctype): (&[u8], &str) = match path.as_str() {
            "/" | "/index.html" => (INDEX.as_bytes(), "text/html; charset=utf-8"),
            "/pluto.jpg" => (PLUTO, "image/jpeg"),
            "/pluto_sim.png" => (SIM, "image/png"),
            "/pluto_view.json" => (VIEW, "application/json"),
            _ => {
                let _ = req.respond(tiny_http::Response::empty(404));
                continue;
            }
        };
        let hdr = tiny_http::Header::from_bytes("Content-Type", ctype).expect("header");
        // The page and maps change with every build; never let the browser keep an old copy.
        let nc = tiny_http::Header::from_bytes("Cache-Control", "no-store").expect("header");
        let _ = req.respond(
            tiny_http::Response::from_data(body)
                .with_header(hdr)
                .with_header(nc),
        );
    }
}

/// RBJ cookbook biquad, transposed direct form II.
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}

impl Biquad {
    fn new(sr: f32, f0: f32, high: bool) -> Self {
        let w = 2.0 * std::f32::consts::PI * f0 / sr;
        let (sn, cs) = w.sin_cos();
        let alpha = sn / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let b = if high {
            [(1.0 + cs) / 2.0, -(1.0 + cs), (1.0 + cs) / 2.0]
        } else {
            [(1.0 - cs) / 2.0, 1.0 - cs, (1.0 - cs) / 2.0]
        };
        Biquad {
            b: b.map(|x| x / a0),
            a: [-2.0 * cs / a0, (1.0 - alpha) / a0],
            z: [0.0; 2],
        }
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// Band split: Butterworth (Q = 1/sqrt 2) low/high-pass pairs at 25/160 Hz (bass),
/// 160/2000 Hz (mid) and 2000/12000 Hz (high).
struct Bands {
    f: [Biquad; 6],
    sum: [f64; 3],
    n: usize,
}

impl Bands {
    fn new(sr: f32) -> Self {
        let hi = (12000.0f32).min(0.45 * sr);
        Bands {
            f: [
                Biquad::new(sr, 25.0, true),
                Biquad::new(sr, 160.0, false),
                Biquad::new(sr, 160.0, true),
                Biquad::new(sr, 2000.0, false),
                Biquad::new(sr, 2000.0, true),
                Biquad::new(sr, hi, false),
            ],
            sum: [0.0; 3],
            n: 0,
        }
    }

    fn push(&mut self, x: f32) {
        let b = self.f[0].run(x);
        let b = self.f[1].run(b);
        let m = self.f[2].run(x);
        let m = self.f[3].run(m);
        let h = self.f[4].run(x);
        let h = self.f[5].run(h);
        self.sum[0] += (b * b) as f64;
        self.sum[1] += (m * m) as f64;
        self.sum[2] += (h * h) as f64;
        self.n += 1;
    }

    /// Band RMS levels in dBFS since the last call.
    fn take(&mut self) -> [f32; 3] {
        let n = self.n.max(1) as f64;
        let out = self.sum.map(|s| (10.0 * (s / n).max(1e-12).log10()) as f32);
        self.sum = [0.0; 3];
        self.n = 0;
        out
    }
}

/// Message (20 bytes, little endian): sample rate (u32), RMS of the last N samples in dBFS
/// (f32), then bass, mid and high band RMS since the previous message in dBFS (f32 each).
fn analyse_loop(rx: &mut rtrb::Consumer<f32>, sr: u32, clients: &Clients) {
    let mut hist = vec![0f32; N];
    let mut pos = 0usize;
    let mut bands = Bands::new(sr as f32);
    let mut msg = vec![0u8; 20];
    msg[..4].copy_from_slice(&sr.to_le_bytes());
    let period = Duration::from_micros(16_667);
    let mut next = Instant::now();
    let start = next;
    let mut warned = false;

    loop {
        next += period;
        let now = Instant::now();
        if next > now {
            thread::sleep(next - now);
        } else {
            next = now;
        }

        let n = rx.slots();
        let got = n > 0;
        if let Ok(chunk) = rx.read_chunk(n) {
            for x in chunk {
                hist[pos] = x;
                pos = (pos + 1) % N;
                bands.push(x);
            }
        }
        if !got {
            if !warned && start.elapsed() > Duration::from_secs(3) {
                warned = true;
                eprintln!(
                    "pluto-viz: no audio is arriving. The device may not be recordable (a sink \
                     bridged to JACK has no monitor): pick another with --device{}",
                    if cfg!(target_os = "linux") {
                        " or use --jack"
                    } else {
                        ""
                    }
                );
            }
            continue;
        }
        warned = true;

        let rms = (hist.iter().map(|x| x * x).sum::<f32>() / N as f32).sqrt();
        msg[4..8].copy_from_slice(&(20.0 * rms.max(1e-9).log10()).to_le_bytes());
        for (i, db) in bands.take().iter().enumerate() {
            msg[8 + 4 * i..12 + 4 * i].copy_from_slice(&db.to_le_bytes());
        }

        let mut list = clients.lock().unwrap();
        list.retain(|tx| tx.send(msg.clone()).is_ok());
    }
}
