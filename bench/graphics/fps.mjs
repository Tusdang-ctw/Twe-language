// web3d-M7 session 16: frame rate of a web page in Chrome.
//
//   node fps.mjs <dir-or-url> [--page path.html] [--gpu low|high] [--seconds N] [--warmup N]
//                [--size WxH] [--vsync] [--shot out.png]
//
// Serves <dir> (a `twec build --target web` output or any static page)
// and opens its index.html in the installed Chrome (playwright-core,
// channel "chrome"). Chrome's background throttles are off, so the
// numbers don't depend on the window being visible (an occluded window
// otherwise drops WebGPU pages to 1 fps). Without --vsync the frame
// rate isn't capped at the display's, so the result is throughput:
// frames per second and ms per frame, averaged over --seconds after
// --warmup. It also prints the GPU the page got.
//
// --gpu high asks Chrome for the discrete GPU on a machine with two
// (`--force_high_performance_gpu`); the default lets Chrome choose,
// which on this laptop is the integrated one.

import { createServer } from "node:http";
import { existsSync, readFileSync, statSync } from "node:fs";
import { extname, join, normalize, resolve } from "node:path";
import { chromium } from "playwright-core";

const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : fallback;
};
const target = args.find((a, i) => !a.startsWith("--") && !args[i - 1]?.startsWith("--"));
if (!target) {
  console.error("usage: node fps.mjs <dir-or-url> [--gpu low|high] [--seconds N] [--warmup N] [--size WxH] [--vsync] [--shot out.png]");
  process.exit(2);
}
const seconds = Number(opt("seconds", 10));
const warmup = Number(opt("warmup", 5));
const [width, height] = opt("size", "1280x720").split("x").map(Number);
const gpu = opt("gpu", "low");

const TYPES = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".json": "application/json",
  ".wasm": "application/wasm",
  ".glb": "model/gltf-binary",
};

let url = target;
let server;
if (!/^https?:/.test(target)) {
  const root = resolve(target);
  server = createServer((req, res) => {
    let path = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname));
    let file = join(root, path);
    if (existsSync(file) && statSync(file).isDirectory()) file = join(file, "index.html");
    if (!file.startsWith(root) || !existsSync(file)) {
      res.writeHead(404).end();
      return;
    }
    res.writeHead(200, { "Content-Type": TYPES[extname(file)] ?? "application/octet-stream" });
    res.end(readFileSync(file));
  });
  await new Promise((ok) => server.listen(0, "127.0.0.1", ok));
  url = `http://127.0.0.1:${server.address().port}/${opt("page", "")}`;
}

const flags = [
  "--enable-unsafe-webgpu",
  "--use-angle=d3d11",
  "--enable-gpu",
  "--ignore-gpu-blocklist",
  "--disable-renderer-backgrounding",
  "--disable-background-timer-throttling",
  "--disable-backgrounding-occluded-windows",
];
if (!args.includes("--vsync")) flags.push("--disable-gpu-vsync", "--disable-frame-rate-limit");
if (gpu === "high") flags.push("--force_high_performance_gpu");

const browser = await chromium.launch({ channel: "chrome", headless: false, args: flags });
try {
  const page = await browser.newPage({ viewport: { width, height } });
  const errors = [];
  page.on("console", (m) => {
    if (m.type() === "error" || m.type() === "warning") errors.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
  await page.goto(url);
  const result = await page.evaluate(
    async ({ warmup, seconds }) => {
      const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
      await sleep(warmup * 1000);
      // A Twe page: its runtime's own counters (see crates/twe-web).
      const script = [...document.scripts].map((s) => s.textContent).join("\n");
      const runtime = script.match(/twe_web\.[0-9a-f]+\.js/);
      const twe = runtime ? await import(`./${runtime[0]}`) : null;
      twe?.frame_stats();
      let frames = 0;
      let running = true;
      const tick = () => {
        frames += 1;
        if (running) requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
      const t0 = performance.now();
      await sleep(seconds * 1000);
      running = false;
      const elapsed = (performance.now() - t0) / 1000;
      const stats = twe ? Array.from(twe.frame_stats()) : null;
      const adapter = navigator.gpu ? await navigator.gpu.requestAdapter() : null;
      const info = adapter?.info ?? {};
      return {
        fps: frames / elapsed,
        msPerFrame: (elapsed * 1000) / frames,
        gpu: `${info.vendor ?? "?"} ${info.architecture ?? ""} ${info.description ?? ""}`.trim(),
        // [frames, ticks/frame, ms/tick, script render ms, kernel ms].
        stats,
      };
    },
    { warmup, seconds },
  );
  console.log(
    `${target}: ${result.fps.toFixed(1)} fps, ${result.msPerFrame.toFixed(2)} ms/frame ` +
      `(${width}x${height}, ${seconds}s after ${warmup}s, vsync ${args.includes("--vsync") ? "on" : "off"}, gpu ${result.gpu})`,
  );
  if (result.stats) {
    const [, ticks, tickMs, renderMs, kernelMs] = result.stats;
    console.log(
      `  per frame: ${ticks.toFixed(2)} ticks x ${tickMs.toFixed(2)} ms script, ` +
        `${renderMs.toFixed(2)} ms script render, ${kernelMs.toFixed(2)} ms kernel (CPU)`,
    );
  }
  if (errors.length) console.log(errors.slice(0, 10).join("\n"));
  const shot = opt("shot", null);
  if (shot) {
    await page
      .screenshot({ path: shot, timeout: 120000 })
      .catch((e) => console.log(`screenshot failed: ${e.message.split("\n")[0]}`));
  }
} finally {
  await browser.close();
  server?.close();
}
