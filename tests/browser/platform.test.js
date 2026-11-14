// Run with: node --test tests/browser/platform.test.js
//
// The browser support rule in .claude/skills/codetrial-web/platform.md, held
// where a text scan can hold it without guessing at intent. web/ has no
// transpiler or polyfill layer, so the API a file names is the API a browser
// runs: a vendor-prefixed constructor for a feature every engine ships
// unprefixed, a viewport unit that ignores the mobile address bar, or a
// prefixed property with no standard one beside it reaches the browser as
// written. Each rule here names exact identifiers rather than a pattern, so it
// fails only where the code cannot mean anything else; web/vendor/ is never
// read.
//
// Scripts are scanned with their comments: no comment in web/ names these
// APIs, and a comment that does fails loudly, where stripping comments with
// a regex could also strip code after a `//` inside a string and hide a use.

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  filesUnder,
  firstPartyFiles,
  firstPartyScripts,
  read,
  stripCssComments,
} from "./source.js";

/// Each file's text keyed by its repository path.
const load = (paths) =>
  Object.fromEntries(paths.map((path) => [path, read(path)]));
const loadWeb = (names) => load(names.map((name) => `web/${name}`));

/// Every Rust source, keyed by path: pages the server builds carry their
/// markup, stylesheet and script as strings.
const rustPages = load(filesUnder("src", ".rs").map((name) => `src/${name}`));

/// The `<tag>` blocks of those pages. Everything outside one is blanked, line
/// breaks kept, so a hit reports the line in the .rs file.
function embedded(tag) {
  const block = new RegExp(`(<${tag}\\b[^>]*>[\\s\\S]*?</${tag}>)`);
  return Object.fromEntries(
    Object.entries(rustPages).flatMap(([path, text]) => {
      const parts = text.split(block);
      if (parts.length === 1) return [];
      const kept = parts.map((part, index) =>
        index % 2 ? part : part.replace(/[^\n]/g, ""),
      );
      return [[path, kept.join("")]];
    }),
  );
}

const embeddedSheets = embedded("style");
const embeddedScripts = embedded("script");
const sheets = Object.fromEntries(
  Object.entries({
    ...loadWeb(firstPartyFiles(".css")),
    ...embeddedSheets,
  }).map(([path, css]) => [path, stripCssComments(css)]),
);
const scripts = { ...loadWeb(firstPartyScripts()), ...embeddedScripts };
const webPages = loadWeb(firstPartyFiles(".html"));

/// Every rule that declares something, as { selectors, body, scope }, `scope`
/// being the preludes around it, outermost first (`@media (min-width:
/// 1000px)`), so a declaration that only holds under a condition is not taken
/// to hold everywhere. Text before a block's `{` up to the last `;` is
/// declarations of the block around it (or a statement such as `@import` at
/// the top level), not part of the prelude, so a nested rule neither hides
/// its parent's declarations nor takes them into its own selector.
function rules(css) {
  const found = [];
  const open = [];
  let start = 0;
  for (let i = 0; i < css.length; i += 1) {
    if (css[i] === "{") {
      const chunk = css.slice(start, i);
      const cut = chunk.lastIndexOf(";") + 1;
      if (open.length) {
        open.at(-1).parent = true;
        open.at(-1).own += chunk.slice(0, cut);
      }
      open.push({ prelude: chunk.slice(cut).trim(), own: "" });
      start = i + 1;
    } else if (css[i] === "}") {
      const block = open.pop();
      if (block) {
        const body = block.own + css.slice(start, i);
        if (!block.parent || body.trim())
          found.push({
            selectors: block.prelude.split(",").map((part) => part.trim()),
            body,
            scope: open.map((outer) => outer.prelude),
          });
      }
      start = i + 1;
    }
  }
  return found;
}
const declares = (body, property) =>
  new RegExp(`(^|[;\\s])${property}\\s*:`).test(body);

const hits = (files, pattern) =>
  Object.entries(files).flatMap(([path, text]) =>
    text
      .split("\n")
      .flatMap((line, index) =>
        pattern.test(line) ? [`${path}:${index + 1}: ${line.trim()}`] : [],
      ),
  );

test("the embedded page scan finds the setup page's stylesheet and script", () => {
  const page = "src/web/setup.rs";
  const unchecked = "the embedded pages go unchecked";
  assert.ok(page in embeddedSheets, `no stylesheet in ${page}; ${unchecked}`);
  assert.ok(page in embeddedScripts, `no script in ${page}; ${unchecked}`);
});

test("audio contexts are built unprefixed", () => {
  // AudioContext has shipped unprefixed in every engine since 2021.
  assert.deepEqual(
    hits(scripts, /\bwebkit(Offline)?AudioContext\b/),
    [],
    "use AudioContext; see platform.md, Which browsers",
  );
});

test("only the task admission reads the user agent", () => {
  // web/task-room.js reports supported_platform for the server's admission
  // check; anywhere else a user-agent read stands in for feature detection.
  const admission = "web/task-room.js";
  const { [admission]: allowed, ...outside } = scripts;
  assert.deepEqual(
    hits(
      outside,
      // The names alone, so bracket access and destructuring count too;
      // vendor is a common word, so it counts only read from navigator.
      /\b(userAgent|userAgentData|appVersion)\b|\bnavigator\s*(\?\.|\.|\??\.?\[\s*["'])\s*vendor\b/,
    ),
    [],
    "detect the feature instead; see platform.md, In new code",
  );
  assert.match(
    allowed ?? "",
    /navigator\.userAgent/,
    "the allowance names a file that no longer reads the user agent",
  );
});

test("viewport heights use dynamic units, not vh", () => {
  // vh is the largest viewport, so on a phone a 100vh layout sits under the
  // address bar. dvh, svh and lvh each say which viewport they mean.
  // Inline styles and style strings set from script count too. A minus sign
  // counts as a boundary unless it continues a name such as `--gap-10vh`.
  assert.deepEqual(
    hits(
      { ...sheets, ...webPages, ...scripts },
      // CSS units are case-insensitive: 100VH is 100vh.
      /(^|[^\w.-]|(?<![\w-])-)\d*\.?\d+vh\b/i,
    ),
    [],
    "use dvh (or svh/lvh where that is the point); see platform.md",
  );
});

test("a prefixed property has its standard property beside it", () => {
  // Properties with no standard equivalent, so there is nothing to pair.
  const unpaired = new Set([
    "-webkit-font-smoothing", // macOS text rendering; no standard property
    "-moz-osx-font-smoothing", // the same, for Firefox on macOS
  ]);
  const missing = Object.entries(sheets).flatMap(([path, css]) =>
    rules(css).flatMap(({ selectors, body }) =>
      [...body.matchAll(/(?:^|[;\s])(-(?:webkit|moz|ms)-([a-z-]+))\s*:/g)]
        .filter(
          ([, property, standard]) =>
            !unpaired.has(property) && !declares(body, standard),
        )
        .map(
          ([, property, standard]) =>
            `${path}: ${selectors.join(", ")} { ${property} } has no ${standard}`,
        ),
    ),
  );
  assert.deepEqual(missing, [], "add the standard property in the same rule");
});

test("a ::-webkit-scrollbar rule has scrollbar-width on the same element", () => {
  // ::-webkit-scrollbar reaches only WebKit and Blink; Firefox, and Chromium
  // since 121, read scrollbar-width. The pair holds when the standard
  // property is declared under the same conditions or fewer.
  const SCROLLBAR = "::-webkit-scrollbar";
  /// Whether `outer` is `inner` or encloses it.
  const encloses = (outer, inner) =>
    outer.every((prelude, index) => inner[index] === prelude);
  const missing = Object.entries(sheets).flatMap(([path, css]) => {
    const all = rules(css);
    return all.flatMap(({ selectors, scope }) =>
      selectors
        .filter((selector) => selector.endsWith(SCROLLBAR))
        .filter((selector) => {
          const element = selector.slice(0, -SCROLLBAR.length);
          return !all.some(
            (rule) =>
              rule.selectors.includes(element) &&
              declares(rule.body, "scrollbar-width") &&
              encloses(rule.scope, scope),
          );
        })
        .map((selector) => `${path}: ${[...scope, selector].join(" ")}`),
    );
  });
  assert.deepEqual(missing, [], "declare scrollbar-width on that element too");
});

test("no new hand-built dialog: a new modal is a <dialog>", () => {
  // These three predate the rule; web/interview.js manages their inert and
  // focus by hand, and #ending-overlay is deliberately not modal. The list may
  // shrink but not grow: a native <dialog> brings focus, inertness, Escape and
  // the top layer that a role="dialog" section has to rebuild.
  const known = new Set(
    ["audio-check", "ending-overlay", "report-modal"].map(
      (id) => `web/interview.html#${id}`,
    ),
  );
  const pages = { ...webPages, ...rustPages };
  const added = Object.entries(pages).flatMap(([path, html]) =>
    [...html.matchAll(/<[a-z][^>]*\brole\s*=\s*["']?dialog\b[^>]*>/gi)]
      .map(([tag]) => {
        const id = tag.match(/\bid\s*=\s*["']?([^"'\s>]+)/i)?.[1];
        return `${path}#${id ?? "(no id)"}`;
      })
      .filter((element) => !known.has(element)),
  );
  assert.deepEqual(
    added,
    [],
    'a role="dialog" element; use <dialog> with showModal(), see platform.md, In new code',
  );
  assert.deepEqual(
    hits(scripts, /role["']\s*,\s*["']dialog|\.role\s*=\s*["']dialog/),
    [],
    "a script-built dialog role; use <dialog>",
  );
});
