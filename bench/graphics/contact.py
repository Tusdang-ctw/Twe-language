"""web3d-M7 session 17: the comparison images, as one contact sheet.

For each scenario: the Blender Cycles reference, Twe and Three.js side by
side, each labelled with its FLIP score from results.json. Writes
comparison.jpg (committed with results.md).

    python contact.py            (from bench/graphics/, after score.py)
"""

import json
from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).parent
SUITE = json.loads((HERE / "suite.json").read_text())
CACHE, OUT = HERE / "cache", HERE / "out"
TILE = 256
LABEL = 22


def tile(path):
    img = Image.open(path).convert("RGBA")
    back = Image.new("RGBA", img.size, (0, 0, 0, 255))
    back.alpha_composite(img)
    return back.convert("RGB").resize((TILE, TILE), Image.LANCZOS)


def main():
    scores = json.loads((HERE / "results.json").read_text())
    names = [n for n in SUITE["scenarios"] if (CACHE / n / "scene.json").exists()]
    columns = [("Cycles (reference)", None), ("Twe", "twe"), ("Three.js r186", "three")]
    sheet = Image.new("RGB", (TILE * len(columns), (TILE + LABEL) * len(names)), (24, 24, 28))
    draw = ImageDraw.Draw(sheet)
    for row, name in enumerate(names):
        scene = json.loads((CACHE / name / "scene.json").read_text())
        paths = [
            CACHE / name / scene["goldens"][SUITE["reference"]],
            OUT / "twe" / f"{name}.png",
            OUT / "three" / f"{name}.png",
        ]
        y = row * (TILE + LABEL)
        for col, ((title, key), path) in enumerate(zip(columns, paths)):
            x = col * TILE
            if path.exists():
                sheet.paste(tile(path), (x, y + LABEL))
            text = name.removeprefix("khronos-") if col == 0 else title
            if key and key in scores.get(name, {}):
                text += f"  FLIP {scores[name][key]:.4f}"
            draw.text((x + 6, y + 5), text, fill=(230, 230, 230))
    sheet.save(HERE / "comparison.jpg", quality=85)
    print(f"wrote {HERE / 'comparison.jpg'}")


if __name__ == "__main__":
    main()
