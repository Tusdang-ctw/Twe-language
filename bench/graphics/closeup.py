"""web3d-M7 follow-up: look closely at one scene.

    python closeup.py <scenario> [x0 y0 x1 y1]

Writes out/closeup/<scenario>.png: the reference, Twe and Three.js (top
row) and the ꟻLIP error maps of Twe and Three.js (bottom row), cropped
to the box if given (in reference pixels), and prints mean colour and
error per image over the box, so a bias (too bright, too blue) shows as
a number.
"""

import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image

HERE = Path(__file__).parent
SUITE = json.loads((HERE / "suite.json").read_text())
CACHE, OUT = HERE / "cache", HERE / "out"


def load(path):
    img = Image.open(path).convert("RGBA")
    a = np.asarray(img, dtype=np.float32) / 255.0
    return a[:, :, :3] * a[:, :, 3:4]


def main():
    name = sys.argv[1]
    scene = json.loads((CACHE / name / "scene.json").read_text())
    ref = load(CACHE / name / scene["goldens"][SUITE["reference"]])
    h, w = ref.shape[:2]
    box = [int(v) for v in sys.argv[2:6]] if len(sys.argv) >= 6 else [0, 0, w, h]
    x0, y0, x1, y1 = box
    imgs = {
        "ref": ref,
        "twe": load(OUT / "twe" / f"{name}.png"),
        "three": load(OUT / "three" / f"{name}.png"),
    }
    errs = {k: np.asarray(Image.open(OUT / "flip" / f"{name}-{k}.png"), dtype=np.float32) / 255.0 for k in ("twe", "three")}
    for k, a in imgs.items():
        c = a[y0:y1, x0:x1].reshape(-1, 3).mean(0)
        e = f"  flip {errs[k][y0:y1, x0:x1].mean():.4f}" if k in errs else ""
        print(f"{k:6} mean rgb {c[0]:.3f} {c[1]:.3f} {c[2]:.3f}{e}")
    crop = lambda a: Image.fromarray((np.clip(a[y0:y1, x0:x1], 0, 1) * 255).astype(np.uint8))
    tiles = [crop(imgs["ref"]), crop(imgs["twe"]), crop(imgs["three"])]
    maps = [Image.new("RGB", tiles[0].size)] + [
        crop(np.repeat(errs[k][:, :, None], 3, axis=2) if errs[k].ndim == 2 else errs[k][:, :, :3]) for k in ("twe", "three")
    ]
    tw, th = tiles[0].size
    sheet = Image.new("RGB", (tw * 3, th * 2))
    for i, t in enumerate(tiles + maps):
        sheet.paste(t.convert("RGB"), ((i % 3) * tw, (i // 3) * th))
    scale = min(1.0, 1500 / sheet.size[0])
    sheet = sheet.resize((int(sheet.size[0] * scale), int(sheet.size[1] * scale)))
    (OUT / "closeup").mkdir(parents=True, exist_ok=True)
    sheet.save(OUT / "closeup" / f"{name}.png")
    print(f"wrote out/closeup/{name}.png")


if __name__ == "__main__":
    main()
