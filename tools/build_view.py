"""Build the visualiser's image and simulation maps from the New Horizons photo PIA19952.

The base picture is NASA's enhanced-colour portrait PIA19952 (MVIC colour over LORRI detail,
5000x5000, data/pia/PIA19952.jpg). Its viewing geometry comes from tools/fit_photo.py
(data/pia/fit.json). The USGS LORRI/MVIC 300 m DEM (data/dem.tif, int16 metres,
equirectangular, longitude 0..360 east) is reprojected into that geometry for the slopes
that shed the tholin snow.

Outputs (repo root):
  pluto.jpg        SIZE² crop of the photo, the disc exactly filling the square
  pluto_sim.png    SIM² RGB: R = order in which the ground turns red (0 first, 1 last), G = red in
                   the photo, B = self-renewing nitrogen ice (Sputnik Planitia). No alpha: browsers premultiply PNG alpha and would wipe it.
  pluto_view.json  view geometry, the photo's tholin red, stain (tholin/ice colour ratio)

usage: python3 tools/build_view.py [--size 4096] [--sim 1024]
"""
import argparse
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image
from scipy import ndimage

sys.path.insert(0, str(Path(__file__).resolve().parent))
from fit_photo import ortho_inv  # noqa: E402

Image.MAX_IMAGE_PIXELS = None
ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "data"


def crop_disc(img, fit, n):
    """The photo resampled so that the fitted disc fills an n×n square."""
    cx, cy, r = fit["cx"], fit["cy"], fit["r"]
    box = (cx - r, cy - r, cx + r, cy + r)
    return img.transform((n, n), Image.EXTENT, box, Image.BICUBIC)


def grid_lonlat(n, fit):
    y, x = np.mgrid[0:n, 0:n].astype(np.float64)
    x = (x + 0.5) / n * 2 - 1
    y = 1 - (y + 0.5) / n * 2
    inside = np.hypot(x, y) <= 1.0
    lon, lat = ortho_inv(x[inside], y[inside], fit["lat"], fit["lon"], np.radians(fit["rot_deg"]))
    return lon, lat, inside


def sample(arr, lon, lat):
    h, w = arr.shape
    return ndimage.map_coordinates(arr, [(90.0 - lat) / 180.0 * h - 0.5, lon / 360.0 * w - 0.5],
                                   order=1, mode="wrap")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--size", type=int, default=4096)
    ap.add_argument("--sim", type=int, default=1024)
    a = ap.parse_args()

    fit = json.loads((DATA / "pia" / "fit.json").read_text())
    photo = Image.open(DATA / "pia" / "PIA19952.jpg").convert("RGB")
    crop_disc(photo, fit, a.size).save(ROOT / "pluto.jpg", quality=93)
    print("wrote pluto.jpg", file=sys.stderr)

    s = a.sim
    lon, lat, inside = grid_lonlat(s, fit)
    dem = np.array(np.memmap(DATA / "dem.tif", dtype="<i2", mode="r", offset=100199,
                             shape=(12444, 24888)), dtype=np.float32)
    dvalid_map = dem > -20000
    dem_f = np.where(dvalid_map, dem, np.median(dem[dvalid_map]))
    h = sample(dem_f, lon, lat)
    dv = sample(dvalid_map.astype(np.float32), lon, lat) > 0.5

    # Slope (rise over run) of the ground: steep walls shed the falling tholin. One grid cell
    # spans PIX metres at the disc centre; towards the limb the ground is foreshortened by mu.
    PIX = 2 * 1188.3e3 / s
    himg = np.zeros((s, s), np.float32)
    himg[inside] = np.where(dv, h, np.median(h[dv]))
    himg = ndimage.gaussian_filter(himg, 1.0)
    gy, gx = np.gradient(himg, PIX)
    yy, xx = (np.mgrid[0:s, 0:s] + 0.5 - s / 2) / (s / 2)
    mu = np.sqrt(np.clip(1 - xx**2 - yy**2, 0.05, 1))
    slope = np.hypot(gx, gy) * mu
    dvimg = np.zeros((s, s), bool)
    dvimg[inside] = dv
    slope[~dvimg] = 0.0

    # Red terrain from the photo itself, so the source sits exactly on what is seen red.
    # Enhanced colour maps NIR/Red/Blue to R/G/B: tholins are bright in R and dim in G and B,
    # while the yellowish north is bright in both R and G, so R - max(G, B) singles them out.
    px = np.asarray(crop_disc(photo, fit, s), np.float32) / 255.0
    idx = (px[..., 0] - np.maximum(px[..., 1], px[..., 2])) / np.maximum(px.sum(-1), 0.05)
    idx = ndimage.gaussian_filter(idx, 1.5)
    inner = inside & (np.hypot(*(np.mgrid[0:s, 0:s] + 0.5 - s / 2)) < 0.97 * s / 2)
    p50, p97 = np.percentile(idx[inner], [50, 97])
    red_img = np.clip((idx - p50) / (p97 - p50), 0, 1) * inner
    red_img = ndimage.gaussian_filter(red_img, 1.0)

    # Where tholin snow settles and stays: next to the ground that is already red (the haze
    # drops it everywhere, but fresh nitrogen frost buries it away from there), in patches,
    # and less on steep walls.
    dist = ndimage.distance_transform_edt(red_img < 0.5)
    halo = np.exp(-dist / (0.15 * s))
    rng = np.random.default_rng(14)
    noise = (ndimage.gaussian_filter(rng.standard_normal((s, s)), 8) * 0.5
             + ndimage.gaussian_filter(rng.standard_normal((s, s)), 30) * 1.0)
    patch = np.clip(1 + 0.22 * noise / noise[inside].std(), 0.4, 1.6)
    stick = 1 - 0.75 * np.clip((slope - 0.06) / 0.25, 0, 1)
    # The brightest ground is fresh nitrogen ice that keeps burying the tholins (the heart).
    lum = px @ np.array([0.299, 0.587, 0.114], np.float32)
    lo, hi = np.percentile(lum[inner], [40, 95])
    fresh = 1 - 0.6 * np.clip((ndimage.gaussian_filter(lum, 2) - lo) / (hi - lo), 0, 1)
    aff = np.clip((0.1 + 0.9 * halo) * patch * stick * fresh, 0, 1) * inside

    # Order in which the ground turns red: the rank of aff over the free (not yet red) disc,
    # 0 = first, 1 = last. The page reddens ground whose order is below the loudness, so the
    # covered share follows the level linearly over its whole range.
    free = inside & (red_img < 0.5)
    order = np.zeros((s, s), np.float32)
    v = aff[free]
    order[free] = np.argsort(np.argsort(-v, kind="stable")) / max(v.size - 1, 1)
    order = np.where(free, ndimage.gaussian_filter(order, 1.0), 0.0)

    # Sputnik Planitia (the heart's left lobe): an impact basin filled with nitrogen ice that
    # convects and renews its surface, so tholins never build up there. In the photo it is
    # the bright ground with almost no texture.
    lum_s = ndimage.gaussian_filter(lum, 4)
    tex = np.sqrt(ndimage.gaussian_filter((lum - ndimage.gaussian_filter(lum, 1.5))**2, 6))
    smooth = 1 - np.clip((tex - np.percentile(tex[inner], 5)) / (np.percentile(tex[inner], 30) - np.percentile(tex[inner], 5)), 0, 1)
    bright = np.clip((lum_s - np.percentile(lum[inner], 55)) / 0.06, 0, 1)
    m = ndimage.binary_opening(smooth * bright > 0.5, iterations=4)
    lab, nlab = ndimage.label(m)
    if nlab:
        big = np.argmax(ndimage.sum(m, lab, range(1, nlab + 1))) + 1
        # Grow the core out through connected ground that is still fairly bright and smooth.
        loose = (smooth > 0.15) & (lum_s > np.percentile(lum[inner], 45))
        m = ndimage.binary_propagation(lab == big, mask=loose | (lab == big))
        m = ndimage.binary_fill_holes(ndimage.binary_closing(m, iterations=8))
    renew = np.clip(ndimage.gaussian_filter(m.astype(np.float32), 4) * 1.4, 0, 1) * inside

    out = np.zeros((s, s, 3), np.uint8)
    out[..., 0] = np.round(np.clip(order, 0, 1) * 255).astype(np.uint8)
    out[..., 2] = np.round(renew * 255).astype(np.uint8)
    out[..., 1] = np.round(red_img * 255).astype(np.uint8)
    out[~inside] = 0
    Image.fromarray(out, "RGB").save(ROOT / "pluto_sim.png")
    Image.fromarray(out[..., 0], "L").save(DATA / "aff_preview.png")
    Image.fromarray(np.round(red_img * 255).astype(np.uint8), "L").save(DATA / "red_preview.png")

    # Tholin colour: the photo's own red, the median over the reddest ground away from
    # the limb (where the photo's shading would darken it).
    sel = (red_img > 0.8) & (np.hypot(*(np.mgrid[0:s, 0:s] + 0.5 - s / 2)) < 0.85 * s / 2)
    tholin = np.median(px[sel], 0)
    # Stain: the colour ratio of tholin-covered ground to clean ice in the photo. Multiplying
    # the photo by it reddens and darkens ground the way the real red regions are, while
    # keeping its texture and light.
    ice = (red_img < 0.03) & (np.hypot(*(np.mgrid[0:s, 0:s] + 0.5 - s / 2)) < 0.85 * s / 2)
    stain = tholin / np.median(px[ice], 0)

    meta = {"lat": round(fit["lat"], 3), "lon": round(fit["lon"], 3), "rot_deg": round(fit["rot_deg"], 3),
            "tholin_rgb": [round(float(v), 4) for v in tholin],
            "stain_rgb": [round(float(v), 4) for v in stain]}
    (ROOT / "pluto_view.json").write_text(json.dumps(meta, indent=1))
    print(meta, file=sys.stderr)


if __name__ == "__main__":
    main()
