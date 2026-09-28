// web3d-M3: the wasm script-tick benchmark, run under Node.
//
// The browser measurement (`frame_stats()` in a page) depends on the
// page being visible and on the display; this runs the same interpreter
// build headless, so the number is stable enough to track.
//
//   cargo build -p twe-web --target wasm32-unknown-unknown --release
//   wasm-bindgen --target nodejs --no-typescript --out-dir target/web-bench \
//       target/wasm32-unknown-unknown/release/twe_web.wasm
//   node web/bench.mjs examples/swarm_3d.twe [ticks]
//
// Prints the median and best tick in milliseconds (M3's exit target is
// a median of 4 ms or less for examples/swarm_3d.twe).
import { createRequire } from "node:module";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

const require = createRequire(import.meta.url);
const pkg = process.env.TWE_BENCH_PKG ?? resolve("target/web-bench/twe_web.js");

// The runtime imports macroquad's `env` functions (see web/env.js). The
// page maps them with an import map; here the same stubs become a
// CommonJS `env` package next to the runtime.
const envDir = join(dirname(pkg), "node_modules", "env");
mkdirSync(envDir, { recursive: true });
const stubs = readFileSync(new URL("./env.js", import.meta.url), "utf8").replace(
  /^export function (\w+)/gm,
  "exports.$1 = function $1",
);
writeFileSync(join(envDir, "index.js"), stubs);

const { bench_ticks } = require(pkg);

const [script, ticksArg] = process.argv.slice(2);
if (!script) {
  console.error("usage: node web/bench.mjs <script.twe> [ticks]");
  process.exit(2);
}
const ticks = Number(ticksArg ?? 300);
const times = Array.from(bench_ticks(readFileSync(script, "utf8"), ticks));
// The first ticks warm up the JIT and the heap; judge the rest.
const steady = times.slice(Math.min(30, Math.floor(ticks / 4))).sort((a, b) => a - b);
const median = steady[Math.floor(steady.length / 2)];
console.log(
  `${script}: median ${median.toFixed(2)} ms/tick, best ${steady[0].toFixed(2)} ms, ` +
    `p90 ${steady[Math.floor(steady.length * 0.9)].toFixed(2)} ms (${steady.length} ticks)`,
);
