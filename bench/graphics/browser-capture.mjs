// web3d-M8: capture a deterministic browser frame of a `twec build
// --target web` game, so "it runs in the browser" is shown, not
// asserted. Diffing against a native reference is a separate step (any
// image tool) to avoid adding an npm image dependency.
//
//   node web/browser-capture.mjs <dist-dir> --capture N --shot out.png
//        [--browser chrome|edge|firefox|webkit] [--headless]
//
// It serves <dist-dir> (a build output), opens `index.html?capture=N`
// (the shell renders N deterministic ticks with no input, then sets
// `window.tweCaptured`), and screenshots the `#twe-canvas`. Exit 3 when
// the browser has no WebGPU (the game shows a "needs WebGPU" message).
//
// Requires playwright-core + the named browser installed (Chrome/Edge
// are channels of the system install; firefox/webkit are Playwright's).

import { createServer } from "node:http";
import { existsSync, readFileSync, statSync } from "node:fs";
import { extname, join, normalize, resolve } from "node:path";
import { chromium, firefox, webkit } from "playwright-core";

const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : fallback;
};
const has = (name) => args.includes(`--${name}`);
const dir = args.find((a, i) => !a.startsWith("--") && !args[i - 1]?.startsWith("--"));
if (!dir) {
  console.error("usage: node web/browser-capture.mjs <dist-dir> --capture N --shot out.png [--browser chrome|edge|firefox|webkit] [--headless]");
  process.exit(2);
}
const capture = Number(opt("capture", 60));
const shot = opt("shot", "browser-frame.png");
const browserName = opt("browser", "chrome");
const headless = has("headless");

const TYPES = {
  ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript",
  ".json": "application/json", ".wasm": "application/wasm", ".png": "image/png",
  ".glb": "model/gltf-binary", ".hdr": "image/vnd.radiance", ".cube": "text/plain",
  ".twebundle": "application/octet-stream",
};

const root = resolve(dir);
const server = createServer((req, res) => {
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
const url = `http://127.0.0.1:${server.address().port}/index.html?capture=${capture}`;

// Chrome needs these to expose WebGPU off the main compositor path.
const chromeFlags = ["--enable-unsafe-webgpu", "--use-angle=d3d11", "--enable-gpu"];

let browser;
if (browserName === "chrome" || browserName === "edge") {
  browser = await chromium.launch({
    channel: browserName === "edge" ? "msedge" : "chrome",
    headless,
    args: chromeFlags,
  });
} else if (browserName === "firefox") {
  browser = await firefox.launch({
    headless,
    firefoxUserPrefs: { "dom.webgpu.enabled": true, "gfx.webgpu.force-enabled": true },
  });
} else if (browserName === "webkit") {
  browser = await webkit.launch({ headless });
} else {
  console.error(`unknown --browser ${browserName}`);
  process.exit(2);
}

let code = 0;
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 960 } });
  page.on("console", (m) => { if (m.type() === "error") console.error(`[page] ${m.text()}`); });
  await page.goto(url, { waitUntil: "load" });
  // WebGPU needs a secure context — check on the served (localhost) page.
  const gpu = await page.evaluate(async () => "gpu" in navigator && !!(await navigator.gpu.requestAdapter()));
  if (!gpu) {
    console.error(`${browserName}: no WebGPU adapter — the game shows its 'needs WebGPU' message.`);
    code = 3;
  } else {
    await page.waitForFunction(() => window.tweCaptured === true, { timeout: 30000 });
    await page.locator("#twe-canvas").screenshot({ path: shot });
    console.log(`${browserName}: captured frame ${capture} → ${shot}`);
  }
} catch (e) {
  console.error(`${browserName}: ${e.message ?? e}`);
  code = 1;
} finally {
  await browser.close();
  server.close();
}
process.exit(code);
