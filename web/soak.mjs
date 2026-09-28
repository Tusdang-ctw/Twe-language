// web3d-M4: the wasm soak, run under Node. The same scripted player
// as `tests/soak.rs` (`twec::soak`), in the WebAssembly build of the
// interpreter, over a game bundle `twec build --target web` wrote.
//
//   cargo build -p twe-web --target wasm32-unknown-unknown --release
//   wasm-bindgen --target nodejs --no-typescript --out-dir target/web-bench \
//       target/wasm32-unknown-unknown/release/twe_web.wasm
//   twec build --target web --out target/s3web examples/survive3d
//   node web/soak.mjs target/s3web [minutes=10] [runs=10] [--stress]
//
// Each run is a fresh game played for `minutes` of simulated time. Exits
// non-zero on the first runtime error.
import { createRequire } from "node:module";
import { mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

const require = createRequire(import.meta.url);
const pkg = process.env.TWE_BENCH_PKG ?? resolve("target/web-bench/twe_web.js");

// The same `env` stubs the page maps with an import map (see bench.mjs).
const envDir = join(dirname(pkg), "node_modules", "env");
mkdirSync(envDir, { recursive: true });
const stubs = readFileSync(new URL("./env.js", import.meta.url), "utf8").replace(
  /^export function (\w+)/gm,
  "exports.$1 = function $1",
);
writeFileSync(join(envDir, "index.js"), stubs);

const { soak } = require(pkg);

const args = process.argv.slice(2);
const stress = args.includes("--stress");
const [dir, minutesArg, runsArg] = args.filter((a) => a !== "--stress");
if (!dir) {
  console.error("usage: node web/soak.mjs <web-build-dir> [minutes] [runs] [--stress]");
  process.exit(2);
}
const bundleName = readdirSync(dir).find((f) => /^game\.[0-9a-f]{12}\.twebundle$/.test(f));
if (!bundleName) {
  console.error(`no game.<hash>.twebundle in ${dir}`);
  process.exit(2);
}
const bundle = readFileSync(join(dir, bundleName));
const minutes = Number(minutesArg ?? 10);
const runs = Number(runsArg ?? 10);
const ticks = Math.round(minutes * 60 * 60);

for (let i = 1; i <= runs; i++) {
  const t0 = performance.now();
  let report;
  try {
    report = JSON.parse(soak(bundle, ticks, i, stress));
  } catch (e) {
    console.error(`run ${i}: FAILED: ${e}`);
    process.exit(1);
  }
  const secs = ((performance.now() - t0) / 1000).toFixed(1);
  console.log(`run ${i}/${runs}: ${minutes} min in ${secs} s ${JSON.stringify(report)}`);
}
console.log(`ok: ${runs} run(s) of ${minutes} min${stress ? " under GC stress" : ""}`);
