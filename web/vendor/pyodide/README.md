# Pyodide Vendor Assets

CPython 3.12 compiled to WebAssembly, version 0.26.4, used by the in-browser
Python runner in `web/runners.js`. Upstream is
<https://github.com/pyodide/pyodide>, MPL-2.0.

## Files

- `pyodide.js`: the loader. The page fetches this one by name and evaluates it
  inside the runner Worker; it reads the other four itself.
- `pyodide.asm.js`: emscripten glue for the wasm module.
- `pyodide.asm.wasm`: the interpreter, 9.6 MB.
- `python_stdlib.zip`: the standard library the interpreter mounts at boot.
- `pyodide-lock.json`: the package index `loadPackage` consults. Nothing in
  CodeTrial calls `loadPackage`, but the loader reads this file unconditionally
  at startup and fails without it.

None of the five are committed. `scripts/fetch-vendor.sh` downloads them from
the base URL in `FETCH` and refuses any file whose SHA-256 does not match
`SHA256SUMS`, the same rule the other vendor directories follow.

## Why these are served from here rather than a CDN

The runner used to load all five from `cdn.jsdelivr.net`, pinning only
`pyodide.js` with an SRI hash computed in the browser. That verified the loader
and nothing else: `loadPyodide` then pulled the wasm, the glue and the stdlib
from the same CDN with no check at all, and those are the bytes that execute
candidate code.

Serving them from here closes that gap and removes `cdn.jsdelivr.net` from both
`script-src` and `connect-src` in the page's Content-Security-Policy, which is
worth more than the pinning was.

## Upgrading

Bump the version in `FETCH`, replace the five hashes in `SHA256SUMS`, and say in
the commit message where the bytes came from. The files are overwritten in place
under the same names, so `VENDOR_CACHE_CONTROL` in `src/web.rs` deliberately
does not promise immutability.
