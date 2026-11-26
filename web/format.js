// JavaScript formatting for the interview editor's Format JS button.
// Prettier is imported on first use rather than with the page, so a candidate
// who never formats never downloads it.
const MODULES = [
  "./vendor/prettier/standalone.mjs",
  "./vendor/prettier-plugins/estree.mjs",
  "./vendor/prettier-plugins/acorn.mjs",
].map((path) => new URL(path, import.meta.url));

/// A formatter with its own copy of Prettier, loaded on its first call.
///
/// The returned function formats `code` as one whole buffer and answers rather
/// than throws, so the caller has one shape to handle. `cursor` is where
/// `caret` lands in the formatted text; Prettier maps one offset, so a
/// selection collapses to where its start went. Prettier's other options stay
/// at their defaults, so the output depends only on the code, the indentation
/// the lobby chose and the pinned version.
export function createJavaScriptFormatter(load = (url) => import(url.href)) {
  // A failed import is cached by the browser under its URL, so a retry asks
  // for that module under `?retry=n` (the server ignores the query) and keeps
  // the modules that loaded.
  const slots = MODULES.map(() => ({ attempt: 0, loading: null }));
  function importModule(slot, url) {
    slot.loading ??= load(
      slot.attempt ? new URL(`?retry=${slot.attempt}`, url) : url,
    ).catch((error) => {
      slot.attempt += 1;
      slot.loading = null;
      throw error;
    });
    return slot.loading;
  }
  function loaded() {
    return Promise.all(
      MODULES.map((url, index) => importModule(slots[index], url)),
    ).then(([prettier, estree, acorn]) => ({
      prettier,
      plugins: [estree, acorn],
    }));
  }

  return async function formatJavaScript(code, caret, indent) {
    let formatter;
    try {
      formatter = await loaded();
    } catch {
      return { ok: false, message: "The formatter could not be loaded" };
    }
    try {
      const { formatted, cursorOffset } =
        await formatter.prettier.formatWithCursor(code, {
          parser: "acorn",
          plugins: formatter.plugins,
          cursorOffset: caret,
          useTabs: indent === "\t",
          tabWidth: indent === "\t" ? 4 : indent.length,
        });
      // Prettier answers -1 when there was nothing to map the caret through,
      // as for a buffer of only whitespace.
      const cursor =
        cursorOffset < 0 ? Math.min(caret, formatted.length) : cursorOffset;
      return { ok: true, value: formatted, cursor };
    } catch (error) {
      // The parser's first line ends in a terse "(8:1)"; the place is said in
      // words instead. The code frame after that line repeats the candidate's
      // code back to them, so it is left out.
      const reason = String(error?.message)
        .split("\n")[0]
        .replace(/ \(\d+:\d+\)$/, "");
      const start = error?.loc?.start;
      const at = start ? ` at line ${start.line}, column ${start.column}` : "";
      return { ok: false, message: `${reason}${at}` };
    }
  };
}
