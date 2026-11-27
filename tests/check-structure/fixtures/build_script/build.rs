fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let limits = "pub const BACKLOG_MOST: usize = 16;\n";
    std::fs::write(std::path::Path::new(&out).join("limits.rs"), limits).unwrap();
}
