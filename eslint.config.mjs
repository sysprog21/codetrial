import globals from "globals";

// The Rust half of this repo is gated at `-D warnings` and carries four
// `#[allow]`s in 45k lines, each with a reason written beside it. The
// JavaScript half had no static analysis at all, which is the largest asymmetry
// in the project: a misspelled identifier in `web/interview.js` reaches a
// candidate's browser, while the same mistake in `src/web/mod.rs` never leaves
// the developer's terminal.
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

  // A block with nothing in it, a `catch` included, drops what reached it
  // unexplained: a comment inside says why it may be empty. A `debugger` left
  // in stops whoever has the tools open. `promise.catch(() => {})`, this
  // tree's fire-and-forget, is an arrow function's body, not a block.
  "no-empty": "error",
  "no-debugger": "error",

  // More statements that cannot mean what they say. None had a match when
  // they were added, so they cost nothing and keep it that way: a getter that
  // returns nothing, a `finally` whose `return` drops the error, a loop that
  // can only run once, a literal the double rounds, a `map` callback that
  // forgets to return, a write to an import, a constant, a class, a function
  // or a global, `x === -0`, a `for` counting away from its bound, a regex that
  // cannot compile or cannot match, and a line break that joins two
  // statements into one call.
  "array-callback-return": "error",
  "for-direction": "error",
  "getter-return": "error",
  "no-class-assign": "error",
  "no-compare-neg-zero": "error",
  "no-const-assign": "error",
  "no-constructor-return": "error",
  "no-empty-character-class": "error",
  "no-func-assign": "error",
  "no-global-assign": "error",
  "no-import-assign": "error",
  "no-invalid-regexp": "error",
  "no-loss-of-precision": "error",
  "no-shadow-restricted-names": "error",
  "no-unexpected-multiline": "error",
  "no-unreachable-loop": "error",
  "no-unsafe-finally": "error",

  // `let` in a `case` leaks into the next one; `new String` is never equal to
  // the string; `setTimeout("code")` is `eval`; a `catch` that only rethrows
  // is a `try` that says nothing; and a rejection that is not an Error has no
  // stack to say where it came from.
  "no-case-declarations": "error",
  "no-new-wrappers": "error",
  "no-implied-eval": "error",
  "no-useless-catch": "error",
  "prefer-promise-reject-errors": "error",

  // A value written and never read is a step that was meant to matter, a
  // setter's return value goes nowhere, and an expression standing alone
  // as a statement (a ternary chosen for its side effect included) is
  // usually a call or an assignment someone forgot to write.
  "no-useless-assignment": "error",
  "no-setter-return": "error",
  "no-unused-expressions": "error",
};

// Where site data is blocked, touching either storage global throws rather than
// answering a storage that refuses, and the name alone is enough: at module
// scope in `web/interview.js` it stopped the whole page from loading (d5ad668),
// and as an argument it throws before the callee's own `try` can answer it. Both
// work wherever storage is allowed, so a local run never shows either failure.
const STORAGE_MESSAGE =
  "Throws where site data is blocked. Use storageArea() from web/audio-output.js.";
const STORAGE_ACCESS = {
  "no-restricted-globals": [
    "error",
    ...["localStorage", "sessionStorage"].map((name) => ({
      name,
      message: STORAGE_MESSAGE,
    })),
  ],
  "no-restricted-properties": [
    "error",
    ...["localStorage", "sessionStorage"].map((property) => ({
      property,
      message: STORAGE_MESSAGE,
    })),
  ],
};

export default [
  {
    // `externals/` holds unrelated checkouts, is untracked, and is excluded in
    // `.git/info/exclude`. Linting it failed the gate on somebody else's JSX,
    // and a gate that is red for something outside the repository is one whose
    // red line stops being read. `claudedocs/` is a per-developer working
    // directory git ignores the same way.
    ignores: [
      "web/vendor/**",
      "node_modules/**",
      "target/**",
      "externals/**",
      "claudedocs/**",
    ],
  },
  {
    // Every file ESLint reads, with no `files` key: a file no block below
    // names still gets these rules, and fails `no-undef` loudly until a block
    // gives it the globals of where it runs, instead of passing with none.
    linterOptions: { reportUnusedDisableDirectives: "error" },
    rules: CORRECTNESS,
  },
  {
    // The browser tier. `web/*worker*.js` runs without a `window`, so the
    // worker globals ride along rather than splitting the tree in two for the
    // sake of two files.
    files: ["web/**/*.js", ".claude/skills/codetrial-review/workbench/*.js"],
    languageOptions: {
      ecmaVersion: 2023,
      sourceType: "module",
      globals: { ...globals.browser, ...globals.worker },
    },
  },
  {
    // Application code only. The review workbench under `.claude/skills` is a
    // developer tool, not a page a candidate opens. A candidate's console is
    // no log anyone reads, so output that is not a warning or an error is a
    // debugging leftover.
    files: ["web/**/*.js"],
    rules: {
      ...STORAGE_ACCESS,
      "no-console": ["error", { allow: ["warn", "error"] }],
    },
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
    files: [
      "tests/browser/**/*.js",
      "tests/harness/**/*.js",
      "tests/stubs/**/*.{js,mjs,cjs}",
      "scripts/**/*.mjs",
      "scripts/**/*.cjs",
      "eslint.config.mjs",
    ],
    languageOptions: {
      ecmaVersion: 2023,
      sourceType: "module",
      globals: { ...globals.node, ...globals.browser },
    },
  },
  {
    // A test run alone hides every other test in the file, and one skipped
    // waits for something; each says so in the disable comment's reason.
    files: ["tests/**/*.js", "tests/**/*.mjs", "scripts/**/*.test.mjs"],
    rules: {
      "no-restricted-properties": [
        "error",
        ...["test", "it", "describe", "suite"].flatMap((object) =>
          ["only", "skip", "todo"].map((property) => ({
            object,
            property,
            message:
              "Run every test; a skip needs its reason in a disable comment.",
          })),
        ),
      ],
      "no-restricted-syntax": [
        "error",
        {
          selector:
            "CallExpression[callee.name=/^(test|it|describe|suite)$/] > ObjectExpression > Property[key.name=/^(only|skip|todo)$/][value.value=true]",
          message:
            "Run every test; a skip names its reason as the option's string.",
        },
      ],
    },
  },
  {
    // `.cjs` predates `type: module` in package.json and is still CommonJS.
    files: ["scripts/**/*.cjs", "tests/stubs/**/*.cjs"],
    languageOptions: { sourceType: "commonjs" },
  },
  {
    // The desktop app's start page, a classic script loaded with `defer`
    // while the server it waits for starts.
    files: ["desktop/ui/**/*.js"],
    languageOptions: {
      ecmaVersion: 2023,
      sourceType: "script",
      globals: globals.browser,
    },
  },
  {
    // `importScripts` at the top of this worker pulls in the vendored MediaPipe
    // bundle, which announces itself as a bare global. Declared here rather
    // than silenced at the call site, so the day the vendor drops the name the
    // gate says so instead of the worker failing on a candidate's webcam.
    files: ["web/face-worker.js"],
    languageOptions: { globals: { FaceDetection: "readonly" } },
  },
  {
    // Loaded by `audioWorklet.addModule`, so it runs in the audio worklet's
    // scope, whose globals (`currentTime`, `registerProcessor`) no page has.
    files: ["web/muted-speech-worklet.js"],
    languageOptions: { globals: globals.audioWorklet },
  },
];
