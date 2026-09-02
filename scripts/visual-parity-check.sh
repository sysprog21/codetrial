#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$ROOT/scripts/session-cookie.sh"
GOLDEN_DIR=${VISUAL_PARITY_GOLDEN_DIR:-$ROOT/tests/golden/visual}
ARTIFACT_DIR=${VISUAL_PARITY_ARTIFACT_DIR:-$ROOT/tests/artifacts/visual}
THRESHOLD=${VISUAL_PARITY_THRESHOLD:-0.05}
TMP=$(mktemp -d)
SERVER_LOG="$TMP/server.log"
LOGIN_DB_PATH="$TMP/accounts.db"
LOGIN_SESSION_SECRET=visual-parity-session
CONFIG_PATH="$TMP/codetrial.env.local"
printf '%s\n' \
    'LIVEKIT_URL=wss://example.livekit.cloud' \
    'LIVEKIT_API_KEY=visual-parity-key' \
    'LIVEKIT_API_SECRET=visual-parity-secret' > "$CONFIG_PATH"

cleanup()
{
    if [ "${SERVER_PID:-}" ]; then
        kill "$SERVER_PID" 2> /dev/null || true
        wait "$SERVER_PID" 2> /dev/null || true
    fi
    rm -r "$TMP" 2> /dev/null || true
}
trap cleanup EXIT INT TERM

# Resolve Playwright from the repo's own devDependencies so the usual path is
# `npm install` with no environment variable. PLAYWRIGHT_PATH stays an override
# for a Playwright installed elsewhere.
if [ -z "${PLAYWRIGHT_PATH:-}" ]; then
    PLAYWRIGHT_PATH=$(node -e "console.log(require.resolve('playwright', { paths: ['$ROOT'] }))" 2> /dev/null || true)
fi
if [ -z "$PLAYWRIGHT_PATH" ]; then
    echo "Playwright is not installed. From the repo root:" >&2
    echo "  npm install && npx playwright install chromium" >&2
    echo "Or set PLAYWRIGHT_PATH to an existing Playwright module." >&2
    exit 1
fi

PORT=${PORT:-}
if [ -z "$PORT" ]; then
    PORT=$(node -e "const s=require('net').createServer();s.listen(0,'127.0.0.1',()=>{console.log(s.address().port);s.close();});")
fi
BASE_URL="http://127.0.0.1:$PORT"

if curl -fsS "$BASE_URL" > /dev/null 2>&1; then
    echo "Port $PORT is already serving HTTP; set PORT to a free port." >&2
    exit 1
fi

mkdir -p "$GOLDEN_DIR" "$ARTIFACT_DIR"

env -u LIVEKIT_URL -u LIVEKIT_API_KEY -u LIVEKIT_API_SECRET -u GOOGLE_API_KEY \
    SESSION_SECRET="$LOGIN_SESSION_SECRET" \
    CODETRIAL_DB_PATH="$LOGIN_DB_PATH" \
    cargo run --quiet --manifest-path "$ROOT/Cargo.toml" -- web --config "$CONFIG_PATH" --web-addr "127.0.0.1:$PORT" --web-dir "$ROOT/web" \
    > "$SERVER_LOG" 2>&1 &
SERVER_PID=$!

i=0
until curl -fsS "$BASE_URL" > /dev/null 2>&1; do
    i=$((i + 1))
    if [ "$i" -gt 90 ]; then
        cat "$SERVER_LOG" >&2
        exit 1
    fi
    sleep 1
done

echo "visual parity threshold: $THRESHOLD"

SESSION_COOKIE=${VISUAL_PARITY_SESSION_COOKIE:-}
if [ -z "$SESSION_COOKIE" ]; then
    SESSION_COOKIE=$(login_session_cookie "$BASE_URL" visual-parity) || exit 2
fi

BASE_URL="$BASE_URL" \
    GOLDEN_DIR="$GOLDEN_DIR" \
    ARTIFACT_DIR="$ARTIFACT_DIR" \
    SESSION_COOKIE="$SESSION_COOKIE" \
    PLAYWRIGHT_PATH="$PLAYWRIGHT_PATH" \
    VISUAL_PARITY_CSS="${VISUAL_PARITY_CSS:-}" \
    VISUAL_PARITY_THRESHOLD="$THRESHOLD" \
    node << 'NODE'
const { chromium } = require(process.env.PLAYWRIGHT_PATH);
const fs = require("fs");
const path = require("path");

const pages = [
  { name: "home", route: "/", ready: "Practice a live technical interview", selectors: ["body", "main", "h1", ".problem-grid", ".problem-card", ".duration-row", ".start-row"] },
  { name: "interview", route: "/interview?problem=two-sum&duration=45", ready: "Two Sum", selectors: ["body", "main", "#problem-title", ".interview-sidebar", ".editor-panel", "#editor", "#run-tests", "#transcript-panel"] },
];

// scripts/session-cookie.sh hands over a bare `name=value`, attributes already
// stripped, so this only has to split on the first `=`.
const [cookieName, ...cookieValue] = (process.env.SESSION_COOKIE || "").split("=");

async function capture(browser, item, output) {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, deviceScaleFactor: 1 });
  if (cookieName && cookieValue.length > 0) {
    await page.context().addCookies([{ name: cookieName, value: cookieValue.join("="), url: process.env.BASE_URL, httpOnly: true, sameSite: "Lax" }]);
  }
  await page.goto(`${process.env.BASE_URL}${item.route}`, { waitUntil: "domcontentloaded" });
  await page.getByText(item.ready).first().waitFor({ timeout: 30000 });
  if (await page.locator("#audio-check").isVisible().catch(() => false)) {
    await page.getByRole("button", { name: "Play test tone" }).click();
    await page.getByRole("button", { name: "I heard it" }).click();
    const join = page.getByRole("button", { name: "Start interview" });
    for (let i = 0; i < 60 && (await join.isDisabled()); i++) {
      await page.waitForTimeout(500);
    }
    await join.click();
    await page.locator("#audio-check").waitFor({ state: "hidden" });
  }
  if (process.env.VISUAL_PARITY_CSS) {
    await page.addStyleTag({ path: process.env.VISUAL_PARITY_CSS });
  }
  await page.screenshot({ path: output, fullPage: true });
  const selectorBoxes = await page.evaluate((candidates) => candidates.flatMap((selector) => {
    const element = document.querySelector(selector);
    if (!element) return [];
    const rect = element.getBoundingClientRect();
    return [{
      selector,
      left: Math.floor(rect.left + window.scrollX),
      top: Math.floor(rect.top + window.scrollY),
      right: Math.ceil(rect.right + window.scrollX),
      bottom: Math.ceil(rect.bottom + window.scrollY),
    }];
  }), item.selectors);
  await page.close();
  return selectorBoxes;
}

async function compare(browser, expectedPath, actualPath, diffPath, selectorBoxes) {
  const page = await browser.newPage();
  const result = await page.evaluate(async ({ expectedPng, actualPng, selectorBoxes }) => {
    function image(data) {
      return new Promise((resolve, reject) => {
        const img = new Image();
        img.onload = () => resolve(img);
        img.onerror = reject;
        img.src = `data:image/png;base64,${data}`;
      });
    }
    const expected = await image(expectedPng);
    const actual = await image(actualPng);
    const width = Math.max(expected.width, actual.width);
    const height = Math.max(expected.height, actual.height);
    const canvas = document.createElement("canvas");
    const other = document.createElement("canvas");
    const diff = document.createElement("canvas");
    for (const node of [canvas, other, diff]) {
      node.width = width;
      node.height = height;
    }
    const a = canvas.getContext("2d");
    const b = other.getContext("2d");
    const d = diff.getContext("2d");
    a.drawImage(expected, 0, 0);
    b.drawImage(actual, 0, 0);
    const first = a.getImageData(0, 0, width, height);
    const second = b.getImageData(0, 0, width, height);
    const out = d.createImageData(width, height);
    let changed = 0;
    const changedSelectors = new Set();
    for (let i = 0; i < first.data.length; i += 4) {
      const delta =
        Math.abs(first.data[i] - second.data[i]) +
        Math.abs(first.data[i + 1] - second.data[i + 1]) +
        Math.abs(first.data[i + 2] - second.data[i + 2]) +
        Math.abs(first.data[i + 3] - second.data[i + 3]);
      if (delta > 96) {
        changed += 1;
        const pixel = i / 4;
        const x = pixel % width;
        const y = Math.floor(pixel / width);
        for (const box of selectorBoxes) {
          if (x >= box.left && x <= box.right && y >= box.top && y <= box.bottom) {
            changedSelectors.add(box.selector);
          }
        }
        out.data[i] = 220;
        out.data[i + 1] = 40;
        out.data[i + 2] = 40;
        out.data[i + 3] = 255;
      } else {
        const shade = Math.round((first.data[i] + first.data[i + 1] + first.data[i + 2]) / 6);
        out.data[i] = shade;
        out.data[i + 1] = shade;
        out.data[i + 2] = shade;
        out.data[i + 3] = 180;
      }
    }
    d.putImageData(out, 0, 0);
    return { changed, total: width * height, diff: diff.toDataURL("image/png"), changedSelectors: [...changedSelectors] };
  }, {
    expectedPng: fs.readFileSync(expectedPath).toString("base64"),
    actualPng: fs.readFileSync(actualPath).toString("base64"),
    selectorBoxes,
  });
  await page.close();
  fs.writeFileSync(diffPath, Buffer.from(result.diff.split(",")[1], "base64"));
  return { ratio: result.changed / result.total, changedSelectors: result.changedSelectors };
}

(async () => {
  // Fake media so the mandatory media gate can be cleared before capture;
  // otherwise every interview screenshot would just be the gate.
  const browser = await chromium.launch({
    args: ["--use-fake-device-for-media-stream", "--use-fake-ui-for-media-stream"],
  });
  let failed = false;
  try {
    for (const item of pages) {
      const goldenPath = path.join(process.env.GOLDEN_DIR, `${item.name}.png`);
      if (!fs.existsSync(goldenPath)) {
        throw new Error(`missing visual golden: ${goldenPath}`);
      }
      const actualPath = path.join(process.env.ARTIFACT_DIR, `${item.name}-actual.png`);
      const diffPath = path.join(process.env.ARTIFACT_DIR, `${item.name}-diff.png`);
      const selectorBoxes = await capture(browser, item, actualPath);
      const result = await compare(browser, goldenPath, actualPath, diffPath, selectorBoxes);
      const ratio = result.ratio;
      if (ratio > Number(process.env.VISUAL_PARITY_THRESHOLD)) {
        failed = true;
        console.error(`${item.name} visual diff ${(ratio * 100).toFixed(2)}%`);
        console.error(`diff image: ${diffPath}`);
        console.error(`changed selectors: ${result.changedSelectors.join(", ") || "(none)"}`);
      } else {
        console.log(`${item.name} visual diff ${(ratio * 100).toFixed(2)}%`);
      }
    }
  } finally {
    await browser.close();
  }
  if (failed) process.exit(1);
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
NODE
