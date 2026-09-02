//! Makes `rust-embed` see a changed asset tree, and installs the git hooks.
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

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=web");
    install_git_hooks();
}

/// Links `scripts/git-*.sh` into the hooks directory of a checkout.
///
/// A hook nobody installed is a hook nobody runs, and the first person to find
/// that out is whoever reads a commit message the rules would have caught. A
/// build is the one step every contributor takes, so it is where installation
/// belongs; `make hooks` stays for anyone who wants it on its own.
///
/// Never fails the build. A checkout can be read-only, a hooks directory can
/// belong to somebody else's tooling, and a Windows build has no `sh`. None of
/// that is a reason to refuse to compile, so a failure is a warning and the
/// build goes on.
fn install_git_hooks() {
    let root = std::env::var_os("CARGO_MANIFEST_DIR").map(std::path::PathBuf::from);
    let Some(root) = root else { return };

    // A worktree has a `.git` file rather than a directory, and both mean the
    // same thing here: this is a checkout, so it has hooks worth installing.
    if !root.join(".git").exists() {
        return;
    }

    // Watched so that adding a hook script, or deleting an installed hook,
    // reruns this. Without the second path Cargo would keep the build script
    // fresh and a removed hook would stay removed until something else forced a
    // rerun.
    //
    // The price, stated because it will otherwise look like a bug. A build
    // script rerun makes the crate dirty, and Cargo follows the installed
    // symlinks, so editing a hook script counts as changing this directory:
    // measured at 10.3s against 0.68s for a no-op build. The first build after
    // a fresh clone also pays one extra rebuild, because installing the hooks
    // is itself a change to the directory being watched. Both are the price of
    // a hook that comes back after somebody deletes it; `make hooks` is the
    // same install without the watch.
    println!("cargo:rerun-if-changed=scripts/install-git-hooks.sh");

    // Asked of git rather than assumed to be `.git/hooks`: a linked worktree
    // keeps its hooks with the common directory, and core.hooksPath moves them
    // anywhere. Watching the wrong directory is a watch that never fires.
    //
    // Not the whole of scripts/, though adding a hook there is the one change
    // this does not notice. That directory holds every gate script in the
    // repository and each edit to one would rebuild the binary; `make hooks`
    // covers the rare case at no standing cost.
    if let Some(hooks) = hooks_dir(&root)
        && hooks.is_dir()
    {
        println!("cargo:rerun-if-changed={}", hooks.display());
    }

    let installer = root.join("scripts/install-git-hooks.sh");
    if !installer.is_file() {
        return;
    }

    // Git for Windows carries a shell, but nothing guarantees it is on PATH,
    // and a Windows symlink needs a privilege the build does not have. Silent
    // rather than a warning: the release job builds a Windows target on every
    // push, and a warning nobody can act on teaches people to skim them.
    if cfg!(windows) {
        return;
    }

    match Command::new("sh")
        .arg(&installer)
        .current_dir(&root)
        .output()
    {
        Ok(output) if output.status.success() => {
            // Cargo swallows a build script's stdout, which is right for the
            // four "HOOK installed" lines and wrong for the two the installer
            // prints when it needs a person: a hook it refused to overwrite,
            // and a core.hooksPath that sends hooks somewhere else. Those two
            // reach the terminal or nothing does.
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let line = line.trim();
                if line.starts_with("KEEP") || line.starts_with("NOTE") {
                    println!("cargo::warning=git hooks: {line}");
                }
            }
        }
        Ok(output) => warn_hooks(&String::from_utf8_lossy(&output.stderr)),
        Err(error) => warn_hooks(&error.to_string()),
    }
}

/// Where git keeps this checkout's hooks, which is `.git/hooks` only in the
/// simple case.
fn hooks_dir(root: &std::path::Path) -> Option<std::path::PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-path", "hooks"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    Some(std::path::PathBuf::from(path.trim()))
}

fn warn_hooks(reason: &str) {
    let reason = reason.trim().replace('\n', "; ");
    let reason = if reason.is_empty() {
        "scripts/install-git-hooks.sh failed".to_string()
    } else {
        reason
    };
    println!("cargo::warning=could not install the git hooks ({reason}); run make hooks");
}
