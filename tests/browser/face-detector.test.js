// The face detector only fails in ways a unit test cannot see: a vendored asset
// under the wrong name, a bundle that wants a DOM the worker does not have, a
// wasm body served without its content type. Every one of those produced the
// same symptom, "face detection is not running", and all of them need a real
// browser, a real worker and the real server to reproduce. So this test drives
// all three.
//
// Skipped, not failed, when Playwright or its Chromium are not installed: the
// suite has to stay runnable without an 800 MB download. `make check` installs
// neither, so treat a skip here as "unverified", not "fine".
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:net";
import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const require = createRequire(import.meta.url);
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

let chromium = null;
try {
  ({ chromium } = require("playwright"));
} catch {
  chromium = null;
}

/// Asked for rather than picked. A fixed port collides with a second copy of
/// this suite and with whatever else on the machine happened to want it.
///
/// The probe frees the port before the server binds it, so this is a hint and
/// not a reservation: two concurrent runs can be handed the same just-freed
/// number. `startServer` retries on that rather than pretending it cannot
/// happen, and a server that still will not come up fails the suite instead of
/// skipping, because a skip here reads as "nothing to test" when what happened
/// is "the test never ran".
async function freePort() {
  const probe = createServer();
  await new Promise((ready) => probe.listen(0, "127.0.0.1", ready));
  const { port } = probe.address();
  await new Promise((closed) => probe.close(closed));
  return port;
}

let PORT = 0;
let BASE = "";
const binary = resolve(root, "target/debug/codetrial");
let server = null;
let browser = null;
const configPath = resolve(root, "target/face-detector-test.env");

async function reachable() {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      const response = await fetch(BASE);
      if (response.ok) return true;
    } catch {
      // Not up yet.
    }
    await new Promise((done) => setTimeout(done, 200));
  }
  return false;
}

before(async () => {
  if (!chromium) return;
  try {
    browser = await chromium.launch();
  } catch {
    // Chromium is not downloaded; every test below skips.
    browser = null;
    return;
  }
  // An unbuilt checkout is the one honest reason to skip, so it is the only
  // one: everything past this point either serves or fails.
  if (!existsSync(binary)) {
    server = null;
    return;
  }
  // `target/` is missing on a clean checkout. Creating it keeps a missing
  // build a skipped suite, which is what the chromium guard above already
  // does, rather than an ENOENT thrown out of `before`.
  mkdirSync(dirname(configPath), { recursive: true });
  writeFileSync(configPath, "LIVEKIT_URL=wss://example.livekit.cloud\nLIVEKIT_API_KEY=face-detector-key\nLIVEKIT_API_SECRET=face-detector-secret\n");

  // Three ports before giving up. A lost race is a fresh number and another
  // try; three lost in a row is not a race any more.
  for (let attempt = 0; attempt < 3; attempt += 1) {
    PORT = await freePort();
    BASE = `http://127.0.0.1:${PORT}/`;
    server = spawn(
      binary,
      ["web", "--config", configPath, "--web-addr", `127.0.0.1:${PORT}`, "--web-dir", resolve(root, "web")],
      {
        cwd: root,
        stdio: "ignore",
        env: { ...process.env, CODETRIAL_DB_PATH: resolve(root, "target/face-detector-test.db") },
      },
    );
    if (await reachable()) return;
    server.kill();
    server = null;
  }
  // Built, launched, and never answered. Skipping here would report the same
  // thing as an unbuilt checkout, and the two are not the same thing at all.
  throw new Error(`the server never answered on 127.0.0.1:${PORT} after three ports`);
});

after(async () => {
  await browser?.close();
  server?.kill();
  rmSync(configPath, { force: true });
});

/// Draws something BlazeFace recognises without needing a photograph checked
/// into the repository.
const DRAW_FACE = `(withFace) => {
  const canvas = document.createElement("canvas");
  canvas.width = 320;
  canvas.height = 320;
  const c = canvas.getContext("2d");
  c.fillStyle = "#dcdcdc";
  c.fillRect(0, 0, 320, 320);
  if (withFace) {
    c.fillStyle = "#e8b58f";
    c.beginPath(); c.ellipse(160, 165, 88, 112, 0, 0, Math.PI * 2); c.fill();
    c.fillStyle = "#3a2a1c";
    c.beginPath(); c.ellipse(160, 70, 92, 58, 0, 0, Math.PI * 2); c.fill();
    c.fillStyle = "#ffffff";
    for (const x of [128, 192]) { c.beginPath(); c.ellipse(x, 148, 17, 11, 0, 0, Math.PI * 2); c.fill(); }
    c.fillStyle = "#20140c";
    for (const x of [128, 192]) { c.beginPath(); c.arc(x, 148, 7, 0, Math.PI * 2); c.fill(); }
    c.strokeStyle = "#8d4a3c"; c.lineWidth = 7;
    c.beginPath(); c.arc(160, 210, 34, 0.25 * Math.PI, 0.75 * Math.PI); c.stroke();
  }
  return canvas;
}`;

/// Drives `integrity-worker.js` the way `interview.js` does, rather than poking
/// `face-worker.js` directly. The detector sits behind a nested worker and is
/// fed a specific frame type, and testing the inner worker in isolation missed
/// exactly that: the page was sending a `VideoFrame`, which MediaPipe cannot
/// read, while this test hand-fed it an `ImageBitmap` and passed. Whatever
/// `postIntegrityFrame` posts is what has to work, so post the same thing.
async function watchFrames() {
  const page = await browser.newPage();
  try {
    await page.goto(BASE, { waitUntil: "domcontentloaded" });
    return await page.evaluate(async (drawSource) => {
      const draw = eval(`(${drawSource})`);
      const events = [];
      const worker = new Worker("/integrity-worker.js", { type: "module" });
      worker.onmessage = (event) => {
        if (event.data?.type === "integrity-event") events.push(event.data);
      };
      worker.onerror = (event) => {
        events.push({ eventType: "WORKER_ERROR", detail: event.message });
      };

      let at = 0;
      const send = async (canvas) => {
        at += 400;
        const frame = await createImageBitmap(canvas);
        worker.postMessage(
          { type: "frame", source: "camera", at, transport: "ImageBitmap", frame },
          [frame],
        );
      };

      // Feed frames until the event arrives rather than sleeping a fixed span.
      // The first detection pays for compiling the wasm and loading the model,
      // which takes tens of seconds on a cold cache, and everything after it is
      // fast; a fixed wait is either flaky or slow.
      const pump = async (canvas, wanted, budgetMs) => {
        const deadline = Date.now() + budgetMs;
        while (Date.now() < deadline) {
          await send(canvas);
          if (events.some((event) => event.eventType === wanted)) return true;
          if (events.some((event) => event.eventType === "FACE_DETECTOR_UNAVAILABLE")) return false;
          await new Promise((done) => setTimeout(done, 250));
        }
        return false;
      };

      await pump(draw(true), "FACE_DETECTED", 90000);
      await pump(draw(false), "FACE_MISSING", 20000);
      return { all: events };
    }, DRAW_FACE);
  } finally {
    await page.close();
  }
}

test("the whole camera pipeline detects a face and reports it losing one", async (t) => {
  if (!browser || !server) {
    t.skip("playwright browser or built server binary unavailable");
    return;
  }

  const { all } = await watchFrames();

  // The symptom every failure in this chain produces: a renamed vendored asset,
  // a DOM member the bridge does not supply, a wasm body served untyped, or a
  // frame type MediaPipe cannot read. `detail` carries which.
  const unavailable = all.find((event) => event.eventType === "FACE_DETECTOR_UNAVAILABLE");
  assert.equal(
    unavailable,
    undefined,
    `detector never ran: ${unavailable?.detail || ""}`,
  );

  const seen = all.map((event) => event.eventType);
  assert.ok(seen.includes("FACE_DETECTED"), `expected a detection, saw ${JSON.stringify(seen)}`);

  // Losing the face has to reach the tracker too, or nothing downstream of it
  // ever fires.
  assert.ok(
    seen.includes("FACE_MISSING"),
    `expected the blank frames to report a missing face, saw ${JSON.stringify(seen)}`,
  );
});

/// Pins the constraint that decided the transport in `postIntegrityFrame`.
/// MediaPipe reads `image.width`; a WebCodecs `VideoFrame` carries `codedWidth`
/// and `displayWidth` and no `width`, so it reaches the detector as an undefined
/// size and throws. Sending one was why every interview reported face detection
/// as unavailable, and the pipeline test above could not catch it because it
/// posts the frame type itself. If a future vendored build accepts a
/// `VideoFrame`, this fails and the cheaper transport becomes available again.
test("a VideoFrame is not a frame this detector can read", async (t) => {
  if (!browser || !server) {
    t.skip("playwright browser or built server binary unavailable");
    return;
  }

  const page = await browser.newPage();
  try {
    await page.goto(BASE, { waitUntil: "domcontentloaded" });
    const reason = await page.evaluate(async () => {
      if (typeof VideoFrame !== "function") return "unsupported";
      const canvas = document.createElement("canvas");
      canvas.width = 64;
      canvas.height = 64;
      canvas.getContext("2d").fillRect(0, 0, 64, 64);
      const worker = new Worker("/face-worker.js");
      const frame = new VideoFrame(canvas, { timestamp: 0 });
      return await new Promise((done) => {
        const timer = setTimeout(() => done("timeout"), 60000);
        worker.onmessage = (event) => {
          clearTimeout(timer);
          done(event.data.type === "result" ? "accepted" : event.data.reason || "unavailable");
        };
        worker.postMessage({ type: "detect", id: 1, frame }, [frame]);
      });
    });

    if (reason === "unsupported") {
      t.skip("this browser has no VideoFrame");
      return;
    }
    assert.notEqual(
      reason,
      "accepted",
      "VideoFrame is now readable: postIntegrityFrame can go back to the zero-copy handle",
    );
    assert.notEqual(reason, "timeout", "the detector should refuse a VideoFrame, not hang");
  } finally {
    await page.close();
  }
});
