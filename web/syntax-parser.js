const RUNTIME_MODULE = new URL(
  "./vendor/tree-sitter/tree-sitter.js",
  import.meta.url,
);
const RUNTIME_WASM = new URL(
  "./vendor/tree-sitter/tree-sitter.wasm",
  import.meta.url,
);
const GRAMMARS = {
  c: "tree-sitter-c",
  cpp: "tree-sitter-cpp",
  java: "tree-sitter-java",
  javascript: "tree-sitter-javascript",
  python: "tree-sitter-python",
  scala: "tree-sitter-scala",
};
const PARSE_BUDGET_MS = 25;

function requireLanguage(language) {
  if (!Object.hasOwn(GRAMMARS, language))
    throw new RangeError("Unsupported language: " + language);
}

async function fetchBytes(url) {
  const response = await fetch(url);
  if (!response.ok)
    throw new Error(
      "Cannot load syntax parser: " + response.status + " " + url,
    );
  return new Uint8Array(await response.arrayBuffer());
}

export function createSyntaxParser({
  loadModule = (url) => import(url.href),
  loadBytes = fetchBytes,
  now = () => performance.now(),
} = {}) {
  let runtime;
  const pending = new Map();
  const parsers = new Map();

  function loadRuntime() {
    runtime ??= Promise.all([
      loadModule(RUNTIME_MODULE),
      loadBytes(RUNTIME_WASM),
    ]).then(async ([module, bytes]) => {
      await module.Parser.init({ wasmBinary: bytes });
      return module;
    });
    return runtime;
  }

  async function prepareLanguage(language) {
    requireLanguage(language);
    // Cache in-flight loads too, so rapid switches do not download twice.
    if (!pending.has(language)) {
      const name = GRAMMARS[language];
      const url = new URL(
        "./vendor/" + name + "/" + name + ".wasm",
        import.meta.url,
      );
      pending.set(
        language,
        Promise.all([loadRuntime(), loadBytes(url)]).then(
          async ([module, bytes]) => {
            const grammar = await module.Language.load(bytes);
            const parser = new module.Parser();
            try {
              parser.setLanguage(grammar);
            } catch (error) {
              parser.delete();
              throw error;
            }
            parsers.set(language, parser);
          },
        ),
      );
    }
    return pending.get(language);
  }

  // The caller owns a returned tree and must delete it after reading it.
  // `budgetMs` bounds the parse: the editor's keystroke budget by default, a
  // longer one for a caller that parses a short snippet once.
  function parseSyntax(code, language, budgetMs = PARSE_BUDGET_MS) {
    requireLanguage(language);
    const parser = parsers.get(language);
    if (!parser) return null;
    const deadline = now() + budgetMs;
    const tree = parser.parse(code, null, {
      progressCallback: () => now() >= deadline,
    });
    // Cancellation otherwise resumes the old source on the next parse.
    if (!tree) parser.reset();
    return tree;
  }

  return { prepareLanguage, parseSyntax };
}

export const { prepareLanguage, parseSyntax } = createSyntaxParser();
