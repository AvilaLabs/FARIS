// Load the browser build and check it starts: the page draws a frame, the
// recorded study is open, the first-open card shows, nothing panics. Serve the
// built dist (crates/faris-app/dist) first, e.g.
//   python3 -m http.server --bind 127.0.0.1 8080 --directory crates/faris-app/dist
// WEB_URL selects another address; WEB_SCREENSHOT the picture's path
// (default web-smoke.png); PLAYWRIGHT_MODULE a playwright index.mjs to use.
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { dirname } from "node:path";

const { chromium } = process.env.PLAYWRIGHT_MODULE
  ? await import(process.env.PLAYWRIGHT_MODULE)
  : await import("playwright");
const base = process.env.WEB_URL || "http://127.0.0.1:8080/";
const shot = process.env.WEB_SCREENSHOT || "web-smoke.png";
const url = new URL(base);
url.searchParams.set("verify", "1");

for (let attempt = 0; ; attempt++) {
  try { if ((await fetch(base)).ok) break; } catch { /* server starting */ }
  if (attempt === 49) throw new Error(`The page did not become ready: ${base}`);
  await new Promise(resolve => setTimeout(resolve, 100));
}

const browser = await chromium.launch({
  headless: true,
  // Headless Chromium has no GPU: the software GL path stands in for a user's.
  args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"],
});
try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const problems = [];
  page.on("pageerror", error => problems.push(`page error: ${error.message}`));
  page.on("console", message => {
    const text = message.text();
    if (message.type() === "error" || /panicked/.test(text)) problems.push(`console ${message.type()}: ${text}`);
  });
  await page.goto(url.href);
  await page.locator("#faris_canvas").waitFor({ state: "attached" });
  // The loading text goes away once the first frame is up.
  await page.locator("#faris-loading").waitFor({ state: "detached", timeout: Number(process.env.WEB_TIMEOUT_MS || 180000) });
  await page.waitForFunction(() => window.__farisStatus && window.__farisStatus.frames > 3, null, { timeout: 120000 });
  // Histories compute in the page; wait for them to settle.
  await page.waitForFunction(() => window.__farisStatus && !window.__farisStatus.historyPending, null, { timeout: 180000 });
  const status = await page.evaluate(() => window.__farisStatus);
  const box = await page.locator("#faris_canvas").boundingBox();
  assert.ok(box && box.width > 400 && box.height > 300, "the canvas has a usable size");
  assert.equal(status.hasResults, true, "the recorded transport is loaded");
  assert.equal(status.pairedLoaded, true, "the no-port arrangement is loaded");
  assert.equal(status.historyAssumptions, true, "the operating history assumptions are loaded");
  assert.equal(status.noticeOpen, true, "the first-open card is shown");
  await mkdir(dirname(shot) || ".", { recursive: true });
  const picture = await page.screenshot({ path: shot });
  assert.ok(picture.length > 30000, "the picture is not blank");
  assert.deepEqual(problems, [], "no console errors or panics");
  console.log(`FARIS browser build starts: ${JSON.stringify(status)}`);
} finally {
  await browser.close();
}
