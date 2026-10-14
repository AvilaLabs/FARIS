// Load the browser build and check it starts: the page draws a frame, the
// recorded study is open, the first-open card shows, nothing panics. Serve the
// built dist (crates/faris-app/dist) first, e.g.
//   python3 -m http.server --bind 127.0.0.1 8080 --directory crates/faris-app/dist
// WEB_URL selects another address; WEB_SCREENSHOT the picture's path
// (default web-smoke.png); PLAYWRIGHT_MODULE a playwright index.mjs to use;
// CHROMIUM_PATH a browser executable to use instead of playwright's own.
// WEB_LONG_TASK_MS is the longest single main-thread task allowed (default 4000,
// measured on headless Chromium with software GL: the longest is about 2.7 s,
// the app's first frame; before the work was split up one task took 19.8 s).
// The account check (api.avilalabs.org) is refused by the browser when the page
// is not served from an Avila Labs address; that one error is expected locally.
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
  executablePath: process.env.CHROMIUM_PATH || undefined,
  // Headless Chromium has no GPU: the software GL path stands in for a user's.
  args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"],
});
try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const problems = [];
  page.on("pageerror", error => problems.push(`page error: ${error.message}`));
  page.on("console", message => {
    const text = message.text();
    // The signed-out account check answers 401 (and fails CORS when served locally); both are expected.
    const source = (message.location() && message.location().url) || "";
    const expected = /api\.avilalabs\.org|net::ERR_FAILED/.test(text) || /api\.avilalabs\.org/.test(source);
    if (!expected && (message.type() === "error" || /panicked/.test(text))) problems.push(`console ${message.type()}: ${text}`);
  });
  // Long main-thread tasks, as the page itself sees them.
  await page.addInitScript(() => {
    window.__longTasks = [];
    new PerformanceObserver(list => {
      for (const entry of list.getEntries()) window.__longTasks.push(Math.round(entry.duration));
    }).observe({ entryTypes: ["longtask"] });
  });
  await page.goto(url.href);
  await page.locator("#faris_canvas").waitFor({ state: "attached" });
  // The loading text goes away once the first frame is up.
  await page.locator("#faris-loading").waitFor({ state: "detached", timeout: Number(process.env.WEB_TIMEOUT_MS || 180000) });
  // No mouse, keyboard or resize events from here to the end of the settle
  // wait: every pending calculation must advance on its own.
  await page.waitForFunction(() => {
    const s = window.__farisStatus;
    return s && s.frames > 3 && !s.historyPending && s.sweepReady;
  }, null, { timeout: 120000, polling: 500 });
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
  const longTasks = await page.evaluate(() => window.__longTasks);
  const longest = Math.max(0, ...longTasks);
  const bound = Number(process.env.WEB_LONG_TASK_MS || 4000);
  assert.ok(longest < bound, `the longest main-thread task was ${longest} ms (bound ${bound} ms): ${longTasks.join(", ")}`);
  // Dragging the 3D view keeps the page drawing. Close the first-open card
  // first, then drag for five seconds and count the frames.
  await page.keyboard.press("Escape");
  const before = (await page.evaluate(() => window.__farisStatus)).frames;
  const cx = box.x + box.width * 0.5, cy = box.y + box.height * 0.45;
  await page.mouse.move(cx, cy);
  await page.mouse.down();
  for (let step = 0; step < 50; step++) {
    await page.mouse.move(cx + 120 * Math.sin(step / 6), cy + 40 * Math.cos(step / 6));
    await page.waitForTimeout(100);
  }
  await page.mouse.up();
  const after = await page.evaluate(() => window.__farisStatus);
  assert.ok(after.frames - before > 25, `frames kept advancing while dragging: ${before} to ${after.frames}`);
  // Nothing may be left waiting on input afterwards.
  assert.equal(after.historyPending, false, "no history left pending after the drag");
  assert.equal(after.sweepPending, false, "no sweep work left pending after the drag");
  const finalTasks = await page.evaluate(() => window.__longTasks);
  assert.ok(Math.max(0, ...finalTasks) < bound, `a long task during the drag: ${finalTasks.join(", ")}`);
  assert.deepEqual(problems, [], "no console errors or panics");
  console.log(`FARIS browser build starts: ${JSON.stringify(status)}; longest task ${longest} ms`);
} finally {
  await browser.close();
}
