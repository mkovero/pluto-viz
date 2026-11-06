//! JACK client that feeds the Pluto visualizer.
//!
//! Two JACK inputs (auto-connected to the first physical capture ports unless
//! `--connect` says otherwise) are mixed to mono and analysed ~60 times a second: the RMS
//! level over the last 2048 samples, and the bass / mid / high band levels since the last
//! frame from time-domain filters (exact band edges at any sample rate, one frame of
//! latency; FFT bins at 96 kHz would put the whole bass band in three bins). The levels go
//! out over a WebSocket. The same binary serves the page and the images over HTTP, so the
//! browser needs nothing else.

use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const N: usize = 2048;

static INDEX: &str = include_str!("../../index.html");
static PLUTO: &[u8] = include_bytes!("../../pluto.jpg");
static SIM: &[u8] = include_bytes!("../../pluto_sim.png");
static VIEW: &[u8] = include_bytes!("../../pluto_view.json");

struct Args {
    bind: String,
    port: u16,
    name: String,
    connect: Vec<String>,
}

fn usage() -> ! {
    eprintln!(
        "usage: pluto-viz [--bind ADDR] [--port N] [--name JACK_NAME] [--connect PORT]...\n\
         \n  --bind     listen address (default 127.0.0.1)\
         \n  --port     HTTP port; the WebSocket uses PORT+1 (default 8765)\
         \n  --name     JACK client name (default pluto-viz)\
         \n  --connect  JACK source port to feed in (repeat; 1st -> in_1, 2nd -> in_2).\
         \n             Default: the first two physical capture ports."
    );
    std::process::exit(2)
}

fn parse_args() -> Args {
    let mut a = Args { bind: "127.0.0.1".into(), port: 8765, name: "pluto-viz".into(), connect: vec![] };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().unwrap_or_else(|| usage());
        match k.as_str() {
            "--bind" => a.bind = v(),
            "--port" => a.port = v().parse().unwrap_or_else(|_| usage()),
            "--name" => a.name = v(),
            "--connect" => a.connect.push(v()),
            _ => usage(),
        }
    }
    a
}

type Clients = Arc<Mutex<Vec<Sender<Vec<u8>>>>>;

fn main() {
    let args = parse_args();

    let (client, _) = jack::Client::new(&args.name, jack::ClientOptions::NO_START_SERVER)
        .unwrap_or_else(|e| {
            eprintln!("pluto-viz: cannot reach a JACK server: {e}");
            std::process::exit(1)
        });
    let sr = client.sample_rate();
    let in1 = client.register_port("in_1", jack::AudioIn::default()).expect("register in_1");
    let in2 = client.register_port("in_2", jack::AudioIn::default()).expect("register in_2");
    let dests = [in1.name().expect("port name"), in2.name().expect("port name")];

    // The process callback only copies samples into a lock-free ring buffer:
    // no allocation, locks or syscalls on the audio thread.
    let rb = jack::RingBuffer::new(1 << 18).expect("ring buffer");
    let (mut reader, mut writer) = rb.into_reader_writer();
    let process = jack::contrib::ClosureProcessHandler::new(
        move |_: &jack::Client, ps: &jack::ProcessScope| -> jack::Control {
            let (a, b) = (in1.as_slice(ps), in2.as_slice(ps));
            for (x, y) in a.iter().zip(b) {
                let s = 0.5 * (x + y);
                if writer.space() >= 4 {
                    writer.write_buffer(&s.to_le_bytes());
                }
            }
            jack::Control::Continue
        },
    );
    let active = client.activate_async((), process).expect("activate JACK client");

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
        let _ = active.as_client().connect_ports_by_name(&sources[0], &dests[1]);
    }

    let clients: Clients = Arc::new(Mutex::new(Vec::new()));

    let ws_clients = clients.clone();
    let ws_addr = format!("{}:{}", args.bind, args.port + 1);
    let ws = TcpListener::bind(&ws_addr).unwrap_or_else(|e| {
        eprintln!("pluto-viz: bind {ws_addr}: {e}");
        std::process::exit(1)
    });
    thread::spawn(move || {
        for stream in ws.incoming().flatten() {
            let (tx, rx) = mpsc::channel();
            ws_clients.lock().unwrap().push(tx);
            thread::spawn(move || serve_ws(stream, rx));
        }
    });

    let http_addr = format!("{}:{}", args.bind, args.port);
    let http = tiny_http::Server::http(&http_addr).unwrap_or_else(|e| {
        eprintln!("pluto-viz: bind {http_addr}: {e}");
        std::process::exit(1)
    });
    thread::spawn(move || serve_http(http));
    eprintln!("pluto-viz: open http://{http_addr}/  (JACK {sr} Hz)");

    analyse_loop(&mut reader, sr, &clients);
    drop(active);
}

fn serve_ws(stream: std::net::TcpStream, rx: Receiver<Vec<u8>>) {
    let _ = stream.set_nodelay(true);
    let Ok(mut ws) = tungstenite::accept(stream) else { return };
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
        let _ = req.respond(tiny_http::Response::from_data(body).with_header(hdr).with_header(nc));
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
        Biquad { b: b.map(|x| x / a0), a: [-2.0 * cs / a0, (1.0 - alpha) / a0], z: [0.0; 2] }
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
fn analyse_loop(reader: &mut jack::RingBufferReader, sr: u32, clients: &Clients) {
    let mut hist = vec![0f32; N];
    let mut pos = 0usize;
    let mut bands = Bands::new(sr as f32);
    let mut raw = vec![0u8; 4096];
    let mut msg = vec![0u8; 20];
    msg[..4].copy_from_slice(&sr.to_le_bytes());
    let period = Duration::from_micros(16_667);
    let mut next = Instant::now();

    loop {
        next += period;
        let now = Instant::now();
        if next > now {
            thread::sleep(next - now);
        } else {
            next = now;
        }

        let mut got = false;
        loop {
            let n = reader.read_buffer(&mut raw) / 4 * 4;
            if n == 0 {
                break;
            }
            got = true;
            for c in raw[..n].chunks_exact(4) {
                let x = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                hist[pos] = x;
                pos = (pos + 1) % N;
                bands.push(x);
            }
        }
        if !got {
            continue;
        }

        let rms = (hist.iter().map(|x| x * x).sum::<f32>() / N as f32).sqrt();
        msg[4..8].copy_from_slice(&(20.0 * rms.max(1e-9).log10()).to_le_bytes());
        for (i, db) in bands.take().iter().enumerate() {
            msg[8 + 4 * i..12 + 4 * i].copy_from_slice(&db.to_le_bytes());
        }

        let mut list = clients.lock().unwrap();
        list.retain(|tx| tx.send(msg.clone()).is_ok());
    }
}
