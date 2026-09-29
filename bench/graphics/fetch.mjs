// web3d-M7: fetch the graphics comparison suite into cache/.
//
// For each scenario in suite.json, at the pinned commits:
//   - resolve the scenario (camera orbit, field of view, size,
//     environment) from the Khronos Render Fidelity generator's
//     config, applying its defaults (src/config-reader.ts);
//   - download the model from glTF-Sample-Assets; multi-file .gltf
//     models are packed into one .glb (gltf-pipeline) so Twe and
//     Three.js load byte-identical input;
//   - download the environment .hdr and the goldens: the path-traced
//     reference and the other engines' renders listed in suite.json.
//
// Writes cache/<scenario>/{model.glb, scene.json, goldens/<renderer>.png}
// and cache/env/<file>.hdr. Files already present are kept.
//
//   node fetch.mjs            (from bench/graphics/)

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import gltfPipeline from "gltf-pipeline";

const here = dirname(fileURLToPath(import.meta.url));
const suite = JSON.parse(readFileSync(join(here, "suite.json"), "utf8"));
const cache = join(here, "cache");
const gen = suite.generator;
const assets = suite.sampleAssets;

// The generator's defaults (src/config-reader.ts at the pinned commit).
const DEFAULTS = {
  lighting: "../../../environments/lightroom_14b.hdr",
  dimensions: { width: 768, height: 768 },
  target: { x: 0, y: 0, z: 0 },
  orbit: { theta: 0, phi: 90, radius: 1 },
  verticalFoV: 45,
  renderSkybox: false,
};

async function get(url, { optional = false } = {}) {
  const res = await fetch(url, { headers: { "User-Agent": "twe-graphics-bench" } });
  if (res.status === 404 && optional) return null;
  if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
  return Buffer.from(await res.arrayBuffer());
}

// A file in a repo at a commit; follows Git LFS pointers.
async function repoFile(repo, commit, path, opts) {
  const raw = await get(`https://raw.githubusercontent.com/${repo}/${commit}/${path}`, opts);
  if (raw && raw.subarray(0, 40).toString().startsWith("version https://git-lfs")) {
    return get(`https://media.githubusercontent.com/media/${repo}/${commit}/${path}`, opts);
  }
  return raw;
}

// Every file under a directory of a repo at a commit (GitHub contents API).
async function listDir(repo, commit, dir) {
  const res = await fetch(`https://api.github.com/repos/${repo}/contents/${dir}?ref=${commit}`, {
    headers: { "User-Agent": "twe-graphics-bench", Accept: "application/vnd.github+json" },
  });
  if (!res.ok) throw new Error(`list ${dir}: HTTP ${res.status}`);
  const out = [];
  for (const entry of await res.json()) {
    if (entry.type === "dir") out.push(...(await listDir(repo, commit, entry.path)));
    else out.push(entry.path);
  }
  return out;
}

async function saveOnce(path, produce) {
  if (existsSync(path)) return false;
  mkdirSync(dirname(path), { recursive: true });
  const bytes = await produce();
  if (bytes == null) return false;
  writeFileSync(path, bytes);
  return true;
}

// "../../../glTF-Sample-Assets/Models/X/..." -> "Models/X/..."
const assetPath = (p) => p.replace(/^(\.\.\/)+glTF-Sample-Assets\//, "");
// "../../../environments/x.hdr" -> "environments/x.hdr"
const generatorPath = (p) => p.replace(/^(\.\.\/)+/, "");

function pngSize(bytes) {
  return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
}

const config = JSON.parse(
  (await repoFile(gen.repo, gen.commit, "test/config.json")).toString("utf8"),
);

for (const name of suite.scenarios) {
  const raw = config.scenarios.find((s) => s.name === name);
  if (!raw) throw new Error(`${name} is not in the generator config`);
  const s = {
    ...DEFAULTS,
    ...raw,
    dimensions: { ...DEFAULTS.dimensions, ...raw.dimensions },
    target: { ...DEFAULTS.target, ...raw.target },
    orbit: { ...DEFAULTS.orbit, ...raw.orbit },
  };
  const dir = join(cache, name);
  mkdirSync(join(dir, "goldens"), { recursive: true });

  // Model.
  const model = assetPath(s.model);
  const fetched = await saveOnce(join(dir, "model.glb"), async () => {
    if (model.endsWith(".glb")) return repoFile(assets.repo, assets.commit, model);
    const folder = model.slice(0, model.lastIndexOf("/"));
    const src = join(dir, "src");
    for (const file of await listDir(assets.repo, assets.commit, folder)) {
      const local = join(src, file.slice(folder.length + 1));
      await saveOnce(local, () => repoFile(assets.repo, assets.commit, file));
    }
    const gltf = JSON.parse(readFileSync(join(src, model.slice(folder.length + 1)), "utf8"));
    // keepUnusedElements: gltf-pipeline's cleanup can't see textures
    // referenced only from material extensions (anisotropy, sheen,
    // clearcoat …) and would drop them.
    const { glb } = await gltfPipeline.gltfToGlb(gltf, {
      resourceDirectory: src,
      keepUnusedElements: true,
    });
    return glb;
  });

  // Environment.
  const envFile = s.lighting.split("/").pop();
  await saveOnce(join(cache, "env", envFile), () =>
    repoFile(gen.repo, gen.commit, generatorPath(s.lighting)),
  );

  // Goldens: the reference, then the other engines that rendered it.
  const goldens = {};
  for (const renderer of [suite.reference, ...suite.alsoScored]) {
    const path = join(dir, "goldens", `${renderer}.png`);
    await saveOnce(path, () =>
      repoFile(gen.repo, gen.commit, `test/goldens/${name}/${renderer}-golden.png`, {
        optional: true,
      }),
    );
    if (existsSync(path)) goldens[renderer] = `goldens/${renderer}.png`;
  }
  if (!goldens[suite.reference]) throw new Error(`${name}: no ${suite.reference} golden`);

  // Render at the reference's pixel size (the goldens are 2x the
  // scenario's CSS size).
  const pixels = pngSize(readFileSync(join(dir, goldens[suite.reference])));
  const scene = {
    name,
    model: "model.glb",
    environment: `../env/${envFile}`,
    pixels,
    target: s.target,
    orbit: s.orbit,
    verticalFoV: s.verticalFoV,
    renderSkybox: s.renderSkybox,
    goldens,
  };
  writeFileSync(join(dir, "scene.json"), JSON.stringify(scene, null, 2));
  console.log(
    `${name}: ${pixels.width}x${pixels.height}, ${Object.keys(goldens).join(" ")}${fetched ? " (model fetched)" : ""}`,
  );
}
