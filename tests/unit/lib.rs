//! The `tests` module of `src/lib.rs`, which declares this file by path.
//! Everything here reaches into `src/lib.rs` through `super`, so it is a unit
//! test and not an integration test: private items are in scope.

/// The empty path is what a `PathBuf` defaults to, and joining a file name
/// onto it yields that name alone, which resolves against the working
/// directory. That is the fallback this function has for when the
/// executable cannot be located, not the answer it gives when it can, and
/// the difference decides where a released binary looks for its config.
#[test]
fn exe_dir_names_the_folder_the_running_binary_sits_in() {
    let exe = std::env::current_exe().expect("a running test binary has a path");
    assert_eq!(
        super::exe_dir(),
        exe.parent().expect("an executable sits in a folder")
    );
}
