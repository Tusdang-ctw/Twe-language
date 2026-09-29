// web3d-M7: the whole comparison in one command, from bench/graphics/:
//
//   npm install          (once: three, playwright-core, gltf-pipeline)
//   node run.mjs [scenario ...]
//
// fetch the suite → render with Three.js → render with Twe → score.
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..", "..");
const scenes = process.argv.slice(2);

function step(label, cmd, args, opts = {}) {
  console.log(`\n== ${label}`);
  const r = spawnSync(cmd, args, { stdio: "inherit", shell: process.platform === "win32", ...opts });
  if (r.status !== 0) {
    console.error(`${label} failed (exit ${r.status})`);
    process.exit(r.status ?? 1);
  }
}

step("fetch", "node", ["fetch.mjs"], { cwd: here });
step("three.js", "node", ["render-three.mjs", ...scenes], { cwd: here });
step(
  "twe",
  "cargo",
  ["test", "--release", "--test", "graphics_bench", "--", "--ignored", "--nocapture"],
  { cwd: repo, env: { ...process.env, TWE_BENCH_SCENES: scenes.join(",") } },
);
step("score", "python", ["score.py"], { cwd: here });
