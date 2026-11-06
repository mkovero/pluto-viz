# pluto-viz

A screensaver-style music visualiser: Pluto as New Horizons photographed it, with red
tholin snow falling on it to the music. Silence leaves the planet as photographed; as the
music gets louder, red spreads out from Pluto's existing red ground across the ice, following
the real terrain, and frost slowly buries it again when the music goes quiet.

Audio comes from JACK. A small Rust bridge reads two JACK inputs, measures loudness and band
levels, and serves the page; the page renders with WebGL2 in any current browser.

## What the red is: tholins

Pluto's red is **tholins**: complex organic molecules made when sunlight's ultraviolet and
charged particles break up methane and nitrogen high in Pluto's thin atmosphere. The
fragments recombine into ever larger molecules that form Pluto's blue-tinged haze layers,
and the particles slowly settle out — a very fine, reddish-brown snow falling on the
surface.

Where nothing buries it, it builds up: the dark red belt along the equator (Cthulhu
Macula, the large red region at lower left in the picture) is old ground that has been
collecting it for a very long time. Where fresh ice keeps forming, the surface stays bright:

- **Sputnik Planitia**, the left lobe of the bright "heart" (Tombaugh Regio), is a basin
  about 1000 km across, thought to be an ancient impact, filled with nitrogen ice that slowly
  convects in cells a few tens of kilometres wide. Its surface renews itself in under a
  million years, so tholins never get to accumulate there.
- Bright, fresh nitrogen and methane frost on the highlands hides them too.

Pluto also colours its moon: methane escaping Pluto's atmosphere drifts to Charon, freezes
onto Charon's very cold winter pole, and turns into tholins there — Charon's red polar cap,
Mordor Macula.

## What the visualiser does

- **The picture** is NASA's enhanced-colour portrait of Pluto, PIA19952 (New Horizons, July
  2015: MVIC colour combined with LORRI detail), shown in its own colours and light,
  dimmed for a dark room.
- **Terrain** comes from the USGS New Horizons global mosaic and elevation model (300 m per
  pixel), reprojected into the photo's viewing geometry. That geometry (sub-observer
  latitude 33.1°, longitude 165.2°, north rotated 5.9°) was found by fitting the photo
  against the map (`tools/fit_photo.py`).
- **Snowfall follows loudness.** Flakes land as a fine random grain and pile up. Each spot
  of ground turns red in a fixed order: next to the red ground first, then outward, flat
  ground before steep walls, the photo's dark hollows before its bright rims and ridges.
  The covered share of the free ground follows the loudness over a 56 dB range, so quiet
  passages keep the red close to Cthulhu and the loudest reach nearly the whole disc.
- **A thin dusting is a light peach tint**; only where the snow piles up does the ground
  reach the photo's deep tholin red. The ground's texture stays visible under it.
- **Sputnik Planitia stays white**: snow only tints it slightly, because the real plain
  keeps renewing its ice.
- **Bass** makes the snow fall harder, and kicks bring a short flurry.
- **In silence** fresh frost slowly buries the snow, back to the photograph.

## Keys

| Key | Action |
|---|---|
| F, double-click | Fullscreen |
| H | Show / hide the key hint |
| ↑ / ↓ (+ / −) | Input sensitivity ±3 dB (−30 … +30 dB) |
| → / ← (] / [) | Snowfall speed ×1.25 (0.1× … 64×): how fast snow settles and gets buried |
| . / , | Brightness ±5 % (5 … 100 %, default 45 %) |
| 0 | Reset all three |

Settings are kept in the browser's local storage. After a change, a readout shows the
settings and the input level on the scale the snowfall uses.

## Running

You need a running JACK server (PipeWire's JACK works), Rust 1.85 or newer, and a browser
with WebGL2 and float render targets (any current Chrome, Brave, Firefox or Edge).

```sh
./run.sh                                         # builds the bridge, starts it, opens the page
./run.sh --connect system:capture_1 --connect system:capture_2
```

The bridge (`bridge/`, binary `pluto-viz`) embeds the page and images, so the single binary
is all a machine needs:

```
pluto-viz [--bind ADDR] [--port N] [--name JACK_NAME] [--connect PORT]...
  --bind     listen address (default 127.0.0.1)
  --port     HTTP port; the WebSocket uses PORT+1 (default 8765)
  --name     JACK client name (default pluto-viz)
  --connect  JACK source port to feed in (repeat; 1st -> in_1, 2nd -> in_2).
             Default: the first two physical capture ports.
```

Open `http://127.0.0.1:8765/`. The bridge only reads audio; it never outputs any.

To visualise what the desktop is playing through PipeWire, connect the bridge to the
playback ports, e.g. `--connect PipeWire:playback_FL --connect PipeWire:playback_FR` (check
the names with `jack_lsp`). `start-kiosk.sh` does that and opens the page in a Brave app
window with its own profile, so a GPU reset in your everyday browser does not leave WebGL
blocked there.

Test hooks: `?level=0.5` pins the loudness (no audio needed); `?warm=N` runs N snow steps
on the first frame.

## Rebuilding the images

The repository includes the built images (`pluto.jpg`, `pluto_sim.png`, `pluto_view.json`).
To rebuild them from the source data (needs Python 3 with numpy, scipy and Pillow):

```sh
tools/fetch_data.sh             # ~1 GB into data/
python3 tools/fit_photo.py      # photo viewing geometry -> data/pia/fit.json
python3 tools/build_view.py     # pluto.jpg, pluto_sim.png, pluto_view.json
cargo build --release --manifest-path bridge/Cargo.toml   # embeds them
```

`pluto_sim.png` holds, per simulation cell: R = the order in which the ground turns red,
G = the ground already red in the photo, B = Sputnik Planitia's self-renewing ice.

## Credits

- PIA19952 "Pluto in Enhanced Color": NASA / Johns Hopkins University Applied Physics
  Laboratory / Southwest Research Institute.
- Pluto New Horizons LORRI/MVIC global mosaic and DEM, 300 m/px (July 2017): USGS
  Astrogeology Science Center, from New Horizons data (NASA / JHUAPL / SwRI); Schenk et
  al. 2018.

NASA and USGS imagery is not subject to copyright in the United States; see NASA's media
usage guidelines. The code is under the MIT licence (`LICENSE`).
