"""Theme variants of the app icon, every .ico size recoloured on its own (the hand-drawn 16-48 px entries have
thicker strokes than a downscaled 256 px; owner, 2026-11-17).

    python -I tools/make_theme_icons.py

Reads InstantReplay/Assets/icon.ico (dark: near-black tile, cyan mark) and writes
crates/lag-app/assets/icon-light.ico (cream + bronze) and crates/lag-app/assets/icon-gta.ico (GTA VI theme:
violet-night tile, mark in a pink -> orange sunset gradient), plus a preview strip next to them.
Every pixel is split into "how much mark" and "how much track" by solving against the dark palette, then
rebuilt from the theme's palette, so anti-aliased edges keep their shape.
"""
import os
import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, "..", "..", "InstantReplay", "Assets", "icon.ico")
OUT = os.path.join(HERE, "..", "crates", "lag-app", "assets")

BG, LINE, TRACK = np.array([22, 27, 34.]), np.array([34, 211, 238.]), np.array([34, 41, 51.])
PINV = np.linalg.pinv(np.stack([LINE - BG, TRACK - BG], axis=1))


def weights(rgb):
    uv = (PINV @ (rgb - BG).reshape(-1, 3).T).T.reshape(rgb.shape[:2] + (2,))
    u, v = np.clip(uv[..., 0:1], 0, 1), np.clip(uv[..., 1:2], 0, 1)
    s = u + v
    k = np.where(s > 1, 1 / np.maximum(s, 1e-9), 1)
    return u * k, v * k


def recolour(img, bg, line, track):
    """`line` may be an (h, w, 3) array: a per-pixel colour (gradient)."""
    im = np.asarray(img.convert("RGBA")).astype(np.float64)
    rgb, a = im[..., :3], im[..., 3:4]
    u, v = weights(rgb)
    new = bg + u * (line - bg) + v * (track - bg)
    return Image.fromarray(np.concatenate([np.clip(new, 0, 255), a], -1).astype(np.uint8), "RGBA")


def sunset(h, w):
    """Diagonal pink (#FF4FA3, top-left) -> orange (#FF8A4C, bottom-right) gradient."""
    y, x = np.mgrid[0:h, 0:w]
    t = ((x / max(1, w - 1)) + (y / max(1, h - 1))) / 2
    pink, orange = np.array([255, 79, 163.]), np.array([255, 138, 76.])
    return pink + t[..., None] * (orange - pink)


THEMES = {
    "icon-light.ico": lambda h, w: (np.array([234, 227, 209.]), np.array([169, 114, 46.]), np.array([214, 203, 178.])),
    "icon-gta.ico": lambda h, w: (np.array([26, 15, 46.]), sunset(h, w), np.array([58, 33, 86.])),
}

ico = Image.open(SRC)
sizes = sorted(ico.info["sizes"])
frames = []
for sz in sizes:
    ico.size = sz
    ico.load()
    frames.append(ico.convert("RGBA").copy())
big = max(range(len(sizes)), key=lambda i: sizes[i][0])
rows = [frames]
for name, palette in THEMES.items():
    out = [recolour(f, *palette(f.height, f.width)) for f in frames]
    path = os.path.join(OUT, name)
    out[big].save(path, sizes=sizes, append_images=[im for i, im in enumerate(out) if i != big])
    assert sorted(Image.open(path).info["sizes"]) == sizes, name
    rows.append(out)
    print("wrote", os.path.normpath(path), [s[0] for s in sizes])

strip = Image.new("RGBA", (sum(s[0] + 8 for s in sizes) + 4, len(rows) * 264), (128, 128, 128, 255))
for r, row in enumerate(rows):
    x = 4
    for im, sz in zip(row, sizes):
        strip.paste(im, (x, 4 + r * 264), im)
        x += sz[0] + 8
strip.save(os.path.join(OUT, "..", "..", "..", "target", "theme-icons-preview.png"))
