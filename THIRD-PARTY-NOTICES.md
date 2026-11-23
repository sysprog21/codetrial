# Third-Party Notices

CodeTrial redistributes the following browser assets. Verify their
pinned files with `make verify-vendor`.

| Component | Version | License | Purpose |
|---|---:|---|---|
| [MediaPipe Face Detection](https://www.npmjs.com/package/@mediapipe/face_detection) | 0.4.1646425229 | Apache-2.0 | Local face-presence analysis |
| [LiveKit client](https://www.npmjs.com/package/livekit-client) | 2.20.0 | Apache-2.0 | Browser room client, `dist/livekit-client.umd.js` |
| [Pyodide](https://github.com/pyodide/pyodide) | 0.26.4 | MPL-2.0 | In-browser Python runner |
| CPython standard library | 3.12 | PSF-2.0 | Shipped inside Pyodide as `python_stdlib.zip` |
| [pdf.js](https://github.com/mozilla/pdf.js) | 6.3.289 | Apache-2.0 | Local text extraction from JD and resume PDFs |
| [web-tree-sitter](https://github.com/tree-sitter/tree-sitter) | 0.25.10 | MIT | Browser code parsing |
| [Tree-sitter C](https://github.com/tree-sitter/tree-sitter-c) | 0.24.2 | MIT | C grammar |
| [Tree-sitter C++](https://github.com/tree-sitter/tree-sitter-cpp) | 0.23.4 | MIT | C++ grammar |
| [Tree-sitter Java](https://github.com/tree-sitter/tree-sitter-java) | 0.23.5 | MIT | Java grammar |
| [Tree-sitter JavaScript](https://github.com/tree-sitter/tree-sitter-javascript) | 0.25.0 | MIT | JavaScript grammar |
| [Tree-sitter Python](https://github.com/tree-sitter/tree-sitter-python) | 0.25.0 | MIT | Python grammar |
| [Tree-sitter Scala](https://github.com/tree-sitter/tree-sitter-scala) | 0.24.0 | MIT | Scala grammar (Chisel in the defense panel) |
| [Three.js](https://threejs.org/) | 0.185.1 | MIT | Avatar renderer |
| [@pixiv/three-vrm](https://github.com/pixiv/three-vrm) | 3.5.5 | MIT | VRM support |
| [Prettier](https://github.com/prettier/prettier) | 3.10.1 | MIT | Editor's Format JS, `standalone.mjs` with the estree and acorn plugins |

The MediaPipe binaries, the Pyodide binaries, pdf.js, Tree-sitter and Prettier are fetched at build time from
the version or commit named in their `FETCH` manifests, and accepted only when
they match the adjacent `SHA256SUMS`. The LiveKit client and the three-vrm
bundle are committed and pinned in place.

License texts and source details are shipped with the relevant assets:

- `web/vendor/LICENSE-apache-2.0.txt` (LiveKit client, MediaPipe, pdf.js)
- `web/vendor/tree-sitter/LICENSE` (browser runtime and six grammars)
- `web/vendor/avatar/LICENSE-three.txt`
- `web/vendor/avatar/LICENSE-three-vrm.txt`
- `web/vendor/avatar/NOTICE`
- `web/vendor/avatar/README.md`
- `web/vendor/pyodide/README.md`
- `web/vendor/pdfjs/README.md`
- `web/vendor/prettier/LICENSE`, `web/vendor/prettier/THIRD-PARTY-NOTICES.md`
  (Prettier and what its bundles include, such as acorn and acorn-jsx) and
  `web/vendor/prettier/README.md`

The build also patches two Rust crates, kept in source form with their licenses:

- `vendor/livekit-net/` ([livekit-net](https://crates.io/crates/livekit-net)
  0.2.0, Apache-2.0), LiveKit's network transport, moved to tokio-tungstenite
  0.30 for patched certificate roots and given bundled roots on its proxied
  path; its `README.md` and `LICENSE` say what changed
- `vendor/tokio-tungstenite/` ([tokio-tungstenite](https://crates.io/crates/tokio-tungstenite)
  0.30.0, MIT), moved to webpki-roots 1.0.10; its `README.md` and `LICENSE`
  say what changed

The executable compiles in Mozilla's root certificates from
[webpki-roots](https://crates.io/crates/webpki-roots) 1.0.10, under
the [Community Data License Agreement - Permissive 2.0](https://cdla.dev/permissive-2-0/).

Compiler Explorer is a remote service, not bundled software. C, C++, and Java
source is sent to it only when remote test runs are enabled.
