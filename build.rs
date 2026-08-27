//! Makes `rust-embed` see a changed asset tree.
//!
//! The derive expands to one `include_bytes!` per file, and rustc records
//! those in dep-info, so *editing* an embedded asset already rebuilds. Files
//! that appear or disappear do not: they were never in the previous expansion,
//! so nothing names them and Cargo considers the crate fresh.
//!
//! The failure that costs a release: build once, run `scripts/fetch-vendor.sh`
//! for the first time, build again. Nothing recompiles, and the binary ships
//! without the 37 MB of vendored runtime. It boots, serves the page, and then
//! fails in the browser with a Python runtime that will not start.
//!
//! Cargo walks a directory given to `rerun-if-changed` in full, so the one
//! line covers the tree.
fn main() {
    println!("cargo:rerun-if-changed=web");
}
