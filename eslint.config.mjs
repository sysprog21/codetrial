import globals from "globals";

// The Rust half of this repo is gated at `-D warnings` and carries no
// `#[allow]` anywhere. The JavaScript half had no static analysis at all, which
// is the largest asymmetry in the project: a misspelled identifier in
// `web/interview.js` reaches a candidate's browser, while the same mistake in
// `src/web.rs` never leaves the developer's terminal.
//
// This config is deliberately narrow. Run against the codebase as it stands it
// finds almost nothing, because the code is already clean; every rule here is
// paying for the mistake somebody makes next year, not one sitting in the tree
// today. Rules were kept only if their failure mode is "this is a bug" rather
// than "this is a style I prefer", so the gate never becomes something people
// learn to argue with.
//
// Three of eslint's recommended rules are deliberately absent:
//
//   require-atomic-updates      fires on every `state.x = await f()` in
//                               `interview.js` and `app.js`. Five hits, zero
//                               real races: this code is single-threaded and
//                               the objects are not shared across tasks.
//   no-promise-executor-return  fires on `new Promise((r) => setTimeout(r, n))`
//                               in the browser tests, which is the idiom.
//   no-template-curly-in-string fires on tests that assert about template
//                               syntax, where the literal `${` is the subject.
//
// Vendored bytes under web/vendor are excluded: they are third-party bundles
// verified by SHA256SUMS, and linting somebody else's minified output tells us
// nothing we can act on.

const CORRECTNESS = {
  // The reason this file exists. A typo'd identifier is the one class of bug
  // that untyped JavaScript ships to production silently.
  "no-undef": "error",

  // Catches the other half of a rename: the old name left behind, the import
  // that no longer refers to anything, the constant nobody reads.
  "no-unused-vars": ["error", { args: "none", caughtErrors: "none" }],

  // Each of these is a statement that cannot mean what it says.
  "no-dupe-args": "error",
  "no-dupe-class-members": "error",
  "no-dupe-else-if": "error",
  "no-dupe-keys": "error",
  "no-duplicate-case": "error",
  "no-duplicate-imports": "error",
  "no-self-assign": "error",
  "no-self-compare": "error",
  "no-sparse-arrays": "error",
  "no-unreachable": "error",
  "no-unsafe-negation": "error",
  "no-unsafe-optional-chaining": "error",
  "use-isnan": "error",
  "valid-typeof": "error",

  // `case` fallthrough and a constant loop guard are both usually a missing
  // line rather than an intent, and both stay silent about it at runtime.
  "no-constant-condition": "error",
  "no-fallthrough": "error",

  // An `await` inside a promise executor swallows its own rejection.
  "no-async-promise-executor": "error",
};

export default [
  {
    ignores: ["web/vendor/**", "node_modules/**", "target/**"],
  },
  {
    // The browser tier. `web/*worker*.js` runs without a `window`, so the
    // worker globals ride along rather than splitting the tree in two for the
    // sake of two files.
    files: ["web/**/*.js"],
    languageOptions: {
      ecmaVersion: 2023,
      sourceType: "module",
      globals: { ...globals.browser, ...globals.worker },
    },
    linterOptions: { reportUnusedDisableDirectives: "error" },
    rules: CORRECTNESS,
  },
  {
    // Node: the browser tests and the generator scripts.
    //
    // Both global sets, not just node's. These files drive Playwright, and the
    // body of a `page.evaluate(() => ...)` is browser code that happens to be
    // written inside a Node module: it is serialised, shipped across the CDP
    // connection and run in the page, where `document` and `Worker` are exactly
    // as real as `process` is out here. Splitting them apart would mean marking
    // up every evaluate callback to say which half of the file it belongs to,
    // which buys a `no-undef` that is stricter about the wrong thing.
    files: ["tests/browser/**/*.js", "scripts/**/*.mjs", "scripts/**/*.cjs"],
    languageOptions: {
      ecmaVersion: 2023,
      sourceType: "module",
      globals: { ...globals.node, ...globals.browser },
    },
    linterOptions: { reportUnusedDisableDirectives: "error" },
    rules: CORRECTNESS,
  },
  {
    // `.cjs` predates `type: module` in package.json and is still CommonJS.
    files: ["scripts/**/*.cjs"],
    languageOptions: { sourceType: "commonjs" },
  },
  {
    // `importScripts` at the top of this worker pulls in the vendored MediaPipe
    // bundle, which announces itself as a bare global. Declared here rather
    // than silenced at the call site, so the day the vendor drops the name the
    // gate says so instead of the worker failing on a candidate's webcam.
    files: ["web/face-worker.js"],
    languageOptions: { globals: { FaceDetection: "readonly" } },
  },
];
