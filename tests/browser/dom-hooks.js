// Resolves the absolute specifiers `web/replay.js` uses. The page is served from
// the web root, so `/lib.js` is `web/lib.js`; node has no import map, and this is
// the smallest thing that lets the module be imported at all.

import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join } from "node:path";

const WEB = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "web");

export function resolve(specifier, context, next) {
  // A query is carried through rather than resolved away. It is how a test asks
  // for a second instance of the page: ESM caches by URL, and re-running the
  // module scope is the only way to see what a page does when one of its ids is
  // missing.
  const [path, query] = specifier.split("?");
  if (path.startsWith("/") && path.endsWith(".js")) {
    const url = pathToFileURL(join(WEB, path));
    if (query) url.search = query;
    return { url: url.href, shortCircuit: true };
  }
  return next(specifier, context);
}
