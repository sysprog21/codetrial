# Tree-sitter browser assets

The browser uses web-tree-sitter 0.25.10 and, for the languages the Rust
backend also parses, the same grammar versions as its Cargo.toml; Scala is the
browser's alone, for the Chisel lines of a defense:

| Grammar | Version | Upstream |
| --- | --- | --- |
| C | 0.24.2 | https://github.com/tree-sitter/tree-sitter-c |
| C++ | 0.23.4 | https://github.com/tree-sitter/tree-sitter-cpp |
| Java | 0.23.5 | https://github.com/tree-sitter/tree-sitter-java |
| JavaScript | 0.25.0 | https://github.com/tree-sitter/tree-sitter-javascript |
| Python | 0.25.0 | https://github.com/tree-sitter/tree-sitter-python |
| Scala | 0.24.0 | https://github.com/tree-sitter/tree-sitter-scala |

The runtime is fetched here; each grammar is fetched into its sibling
tree-sitter-LANGUAGE directory. FETCH records the versioned source URL,
SHA256SUMS pins the bytes. The LICENSE here covers the runtime and all six
grammars, preserving each package's copyright notice.

Run scripts/fetch-vendor.sh to download and scripts/verify-vendor.sh to verify.
Generated JavaScript and WebAssembly files are ignored by Git. Updating an
asset requires updating its FETCH URL, checksum and license together.

Runtime: https://github.com/tree-sitter/tree-sitter

C's WebAssembly file comes from its v0.24.2 GitHub release; its copyright notice
comes from that tag's LICENSE. The other files and notices come from the pinned
npm packages through jsDelivr. No build step is required in the browser.
