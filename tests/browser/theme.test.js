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
const recordingCss = stripCssComments(read("web/recording/recording.css"));
// Theme tokens holding a bare number, length or duration. Overriding one is
// not a color concern, so reading one does not count as a token read below.
const lengthTokens = new Set(
  captures(
    [theme, ...Object.values(sheets), recordingCss].join("\n"),
    /(--[\w-]+)\s*:\s*-?[\d.]+[a-z%]*\s*;/g,
  ),
);
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

const colorFunction =
  /\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(|#[0-9a-f]{3,8}\b/i;
const namedColor = new RegExp(`\\b(?:${CSS_NAMED_COLORS.join("|")})\\b`, "i");
// Each rule body's declarations, as [property, value] pairs.
const ruleDeclarations = (css) =>
  [...css.matchAll(/\{([^{}]*)\}/g)].map(([, body]) =>
    body
      .split(";")
      .map((d) => d.match(/^\s*([\w-]+)\s*:([\s\S]*)$/)?.slice(1))
      .filter(Boolean),
  );
/// The declarations in a sheet that paint a color literal. Declarations, not
/// lines: a value wrapped onto the lines after its property is still read, and
/// a selector such as `#face-check` is not a hex color.
const paintedColors = (name, css) =>
  ruleDeclarations(css).flatMap((declarations) =>
    declarations.flatMap(([property, value]) => {
      // A mask paints coverage, not color: its stops are alpha only.
      if (/^(?:-webkit-)?mask$/.test(property)) return [];
      const bare = value.replace(/--[\w-]+/g, "");
      if (!colorFunction.test(bare) && !namedColor.test(bare)) return [];
      return [`${name}: ${property}:${value.replace(/\s+/g, " ")}`];
    }),
  );

const SIDES = ["top", "right", "bottom", "left"].map((s) => `border-${s}`);
const SIDE_COLORS = SIDES.map((s) => `${s}-color`);
const SHORTHAND_RESETS = {
  background: ["background-color", "background-image"],
  border: ["border-color", ...SIDES, ...SIDE_COLORS],
  "border-color": SIDE_COLORS,
  outline: ["outline-color"],
  ...Object.fromEntries(SIDES.map((s, i) => [s, [SIDE_COLORS[i]]])),
};
const expandsTo = (p) => [p, ...(SHORTHAND_RESETS[p] ?? [])];
const colorTokens = new Set(
  [theme, ...Object.values(sheets), recordingCss]
    .flatMap((css) => [...declared(css)])
    .filter((t) => !lengthTokens.has(t)),
);
const shadowedTokenReads = (name, css) => {
  const offending = [];
  for (const declarations of ruleDeclarations(css)) {
    const reads = new Set();
    const expanded = new Set();
    for (const [prop, val] of declarations) {
      const isToken = [...val.matchAll(/var\(\s*(--[\w-]+)/g)].some(([, t]) =>
        colorTokens.has(t),
      );
      const targets = expandsTo(prop);
      if (targets.some((t) => reads.has(t) || (!isToken && expanded.has(t)))) {
        offending.push(
          `${name} resets ${prop} after a token read in the same rule: ${val.trim()}`,
        );
      }
      if (isToken) {
        reads.add(prop);
        for (const t of targets) expanded.add(t);
      }
    }
  }
  return offending;
};

const unusedThemeTokens = (themeCss, allCss) => {
  const themed = declared(themeCss);
  const readTokens = new Set(
    captures(allCss, /var\(\s*(--[\w-]+)\s*(?=[,)])/g),
  );
  return [...themed].filter((token) => !readTokens.has(token));
};

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
  for (const [name, css] of Object.entries(sheets)) {
    assert.deepEqual(paintedColors(name, css), [], `${name} paints colors`);
  }
});

test("a declaration does not shadow a token read in the same rule", () => {
  // A later declaration (including shorthands) must not drop an earlier token read.
  for (const [name, css] of Object.entries(sheets)) {
    assert.deepEqual(shadowedTokenReads(name, css), []);
  }
  assert.deepEqual(shadowedTokenReads("recording.css", recordingCss), []);
  for (const css of [
    ".card { background-color: var(--page); background: none; }",
    ".card { border-top-color: var(--accent); border-top: none; }",
    ".card { outline-color: var(--accent); outline: var(--focus-width) solid; }",
    ".card { border: 1px solid var(--edge); border-top: var(--focus-width) solid; }",
    ".card { background-image: linear-gradient(var(--accent), transparent); background: none; }",
    ".card { box-shadow: var(--shadow-card); box-shadow: none; }",
    ".card { color: var(--ink); color: var(--accent); }",
    ".card { border: 1px solid var(--edge); border-top: 2px solid; }",
    ".card { border: 1px solid var(--edge); border-color: transparent; }",
    ".card { border: 1px solid var(--edge); border-top: var(--width, 2px) solid; }",
  ]) {
    assert.ok(shadowedTokenReads("synthetic.css", css).length > 0, css);
  }
  // A token-reading longhand after a token-reading shorthand is a deliberate
  // override (the `.spinner` rule in styles.css), not a dropped read.
  assert.deepEqual(
    shadowedTokenReads(
      "synthetic.css",
      ".spinner { border: 2px solid var(--hairline); border-top-color: var(--accent); }",
    ),
    [],
  );
});

const recordingPaletteProblems = (css) => {
  let root = "";
  const rest = css.replace(/:root\s*\{([^{}]*)\}/g, (_, b) => {
    root += b;
    return `{${b.replace(/--[\w-]+\s*:[^;]*(?:;|$)/g, "")}}`;
  });
  const decl = declared(root);
  const reads = new Set(captures(css, /var\(\s*(--[\w-]+)\s*(?=[,)])/g));
  return [
    ...paintedColors("recording.css", rest),
    ...[...declared(rest)].map((t) => `outside :root: ${t}`),
    ...[...reads].filter((t) => !decl.has(t)).map((t) => `undeclared: ${t}`),
    ...[...decl].filter((t) => !reads.has(t)).map((t) => `unread: ${t}`),
  ];
};

test("recording.css confines color literals and token declarations to its own :root", () => {
  assert.deepEqual(recordingPaletteProblems(recordingCss), []);
  for (const css of [
    ".card { color: #fff; }",
    ".card { --late: 4px; }",
    ":root { --ink: #fff; } .card { color: var(--ink); border-color: var(--missing); }",
    ":root { --ink: #fff; --unused: #000; } .card { color: var(--ink); }",
  ]) {
    assert.ok(recordingPaletteProblems(css).length > 0, css);
  }
});

test("every theme token is read by at least one component or derived token", () => {
  const allCss = [...Object.values(sheets), theme].join("\n");
  assert.deepEqual(
    unusedThemeTokens(theme, allCss),
    [],
    "theme.css declares unused tokens",
  );
  assert.deepEqual(
    unusedThemeTokens(`${theme}\n:root { --obsolete: #fff; }`, allCss),
    ["--obsolete"],
    "orphan token in theme.css must be caught",
  );
  assert.deepEqual(
    unusedThemeTokens(
      `${theme}\n:root { --fallback-read: #fff; }`,
      `${allCss}\n.card { color: var(--fallback-read, currentColor); }`,
    ),
    [],
    "a token read with a fallback must count as read",
  );
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
    const missing = [
      ...new Set(captures(css, /var\(\s*(--[\w-]+)\s*\)/g)),
    ].filter((token) => !known.has(token));
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

test("task dialogs keep their backdrop in both system themes", async (t) => {
  if (!browser) return t.skip("playwright chromium unavailable");
  for (const colorScheme of ["light", "dark"]) {
    const page = await browser.newPage({ colorScheme });
    t.after(() => page.close());
    await page.route(/\.js(\?|$)/, (route) => route.abort());
    await page.goto(`${base}/task.html`, { waitUntil: "domcontentloaded" });
    const dialog = page.locator("#confirm");
    await dialog.evaluate((node) => node.showModal());
    assert.equal(
      await dialog.evaluate(
        (node) => getComputedStyle(node, "::backdrop").backgroundColor,
      ),
      "rgba(0, 0, 0, 0.65)",
    );
  }
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
