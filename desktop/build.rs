fn main() {
    tauri_build::build();

    // tauri-build writes capability schemas into the crate's own `gen/`, a path
    // it does not let a caller move. This app grants no capability, so nothing
    // reads them, and a build should leave no output outside target/.
    let _ = std::fs::remove_dir_all("gen");
}
