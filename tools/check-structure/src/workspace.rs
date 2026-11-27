//! The Cargo workspace around the current directory, as `cargo metadata`
//! describes it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::Error;

pub struct Workspace {
    pub root: PathBuf,
    pub target_directory: PathBuf,
    pub packages: Vec<Package>,
}

pub struct Package {
    pub name: String,
    pub version: String,
    /// How code names the package's library: `my_crate` for `my-crate`.
    pub crate_name: String,
    /// Relative to the workspace's root, `/`-separated; empty for the root
    /// itself.
    pub directory: String,
    pub targets: Vec<Target>,
}

pub struct Target {
    pub kind: TargetKind,
    /// The crate's root file, relative to the workspace's root.
    pub root: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TargetKind {
    Library,
    Binary,
    Example,
    Test,
    Bench,
    BuildScript,
}

impl TargetKind {
    pub fn is_test(self) -> bool {
        matches!(self, TargetKind::Test | TargetKind::Bench)
    }
}

impl Workspace {
    pub fn discover() -> Result<Workspace, Error> {
        let cwd = std::env::current_dir()
            .map_err(|e| Error::new(format!("cannot read current directory: {e}")))?;
        Workspace::discover_in(&cwd)
    }

    pub fn discover_in(dir: &Path) -> Result<Workspace, Error> {
        let output = Command::new("cargo")
            .args(["metadata", "--format-version", "1", "--no-deps"])
            .current_dir(dir)
            .output()
            .map_err(|e| Error::new(format!("cannot run cargo: {e}")))?;
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr);
            return Err(Error::new(format!(
                "not inside a Cargo workspace: cargo metadata says: {}",
                message.trim()
            )));
        }
        let metadata: Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| Error::new(format!("cargo metadata's output is not JSON: {e}")))?;
        Workspace::from_metadata(&metadata)
    }

    fn from_metadata(metadata: &Value) -> Result<Workspace, Error> {
        let root = PathBuf::from(text(metadata, "workspace_root")?);
        let target_directory = PathBuf::from(text(metadata, "target_directory")?);
        let packages = list(metadata, "packages")?
            .iter()
            .map(|package| Package::from_metadata(package, &root))
            .collect::<Result<_, _>>()?;
        Ok(Workspace {
            root,
            target_directory,
            packages,
        })
    }

    /// The files whose change can change the index: the Rust sources and
    /// manifest of every package the workspace holds or builds from a path,
    /// and the lock file. A directory that has a manifest of its own and is not
    /// one of those packages (`desktop/`, a fixture crate, a checkout under
    /// `externals/`) is another project's, and an edit there changes nothing
    /// the index reads; without this the root package's walk hashed it all.
    pub fn source_files(&self) -> Result<Vec<String>, Error> {
        let mut files = vec!["Cargo.toml".to_owned(), "Cargo.lock".to_owned()];

        // `cargo metadata` can fail where the index would not be wrong (an
        // offline registry that lacks a package): every manifest then counts,
        // which rebuilds more often and never less.
        let local = self.local_directories();
        for package in &self.packages {
            let directory = self.root.join(&package.directory);
            collect_sources(&directory, self, local.as_ref(), &mut files)?;
        }
        for directory in local.iter().flatten() {
            collect_sources(&self.root.join(directory), self, local.as_ref(), &mut files)?;
        }
        files.sort();
        files.dedup();
        Ok(files)
    }

    /// The directories of the packages cargo resolves from a path, relative
    /// to the root: the members, path dependencies and what `[patch]` points
    /// at. `None` when cargo cannot say.
    fn local_directories(&self) -> Option<BTreeSet<String>> {
        let output = Command::new("cargo")
            .args(["metadata", "--format-version", "1", "--offline"])
            .current_dir(&self.root)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let metadata: Value = serde_json::from_slice(&output.stdout).ok()?;
        Some(local_directories(&metadata, &self.root))
    }
}

/// The directory of each package of `metadata` that has no registry or git
/// source, relative to `root`; a package outside `root` has no place in the
/// stamp, which names files by their path below it.
fn local_directories(metadata: &Value, root: &Path) -> BTreeSet<String> {
    metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|package| package["source"].is_null())
        .filter_map(|package| package["manifest_path"].as_str())
        .filter_map(|manifest| relative(Path::new(manifest), root).ok())
        .map(|manifest| {
            manifest
                .rsplit_once('/')
                .map_or("", |(directory, _)| directory)
                .to_owned()
        })
        .collect()
}

impl Package {
    fn from_metadata(package: &Value, root: &Path) -> Result<Package, Error> {
        let name = text(package, "name")?.to_owned();
        let manifest = relative(Path::new(text(package, "manifest_path")?), root)?;
        let directory = manifest
            .rsplit_once('/')
            .map_or("", |(directory, _)| directory)
            .to_owned();
        let mut targets = Vec::new();
        let mut library_name = None;
        for target in list(package, "targets")? {
            let kind = target_kind(target)?;
            if kind == TargetKind::Library {
                library_name = Some(text(target, "name")?.to_owned());
            }
            let root = relative(Path::new(text(target, "src_path")?), root)?;
            targets.push(Target { kind, root });
        }
        Ok(Package {
            crate_name: library_name.unwrap_or_else(|| name.replace('-', "_")),
            version: text(package, "version")?.to_owned(),
            name,
            directory,
            targets,
        })
    }
}

fn target_kind(target: &Value) -> Result<TargetKind, Error> {
    let kinds = list(target, "kind")?;
    let first = kinds.first().and_then(Value::as_str).unwrap_or_default();
    Ok(match first {
        "bin" => TargetKind::Binary,
        "example" => TargetKind::Example,
        "test" => TargetKind::Test,
        "bench" => TargetKind::Bench,
        "custom-build" => TargetKind::BuildScript,
        "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro" => TargetKind::Library,
        other => {
            return Err(Error::new(format!(
                "cargo metadata has a target of an unknown kind: {other:?}"
            )));
        }
    })
}

fn collect_sources(
    directory: &Path,
    workspace: &Workspace,
    local: Option<&BTreeSet<String>>,
    files: &mut Vec<String>,
) -> Result<(), Error> {
    let unreadable = |e| Error::new(format!("cannot read {}: {e}", directory.display()));
    for entry in std::fs::read_dir(directory).map_err(unreadable)? {
        let entry = entry.map_err(unreadable)?;
        let path = entry.path();
        let hidden = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'));
        if hidden || path == workspace.target_directory {
            continue;
        }
        // Not through symbolic links, which can loop.
        let file_type = entry.file_type().map_err(unreadable)?;
        if file_type.is_dir() {
            if belongs_to_another_project(&path, workspace, local) {
                continue;
            }
            collect_sources(&path, workspace, local, files)?;
        } else if file_type.is_file()
            && (path.extension().is_some_and(|extension| extension == "rs")
                || path.file_name().is_some_and(|name| name == "Cargo.toml"))
        {
            files.push(relative(&path, &workspace.root)?);
        }
    }
    Ok(())
}

/// Whether `directory` holds a package that is not one of `local`: it has a
/// manifest and cargo does not resolve it from a path. Where `local` is
/// unknown, nothing is another project's.
fn belongs_to_another_project(
    directory: &Path,
    workspace: &Workspace,
    local: Option<&BTreeSet<String>>,
) -> bool {
    let Some(local) = local else {
        return false;
    };
    directory.join("Cargo.toml").is_file()
        && relative(directory, &workspace.root).is_ok_and(|path| !local.contains(&path))
}

fn relative(path: &Path, root: &Path) -> Result<String, Error> {
    let inside = path.strip_prefix(root).map_err(|_| {
        Error::new(format!(
            "{} is outside the workspace's root {}",
            path.display(),
            root.display()
        ))
    })?;
    let parts: Vec<_> = inside
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect();
    Ok(parts.join("/"))
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, Error> {
    value[key].as_str().ok_or_else(|| missing(key))
}

fn list<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, Error> {
    value[key].as_array().ok_or_else(|| missing(key))
}

fn missing(key: &str) -> Error {
    Error::new(format!("cargo metadata's output has no {key:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_packages_their_directories_and_targets() {
        let metadata = serde_json::json!({
            "workspace_root": "/w",
            "target_directory": "/w/target",
            "packages": [{
                "name": "my-tool",
                "version": "0.2.0",
                "manifest_path": "/w/tools/my-tool/Cargo.toml",
                "targets": [
                    {"kind": ["lib"], "name": "my_tool", "src_path": "/w/tools/my-tool/src/lib.rs"},
                    {"kind": ["test"], "name": "it", "src_path": "/w/tools/my-tool/tests/it.rs"}
                ]
            }, {
                "name": "root-bin",
                "version": "1.0.0",
                "manifest_path": "/w/Cargo.toml",
                "targets": [{"kind": ["bin"], "name": "root-bin", "src_path": "/w/src/main.rs"}]
            }]
        });
        let workspace = Workspace::from_metadata(&metadata).unwrap();
        let tool = &workspace.packages[0];
        assert_eq!(
            (tool.directory.as_str(), tool.crate_name.as_str()),
            ("tools/my-tool", "my_tool")
        );
        assert_eq!(tool.targets[1].kind, TargetKind::Test);
        assert_eq!(tool.targets[1].root, "tools/my-tool/tests/it.rs");
        let root_bin = &workspace.packages[1];
        assert_eq!(
            (root_bin.directory.as_str(), root_bin.crate_name.as_str()),
            ("", "root_bin")
        );
    }

    #[test]
    fn only_packages_without_a_registry_or_git_source_are_local() {
        let metadata = serde_json::json!({"packages": [
            {"source": null, "manifest_path": "/w/Cargo.toml"},
            {"source": null, "manifest_path": "/w/vendor/net/Cargo.toml"},
            {"source": null, "manifest_path": "/elsewhere/Cargo.toml"},
            {"source": "registry+https://github.com/rust-lang/crates.io-index",
             "manifest_path": "/home/u/.cargo/registry/src/x/Cargo.toml"}
        ]});
        let local: Vec<String> = local_directories(&metadata, Path::new("/w"))
            .into_iter()
            .collect();
        assert_eq!(local, ["", "vendor/net"]);
    }

    #[test]
    fn the_stamp_skips_a_directory_that_is_another_project_and_keeps_a_local_one() {
        let root = std::env::temp_dir().join(format!("check-structure-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for file in [
            "Cargo.toml",
            "src/lib.rs",
            "plain/mod_like.rs",
            "vendor/kept/Cargo.toml",
            "vendor/kept/src/lib.rs",
            "desktop/Cargo.toml",
            "desktop/src/main.rs",
            "tools/x/tests/fixtures/f/Cargo.toml",
            "tools/x/tests/fixtures/f/src/lib.rs",
        ] {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        let workspace = Workspace {
            target_directory: root.join("target"),
            root: root.clone(),
            packages: Vec::new(),
        };
        let walk = |local: Option<&BTreeSet<String>>| {
            let mut files = Vec::new();
            collect_sources(&root, &workspace, local, &mut files).unwrap();
            files.sort();
            files
        };
        let local: BTreeSet<String> = ["", "vendor/kept"].map(str::to_owned).into();
        assert_eq!(
            walk(Some(&local)),
            [
                "Cargo.toml",
                "plain/mod_like.rs",
                "src/lib.rs",
                "vendor/kept/Cargo.toml",
                "vendor/kept/src/lib.rs"
            ]
        );
        // With no answer from cargo, everything counts: never a missed rebuild.
        assert_eq!(walk(None).len(), 9);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
