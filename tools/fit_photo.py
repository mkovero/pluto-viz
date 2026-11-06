"""Fit the viewing geometry of the PIA19952 photo against the USGS LORRI global map.

The disc (centre and radius in photo pixels) comes from the limb; the sub-observer latitude,
longitude and the image rotation of north are fitted: the photo's blurred luminance (albedo
pattern) is correlated with an orthographic render of the map over the inner disc, on a grid
and then refined. Fine detail does not correlate (the map mixes images under other lighting).

usage: python3 tools/fit_photo.py   -> prints the fitted JSON, writes data/pia/fit.json
"""
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image
from scipy import ndimage, optimize

Image.MAX_IMAGE_PIXELS = None
ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "data"
M = 768  # fit resolution


def ortho_inv(x, y, lat0, lon0, rot):
    """Unit-disc coords (x right, y up) -> lon/lat, with north rotated by rot (radians)."""
    c, s = np.cos(rot), np.sin(rot)
    x, y = c * x + s * y, -s * x + c * y
    rho = np.hypot(x, y)
    cc = np.arcsin(np.clip(rho, 0, 1))
    p0, l0 = np.radians(lat0), np.radians(lon0)
    with np.errstate(invalid="ignore", divide="ignore"):
        lat = np.arcsin(np.cos(cc) * np.sin(p0) + np.where(rho > 0, y * np.sin(cc) * np.cos(p0) / rho, 0))
        lon = l0 + np.arctan2(x * np.sin(cc), rho * np.cos(cc) * np.cos(p0) - y * np.sin(cc) * np.sin(p0))
    return np.degrees(lon) % 360.0, np.degrees(lat)


def main():
    photo = Image.open(DATA / "pia" / "PIA19952.jpg").convert("L")
    W = photo.width
    ph = np.asarray(photo.resize((M, M), Image.BOX), np.float32)
    # Disc from a threshold: centre and radius from area/moments as a first guess.
    m = ph > 0.08 * ph.max()
    m = ndimage.binary_fill_holes(ndimage.binary_opening(m, iterations=2))
    yy, xx = np.nonzero(m)
    cx0, cy0, r0 = xx.mean(), yy.mean(), np.sqrt(m.sum() / np.pi)
    print("disc guess", cx0, cy0, r0, file=sys.stderr)

    lorri = np.memmap(DATA / "lorri.tif", dtype=np.uint8, mode="r", offset=100191, shape=(12444, 24888))
    small = np.asarray(Image.fromarray(np.asarray(lorri)).resize((4096, 2048), Image.BOX), np.float32)
    valid = small > 0
    sh = ndimage.gaussian_filter(small, 2)

    php = ndimage.gaussian_filter(ph, 1.5)  # map and photo at similar blur
    py, px = np.mgrid[0:M, 0:M].astype(np.float64) + 0.5

    def render(p):
        cx, cy, r, lat0, lon0, rot = p
        x = (px - cx) / r
        y = -(py - cy) / r
        inner = np.hypot(x, y) < 0.8
        lon, lat = ortho_inv(x[inner], y[inner], lat0, lon0, rot)
        cols = lon / 360 * small.shape[1] - 0.5
        rows = (90 - lat) / 180 * small.shape[0] - 0.5
        v = ndimage.map_coordinates(sh, [rows, cols], order=1, mode="wrap")
        ok = ndimage.map_coordinates(valid.astype(np.float32), [rows, cols], order=1, mode="wrap") > 0.99
        return inner, v, ok

    def cost(p):
        inner, v, ok = render(p)
        a, b = php[inner][ok], v[ok]
        if a.size < 1000:
            return 1.0
        a = a - a.mean(); b = b - b.mean()
        return -float((a * b).sum() / np.sqrt((a * a).sum() * (b * b).sum() + 1e-9))

    best = None
    for lat0 in range(10, 61, 5):
        for lon0 in range(140, 211, 5):
            for rot in np.radians(range(-30, 31, 5)):
                p = (cx0, cy0, r0, lat0, lon0, rot)
                c = cost(p)
                if best is None or c < best[0]:
                    best = (c, p)
    print("grid", best, file=sys.stderr)
    # The disc stays where the limb is: freed, the optimiser shrinks it onto a sub-region.
    disc = best[1][:3]
    sub = lambda q: cost((*disc, *q))
    res = optimize.minimize(sub, best[1][3:], method="Nelder-Mead",
                            options={"xatol": 1e-3, "fatol": 1e-6, "maxiter": 3000})
    print("refined", res.fun, res.x, file=sys.stderr)
    res.x = np.array([*disc, *res.x])
    best_c = res.fun
    cx, cy, r, lat0, lon0, rot = res.x
    k = W / M
    fit = {"cx": cx * k, "cy": cy * k, "r": r * k, "lat": lat0, "lon": lon0 % 360,
           "rot_deg": float(np.degrees(rot)), "corr": -best_c, "width": W}
    (DATA / "pia" / "fit.json").write_text(json.dumps(fit, indent=1))
    print(json.dumps(fit))

    # Overlay check: photo vs rendered map side by side.
    inner, v, ok = render(res.x)
    img = np.zeros((M, M), np.float32)
    img[inner] = np.where(ok, v, 0)
    img[inner] = (img[inner] - v[ok].mean()) / (3 * v[ok].std()) * 0.5 + 0.5
    img = np.clip(img, 0, 1)
    pn = np.clip((php - php[inner].mean()) / (3 * php[inner].std()) * 0.5 + 0.5, 0, 1)
    rgb = np.stack([pn, img, img], -1)  # grey where aligned, colour fringes where not
    Image.fromarray((rgb * 255).astype(np.uint8)).save(DATA / "pia" / "fit_overlay.png")


if __name__ == "__main__":
    main()
