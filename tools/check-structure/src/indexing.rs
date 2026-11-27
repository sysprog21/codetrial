//! rust-analyzer's SCIP index of the workspace, cached per workspace and
//! configuration under `target/check-structure/` and rebuilt only when a source
//! file changes.
//!
//! Indexing builds what the code needs at compile time, as `cargo check
//! --compile-time-deps` does: `rust-analyzer scip` compiles and runs the build
//! scripts, and compiles the proc macros and what they and the build scripts
//! depend on, into the workspace's target directory. The rest it only reads.
//! `scip` always does this (`cargo.buildScripts.enable` is the language
//! server's), and the index needs it to read the code as the compiler does:
//! which `cfg`s a build script sets, what it writes to `OUT_DIR`, what a proc
//! macro expands to. The lints see such generated items where the workspace's
//! own code uses them. The first index pays for the build; later ones, and the
//! next `cargo check`, find it done. Generated code is as fresh as the last
//! index: a change to a build script's other inputs alone does not index again.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::configuration::Configuration;
use crate::workspace::Workspace;
use crate::{Error, TOOL};

/// The index of one configuration of `workspace`.
pub fn index(workspace: &Workspace, configuration: &Configuration) -> Result<Vec<u8>, Error> {
    configuration.check(&workspace.root)?;
    let cfgs = configuration.cfgs(&workspace.root);
    let directory = cache_dir(workspace, configuration, &cfgs);
    let index = directory.join("index.scip");
    let stamp_path = directory.join("index.stamp");
    // rust-analyzer's settings: the target and features, none by default.
    let ra_config = configuration.settings().to_string();
    let stamp = stamp(workspace, &ra_config, &cfgs)?;
    let last_stamp = std::fs::read_to_string(&stamp_path).ok();
    if last_stamp.as_deref() != Some(stamp.as_str()) || !index.exists() {
        if configuration.is_default() {
            eprintln!("{TOOL}: indexing workspace...");
        } else {
            eprintln!("{TOOL}: indexing workspace with {configuration}...");
        }
        std::fs::create_dir_all(&directory).map_err(|e| io_error(&directory, e))?;
        remove_if_present(&stamp_path)?;
        let config = directory.join("ra-config.json");
        std::fs::write(&config, &ra_config).map_err(|e| io_error(&config, e))?;
        run_rust_analyzer(&workspace.root, &config, &index)?;
        std::fs::write(&stamp_path, &stamp).map_err(|e| io_error(&stamp_path, e))?;
    }
    std::fs::read(&index).map_err(|e| io_error(&index, e))
}

/// Where the index of `configuration` of `workspace` is kept, when cargo
/// builds it with `cfgs`: each configuration's apart, whether the flags or
/// cargo's configuration choose it, so switching between them rebuilds
/// neither.
fn cache_dir(workspace: &Workspace, configuration: &Configuration, cfgs: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    hasher.write(workspace.root.to_string_lossy().as_bytes());
    configuration.hash(&mut hasher);
    hasher.write(cfgs.as_bytes());
    workspace
        .target_directory
        .join(TOOL)
        .join(format!("{:016x}", hasher.finish()))
}

/// What the index is built from: rust-analyzer, its settings, the `cfg`s
/// cargo builds with, and the sources.
fn stamp(workspace: &Workspace, ra_config: &str, cfgs: &str) -> Result<String, Error> {
    let version = Command::new("rust-analyzer")
        .arg("--version")
        .output()
        .map_err(missing_rust_analyzer)?;
    let mut stamp = String::from_utf8_lossy(&version.stdout).into_owned();
    stamp += ra_config;
    stamp.push('\n');
    stamp += cfgs;
    for file in workspace.source_files()? {
        let hash = match std::fs::read(workspace.root.join(&file)) {
            Ok(bytes) => {
                let mut hasher = DefaultHasher::new();
                hasher.write(&bytes);
                format!("{:016x}", hasher.finish())
            }
            Err(e) if e.kind() == ErrorKind::NotFound => "missing".to_owned(),
            Err(e) => return Err(io_error(&workspace.root.join(&file), e)),
        };
        stamp += &format!("{hash} {file}\n");
    }
    Ok(stamp)
}

fn run_rust_analyzer(root: &Path, config: &Path, index: &Path) -> Result<(), Error> {
    let mut command = Command::new("rust-analyzer");
    command
        .arg("scip")
        .arg(root)
        .arg("--config-path")
        .arg(config)
        .arg("--output")
        .arg(index);
    let output = crate::progress::while_waiting(
        "rust-analyzer is indexing (a cold index builds build scripts and takes minutes)",
        || command.output(),
    )
    .map_err(missing_rust_analyzer)?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let last_lines: Vec<&str> = stderr.lines().rev().take(20).collect();
    let last_lines: Vec<&str> = last_lines.into_iter().rev().collect();
    Err(Error::new(format!(
        "rust-analyzer scip failed ({}):\n{}",
        output.status,
        last_lines.join("\n")
    )))
}

fn missing_rust_analyzer(error: std::io::Error) -> Error {
    if error.kind() == ErrorKind::NotFound {
        Error::new(format!(
            "rust-analyzer is not on PATH: {TOOL} reads the index that `rust-analyzer scip` writes"
        ))
    } else {
        Error::new(format!("cannot run rust-analyzer: {error}"))
    }
}

fn remove_if_present(path: &Path) -> Result<(), Error> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(io_error(path, e)),
        _ => Ok(()),
    }
}

fn io_error(path: &Path, error: std::io::Error) -> Error {
    Error::new(format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_configuration_keeps_its_index_apart_whether_flags_or_cargo_choose_it() {
        let workspace = Workspace {
            root: PathBuf::from("/w"),
            target_directory: PathBuf::from("/w/target"),
            packages: Vec::new(),
        };
        let host = Configuration::default();
        let mut windows = Configuration::default();
        windows.target("x86_64-pc-windows-msvc").unwrap();
        let directories = [
            cache_dir(&workspace, &host, "unix\n"),
            // `RUSTFLAGS='--cfg shuttle'`
            cache_dir(&workspace, &host, "shuttle\nunix\n"),
            // `build.target`, and `--target`
            cache_dir(&workspace, &host, "windows\n"),
            cache_dir(&workspace, &windows, "windows\n"),
        ];
        for (index, directory) in directories.iter().enumerate() {
            assert!(directory.starts_with("/w/target/check-structure"));
            assert!(!directories[..index].contains(directory), "{index}");
        }
        assert_eq!(cache_dir(&workspace, &host, "unix\n"), directories[0]);
    }
}
