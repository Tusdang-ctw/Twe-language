// web3d-M7: render every cached scenario with the pinned Three.js in
// headless Chrome, writing out/three/<scenario>.png.
//
//   node render-three.mjs [scenario ...]     (from bench/graphics/)
//
// Uses the Chrome already installed (playwright-core, channel "chrome");
// no browser download. The page is three/render.html, served from this
// folder by a small static server.

import { createServer } from "node:http";
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const here = dirname(fileURLToPath(import.meta.url));
const TYPES = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".json": "application/json",
  ".glb": "model/gltf-binary",
  ".hdr": "application/octet-stream",
};

const server = createServer((req, res) => {
  const path = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname));
  const file = join(here, path);
  if (!file.startsWith(here) || !existsSync(file)) {
    res.writeHead(404).end();
    return;
  }
  res.writeHead(200, { "Content-Type": TYPES[extname(file)] ?? "application/octet-stream" });
  res.end(readFileSync(file));
});
await new Promise((ok) => server.listen(0, "127.0.0.1", ok));
const origin = `http://127.0.0.1:${server.address().port}`;

const wanted = process.argv.slice(2);
const scenarios = readdirSync(join(here, "cache"))
  .filter((d) => existsSync(join(here, "cache", d, "scene.json")))
  .filter((d) => wanted.length === 0 || wanted.includes(d));

const browser = await chromium.launch({
  channel: "chrome",
  args: ["--use-angle=d3d11", "--enable-gpu", "--ignore-gpu-blocklist"],
});
const out = join(here, "out", "three");
mkdirSync(out, { recursive: true });
try {
  for (const name of scenarios) {
    const page = await browser.newPage();
    page.on("pageerror", (e) => console.error(`${name}: ${e.message}`));
    await page.goto(`${origin}/three/render.html`);
    await page.waitForFunction(() => typeof window.renderScene === "function");
    const t0 = performance.now();
    const url = await page.evaluate((n) => window.renderScene(n), name);
    const revision = await page.evaluate(() => window.threeRevision);
    writeFileSync(join(out, `${name}.png`), Buffer.from(url.split(",")[1], "base64"));
    console.log(`${name}: three r${revision}, ${((performance.now() - t0) / 1000).toFixed(1)} s`);
    await page.close();
  }
} finally {
  await browser.close();
  server.close();
}
