"""Writes grid_plane.glb: a flat unit square (x and z from -0.5 to 0.5,
facing +Y) cut into 64 x 64 cells, with normals and UVs and no
material, for `visual` materials that displace vertices
(examples/procedural_materials_3d.twe). Run from this directory:
`python grid_plane.py`."""

import json
import struct

N = 64
positions, normals, uvs, indices = [], [], [], []
for j in range(N + 1):
    for i in range(N + 1):
        u, v = i / N, j / N
        positions += [u - 0.5, 0.0, v - 0.5]
        normals += [0.0, 1.0, 0.0]
        uvs += [u, v]
for j in range(N):
    for i in range(N):
        a = j * (N + 1) + i
        b, c, d = a + 1, a + N + 1, a + N + 2
        # Counter-clockwise seen from +Y.
        indices += [a, c, b, b, c, d]

count = (N + 1) * (N + 1)
pos = struct.pack(f"<{len(positions)}f", *positions)
nrm = struct.pack(f"<{len(normals)}f", *normals)
tex = struct.pack(f"<{len(uvs)}f", *uvs)
idx = struct.pack(f"<{len(indices)}H", *indices)
bin_chunk = pos + nrm + tex + idx
offsets = [0, len(pos), len(pos) + len(nrm), len(pos) + len(nrm) + len(tex)]
gltf = {
    "asset": {"version": "2.0", "generator": "twe grid_plane.py"},
    "scene": 0,
    "scenes": [{"nodes": [0]}],
    "nodes": [{"mesh": 0}],
    "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2}, "indices": 3}]}],
    "buffers": [{"byteLength": len(bin_chunk)}],
    "bufferViews": [
        {"buffer": 0, "byteOffset": offsets[0], "byteLength": len(pos), "target": 34962},
        {"buffer": 0, "byteOffset": offsets[1], "byteLength": len(nrm), "target": 34962},
        {"buffer": 0, "byteOffset": offsets[2], "byteLength": len(tex), "target": 34962},
        {"buffer": 0, "byteOffset": offsets[3], "byteLength": len(idx), "target": 34963},
    ],
    "accessors": [
        {"bufferView": 0, "componentType": 5126, "count": count, "type": "VEC3",
         "min": [-0.5, 0.0, -0.5], "max": [0.5, 0.0, 0.5]},
        {"bufferView": 1, "componentType": 5126, "count": count, "type": "VEC3"},
        {"bufferView": 2, "componentType": 5126, "count": count, "type": "VEC2"},
        {"bufferView": 3, "componentType": 5123, "count": len(indices), "type": "SCALAR"},
    ],
}
js = json.dumps(gltf, separators=(",", ":")).encode()
js += b" " * (-len(js) % 4)
bin_chunk += b"\0" * (-len(bin_chunk) % 4)
total = 12 + 8 + len(js) + 8 + len(bin_chunk)
with open("grid_plane.glb", "wb") as f:
    f.write(struct.pack("<4sII", b"glTF", 2, total))
    f.write(struct.pack("<I4s", len(js), b"JSON") + js)
    f.write(struct.pack("<I4s", len(bin_chunk), b"BIN\0") + bin_chunk)
