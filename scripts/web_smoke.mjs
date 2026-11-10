// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs
// Load the browser build and check it starts: the page draws a frame, the
// recorded study is open, the first-open card shows, nothing panics. Serve the
// built dist (crates/faris-app/dist) first, e.g.
//   python3 -m http.server --bind 127.0.0.1 8080 --directory crates/faris-app/dist
// WEB_URL selects another address; WEB_SCREENSHOT the picture's path
// (default web-smoke.png); PLAYWRIGHT_MODULE a playwright index.mjs to use;
// CHROMIUM_PATH a browser executable to use instead of playwright's own.
// WEB_EXPECT_TRANSPORT=withheld checks legacy replay refusal; WEB_SYNTHETIC_FIXTURE=1
// displays an explicit test-data banner for the isolated current-schema fixture.
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
const expectWithheld = process.env.WEB_EXPECT_TRANSPORT === "withheld";
const syntheticFixture = process.env.WEB_SYNTHETIC_FIXTURE === "1";
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
  // A panic stops the app for good: report it at once instead of waiting out the load timeout.
  let panicked;
  const panic = new Promise((_, reject) => { panicked = reject; });
  panic.catch(() => {});
  const note = problem => {
    const expectedLegacyRefusal = /(?:lost-particle count is NOT_EVALUATED|the recorded run is missing batch_statistics\.py required by its worker receipt)/;
    if (expectWithheld && expectedLegacyRefusal.test(problem)) {
      console.log(`Expected legacy replay refusal: ${problem}`);
      return;
    }
    problems.push(problem);
    console.error(problem);
    if (/panicked|RuntimeError: unreachable/.test(problem)) panicked(new Error(`the app stopped: ${problem}`));
  };
  page.on("pageerror", error => note(`page error: ${error.message}`));
  page.on("console", message => {
    const text = message.text();
    // The signed-out account check answers 401 (and fails CORS when served locally); both are expected.
    const source = (message.location() && message.location().url) || "";
    const expected = /api\.avilalabs\.org|net::ERR_FAILED/.test(text) || /api\.avilalabs\.org/.test(source);
    if (!expected && (message.type() === "error" || /panicked/.test(text))) note(`console ${message.type()}: ${text}`);
  });
  // Long main-thread tasks, as the page itself sees them.
  await page.addInitScript(() => {
    window.__longTasks = [];
    new PerformanceObserver(list => {
      for (const entry of list.getEntries()) window.__longTasks.push(Math.round(entry.duration));
    }).observe({ entryTypes: ["longtask"] });
  });
  if (syntheticFixture) {
    await page.addInitScript(() => {
      document.addEventListener("DOMContentLoaded", () => {
        const banner = document.createElement("div");
        banner.id = "faris-synthetic-smoke-label";
        banner.textContent = "SYNTHETIC CI SMOKE FIXTURE — placeholder values; NOT scientific transport results";
        Object.assign(banner.style, {
          position: "fixed", top: "0", left: "0", right: "0", zIndex: "2147483647",
          padding: "8px 12px", background: "#7b2300", color: "white",
          font: "bold 14px sans-serif", textAlign: "center",
        });
        document.body.append(banner);
      }, { once: true });
    });
  }
  await page.goto(url.href);
  await page.locator("#faris_canvas").waitFor({ state: "attached" });
  // A legacy record can refuse boot before the canvas app exists (missing
  // bound helper) or boot with its numerical values withheld (missing exact
  // loss receipt). The production test accepts only those named outcomes.
  const bootOutcome = await Promise.race([
    page.waitForFunction(() => {
      const loading = document.getElementById("faris-loading");
      if (!loading) return "started";
      return loading.textContent?.startsWith("FARIS could not start:") ? "refused" : false;
    }, null, { timeout: Number(process.env.WEB_TIMEOUT_MS || 180000) })
      .then(handle => handle.jsonValue()),
    panic,
  ]);
  if (bootOutcome === "refused") {
    const message = await page.locator("#faris-loading").innerText();
    assert.ok(expectWithheld, `the positive browser fixture failed during boot: ${message}`);
    assert.match(message,
      /FARIS could not start: (?:.*lost-particle count is NOT_EVALUATED|.*the recorded run is missing batch_statistics\.py required by its worker receipt)/,
      `the legacy boot refusal names an exact current replay requirement: ${message}`);
    await mkdir(dirname(shot) || ".", { recursive: true });
    const picture = await page.screenshot({ path: shot });
    assert.ok(picture.length > 1000, "the legacy refusal screen is visible");
    assert.deepEqual(problems, [], "no unexpected console errors or panics");
    console.log(`FARIS legacy browser refusal: ${message}`);
  } else {
    // No mouse, keyboard or resize events until the page is settled.
    await page.waitForFunction(() => {
      const s = window.__farisStatus;
      return s && s.frames > 3 && !s.historyPending && s.sweepReady;
    }, null, { timeout: 120000, polling: 500 });
    const status = await page.evaluate(() => window.__farisStatus);
    const box = await page.locator("#faris_canvas").boundingBox();
    assert.ok(box && box.width > 400 && box.height > 300, "the canvas has a usable size");
    assert.equal(status.hasResults, !expectWithheld,
      `${expectWithheld ? "legacy transport values remain withheld" : "the recorded transport is loaded"}; browser status: ${JSON.stringify(status)}`);
    if (expectWithheld) {
      assert.ok(status.transportRefusals?.some(item =>
        /lost-particle count is NOT_EVALUATED/.test(item.reason)),
      `the legacy run refusal names its missing exact loss receipt; browser status: ${JSON.stringify(status)}`);
    }
    if (syntheticFixture) {
      await page.locator("#faris-synthetic-smoke-label").waitFor({ state: "visible" });
      assert.match(await page.locator("#faris-synthetic-smoke-label").innerText(), /NOT scientific transport results/);
    }
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
  }
} finally {
  await browser.close();
}
