// Run with: node --test tests/browser/theme.test.js
//
// A theme is a set of custom property overrides: web/theme.css holds the
// defaults and web/theme-override.css, empty as shipped, a deployment's own.
// That only holds while the component stylesheets name roles and never colors,
// and while every page loads both files, in that order, before the rules that
// read them. What can break is a page linking its own sheet without the theme
// (it renders with no colors at all), a rule painting a color a theme cannot
// reach, a component sheet redeclaring a theme role (the page drifts from the
// others), or a rule reading a token nobody declares (a typo falls back
// silently).

import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync } from "node:fs";
import { join } from "node:path";

import {
  captures,
  launchChromium,
  read,
  root,
  startStaticServer,
  stripCssComments,
} from "./source.js";

const declared = (css) => new Set(captures(css, /(--[\w-]+)\s*:/g));
const components = ["styles.css", "task.css", "peer.css"];
const sheets = Object.fromEntries(
  components.map((name) => [name, stripCssComments(read(`web/${name}`))]),
);
const theme = stripCssComments(read("web/theme.css"));

/// Every CSS named color (CSS Color 4, 148 names). A system color such as
/// Highlight is left out: it is the platform's, and forced-colors rules need it.
const CSS_NAMED_COLORS = `
  aliceblue antiquewhite aqua aquamarine azure beige bisque black
  blanchedalmond blue blueviolet brown burlywood cadetblue chartreuse
  chocolate coral cornflowerblue cornsilk crimson cyan darkblue darkcyan
  darkgoldenrod darkgray darkgreen darkgrey darkkhaki darkmagenta
  darkolivegreen darkorange darkorchid darkred darksalmon darkseagreen
  darkslateblue darkslategray darkslategrey darkturquoise darkviolet deeppink
  deepskyblue dimgray dimgrey dodgerblue firebrick floralwhite forestgreen
  fuchsia gainsboro ghostwhite gold goldenrod gray green greenyellow grey
  honeydew hotpink indianred indigo ivory khaki lavender lavenderblush
  lawngreen lemonchiffon lightblue lightcoral lightcyan lightgoldenrodyellow
  lightgray lightgreen lightgrey lightpink lightsalmon lightseagreen
  lightskyblue lightslategray lightslategrey lightsteelblue lightyellow lime
  limegreen linen magenta maroon mediumaquamarine mediumblue mediumorchid
  mediumpurple mediumseagreen mediumslateblue mediumspringgreen
  mediumturquoise mediumvioletred midnightblue mintcream mistyrose moccasin
  navajowhite navy oldlace olive olivedrab orange orangered orchid
  palegoldenrod palegreen paleturquoise palevioletred papayawhip peachpuff
  peru pink plum powderblue purple rebeccapurple red rosybrown royalblue
  saddlebrown salmon sandybrown seagreen seashell sienna silver skyblue
  slateblue slategray slategrey snow springgreen steelblue tan teal thistle
  tomato turquoise violet wheat white whitesmoke yellow yellowgreen
`
  .trim()
  .split(/\s+/);

/// The stylesheets a page links, in order, whatever order a <link> writes its
/// attributes in.
const linkedSheets = (html) =>
  [...html.matchAll(/<link\b[^>]*>/g)]
    .map((match) => match[0])
    .filter((tag) => /\brel="stylesheet"/.test(tag))
    .map((tag) => tag.match(/\bhref="\/([^"]+)"/)?.[1]);

test("every styled page loads the theme, then its overrides, first", () => {
  const styled = readdirSync(join(root, "web"))
    .filter((name) => name.endsWith(".html"))
    .filter((name) => {
      const linked = linkedSheets(read(`web/${name}`));
      if (!linked.some((sheet) => components.includes(sheet))) return false;
      assert.deepEqual(
        linked.slice(0, 2),
        ["theme.css", "theme-override.css"],
        `${name} links ${linked.join(", ")}`,
      );
      return true;
    });
  // Named, so a page that stops linking a component sheet is noticed too.
  assert.deepEqual(styled.sort(), [
    "index.html",
    "interview.html",
    "observer.html",
    "peer-interview.html",
    "peer-lobby.html",
    "replay.html",
    "task.html",
  ]);
});

test("the shipped override file declares nothing", () => {
  assert.deepEqual(
    [...declared(stripCssComments(read("web/theme-override.css")))],
    [],
  );
});

test("component stylesheets paint no color a theme cannot override", () => {
  const colorFunction =
    /\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(|#[0-9a-f]{3,8}\b/i;
  const namedColor = new RegExp(`\\b(?:${CSS_NAMED_COLORS.join("|")})\\b`, "i");
  // Declarations, not lines: a value wrapped onto the lines after its property
  // is still read, and a selector such as `#face-check` is not a hex color.
  for (const [name, css] of Object.entries(sheets)) {
    for (const [, body] of css.matchAll(/\{([^{}]*)\}/g)) {
      for (const declaration of body.split(";")) {
        const [, property, value] =
          declaration.match(/^\s*([\w-]+)\s*:([\s\S]*)$/) ?? [];
        // A mask paints coverage, not color: its stops are alpha only.
        if (!property || /^(?:-webkit-)?mask$/.test(property)) continue;
        const where = `${name}: ${property}:${value.replace(/\s+/g, " ")}`;
        assert.doesNotMatch(value, colorFunction, where);
        assert.doesNotMatch(value, namedColor, where);
      }
    }
  }
});

test("a component sheet neither redeclares a theme role nor reads an undeclared one", () => {
  const themed = declared(theme);
  // peer.css sits on top of styles.css, so a component may read a layout
  // value another component sheet declares.
  const known = new Set([
    ...themed,
    ...Object.values(sheets).flatMap((css) => [...declared(css)]),
  ]);
  for (const [name, css] of Object.entries(sheets)) {
    const redeclared = [...declared(css)].filter((token) => themed.has(token));
    assert.deepEqual(redeclared, [], `${name} redeclares theme roles`);
    // A read with a fallback, var(--x, value), is a value scripts set at
    // runtime (a measured height, a progress fraction), not a theme role.
    const missing = [...new Set(captures(css, /var\((--[\w-]+)\)/g))].filter(
      (token) => !known.has(token),
    );
    assert.deepEqual(missing, [], `${name} reads undeclared tokens`);
  }
});

let browser = null;
let server = null;
let base = "";

before(async () => {
  browser = await launchChromium();
  if (!browser) return;
  ({ server, base } = await startStaticServer());
});

after(async () => {
  await browser?.close();
  await new Promise((closed) => (server ? server.close(closed) : closed()));
});

test("the task editor stays dark on a light system while the interview's follows it", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  const editorBackground = async (path) => {
    const page = await browser.newPage({ colorScheme: "light" });
    t.after(() => page.close());
    // Read before any script runs: the stylesheet alone owns the default.
    await page.route(/\.js(\?|$)/, (route) => route.abort());
    await page.goto(`${base}/${path}`, { waitUntil: "domcontentloaded" });
    return page
      .locator(".editor-stack")
      .evaluate((stack) => getComputedStyle(stack).backgroundColor);
  };
  assert.equal(await editorBackground("task.html"), "rgb(30, 30, 30)");
  assert.equal(await editorBackground("interview.html"), "rgb(255, 255, 255)");
});
