"""web3d-M7 follow-up: where Twe loses to Three.js in a scene.

    python worst.py <scenario> [block]

Splits the frame into blocks, ranks them by how much worse Twe's ꟻLIP
error is than Three.js's there, and prints the top ten with the mean
colour of the reference, Twe and Three.js in each (run score.py first).
"""

import json
import sys

import numpy as np
from PIL import Image

name = sys.argv[1]
B = int(sys.argv[2]) if len(sys.argv) > 2 else 96


def load(p):
    a = np.asarray(Image.open(p).convert("RGBA"), dtype=np.float32) / 255
    return a[:, :, :3] * a[:, :, 3:4]


def err(p):
    e = np.asarray(Image.open(p), dtype=np.float32) / 255
    return e.mean(2) if e.ndim == 3 else e


scene = json.load(open(f"cache/{name}/scene.json"))
ref = load(f"cache/{name}/{scene['goldens']['blender-cycles']}")
twe, three = load(f"out/twe/{name}.png"), load(f"out/three/{name}.png")
et, eh = err(f"out/flip/{name}-twe.png"), err(f"out/flip/{name}-three.png")
h, w = et.shape
rows = []
for y in range(0, h, B):
    for x in range(0, w, B):
        a, b = et[y:y + B, x:x + B].mean(), eh[y:y + B, x:x + B].mean()
        rows.append((a - b, a, b, x, y))
rows.sort(reverse=True)
mean = lambda img, x, y: img[y:y + B, x:x + B].reshape(-1, 3).mean(0).round(3)
for d, a, b, x, y in rows[:10]:
    print(f"({x},{y}) twe {a:.3f} three {b:.3f} | ref {mean(ref, x, y)} twe {mean(twe, x, y)} three {mean(three, x, y)}")
print(f"Twe minus Three.js, mean over blocks: {sum(r[0] for r in rows) / len(rows):+.4f}")
