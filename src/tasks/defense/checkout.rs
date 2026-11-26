//! The student's ca2026-mycpu checkout: the checks before Start, the stamp of
//! the work under defense, the candidate lines, and the private worktree every
//! edit and build happens in. Nothing here writes to the student's own tree.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use serde_json::Value;

use super::select::{Added, added_lines, git_name, graded, worth_asking};

/// The refs `scripts/review.py` reads the course's upstream main from.
const UPSTREAM_REFS: [&str; 2] = ["refs/remotes/upstream/main", "refs/mycpu-review/upstream"];
const NOTES: &str = "refs/notes/mycpu-review";

/// The work under defense, as the checkout names it. It names the work; it
/// proves nothing about who wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stamp {
    pub head: String,
    pub tree: String,
    pub upstream: String,
    pub student_id: String,
    pub origin: Option<String>,
    /// `STUDENT_ID` is `NONE`: the attempt is not scored.
    pub auditor: bool,
}

#[derive(Debug, Clone)]
pub struct Checkout {
    root: PathBuf,
}

impl Checkout {
    /// The checkout at `root`, which must be the top of a git work tree.
    pub fn open(root: &Path) -> Result<Self, &'static str> {
        let root = root.canonicalize().map_err(|_| "workspace_not_git")?;
        let top = git(&root, &["rev-parse", "--show-toplevel"]).map_err(|_| "workspace_not_git")?;
        let top = Path::new(&top)
            .canonicalize()
            .map_err(|_| "workspace_not_git")?;
        if top != root {
            return Err("workspace_not_git");
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The stamp of HEAD, refused when the checkout cannot be defended as it
    /// is.
    pub fn stamp(&self) -> Result<Stamp, &'static str> {
        let head = self.commit("HEAD").ok_or("workspace_not_git")?;
        let tree =
            git(&self.root, &["rev-parse", "HEAD^{tree}"]).map_err(|_| "workspace_not_git")?;
        let upstream = UPSTREAM_REFS
            .iter()
            .find_map(|name| self.commit(name))
            .ok_or("workspace_upstream")?;
        let student_id = git(&self.root, &["show", "HEAD:STUDENT_ID"])
            .ok()
            .and_then(|text| text.lines().next().map(|line| line.trim().to_owned()))
            .filter(|id| valid_student_id(id))
            .ok_or("workspace_student_id")?;
        if self.dirty_graded()? {
            return Err("workspace_dirty");
        }
        let origin = git(&self.root, &["remote", "get-url", "origin"])
            .ok()
            .map(|url| without_credentials(url.trim()));
        Ok(Stamp {
            auditor: student_id == "NONE",
            head,
            tree,
            upstream,
            student_id,
            origin,
        })
    }

    /// The lines the student's own commits add to graded files that a push
    /// review has not asked about yet.
    pub fn candidates(&self, stamp: &Stamp) -> Result<Vec<Added>, &'static str> {
        // One listing of the files each own commit touched, and the notes of
        // the commits a push review recorded on, rather than two processes a
        // commit.
        let range = format!("{}..{}", stamp.upstream, stamp.head);
        let log = git(
            &self.root,
            &[
                "log",
                "--format=%x00%H",
                "--name-only",
                "--no-renames",
                &range,
            ],
        )
        .map_err(|_| "workspace_not_git")?;
        let mut own = BTreeSet::new();
        let mut files = BTreeSet::new();
        for entry in log.split('\0').filter(|entry| !entry.trim().is_empty()) {
            let mut lines = entry.lines();
            own.extend(lines.next().map(str::to_owned));
            files.extend(
                lines
                    .filter_map(git_name)
                    .filter(|f| graded(f))
                    .map(Cow::into_owned),
            );
        }
        let noted =
            git(&self.root, &["notes", &format!("--ref={NOTES}"), "list"]).unwrap_or_default();
        let mut asked = BTreeSet::new();
        for commit in noted
            .lines()
            .filter_map(|line| line.split_whitespace().nth(1))
            .filter(|commit| own.contains(*commit))
        {
            asked.extend(self.asked(commit));
        }
        if files.is_empty() {
            return Ok(Vec::new());
        }

        // From the merge base, as the commits in `range` are: an upstream main
        // fetched since the student's last merge would otherwise show the lines
        // it changed as lines the student added.
        let since = format!("{}...{}", stamp.upstream, stamp.head);
        let mut args = vec![
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--inter-hunk-context=0",
            "-U0",
            &since,
            "--",
        ];
        args.extend(files.iter().map(String::as_str));
        let diff = git(&self.root, &args).map_err(|_| "workspace_not_git")?;
        Ok(added_lines(&diff)
            .into_iter()
            .filter(|c| worth_asking(&c.text))
            .filter(|c| !asked.contains(&(c.file.clone(), c.text.trim().to_owned())))
            .collect())
    }

    /// The source of `path` at the stamped commit.
    pub fn source(&self, head: &str, path: &str) -> Option<String> {
        git(&self.root, &["show", &format!("{head}:{path}")]).ok()
    }

    /// The spec classes of a task, by fully qualified name: each
    /// `*Spec.scala` under `<dir>/src/test/scala` at `head`, named by its file
    /// and the package it declares, read with one `git grep`.
    pub fn specs(&self, head: &str, dir: &str) -> Vec<String> {
        let tests = format!("{dir}/src/test/scala");
        let Ok(found) = git(
            &self.root,
            &[
                "grep",
                "-I",
                "--no-color",
                "-m1",
                "-e",
                "^[[:space:]]*package ",
                head,
                "--",
                &tests,
            ],
        ) else {
            return Vec::new();
        };
        let mut specs: Vec<String> = found
            .lines()
            .filter_map(|line| {
                let (file, text) = line
                    .strip_prefix(head)?
                    .strip_prefix(':')?
                    .split_once(':')?;
                let class = file.rsplit('/').next()?.strip_suffix(".scala")?;
                let package = text.trim().strip_prefix("package ")?.trim();
                let plain = package
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_'));
                (class.ends_with("Spec") && safe_name(class) && plain)
                    .then(|| format!("{package}.{class}"))
            })
            .collect();
        specs.sort();
        specs.dedup();
        specs
    }

    /// The ELF programs already built for `arch`, by file name.
    pub fn programs(&self, arch: &str) -> Vec<String> {
        let dir = self.root.join("build/sw/mycpu").join(arch);
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.ends_with(".elf") && safe_name(name))
            .collect();
        names.sort();
        names
    }

    /// The (file, trimmed quote) pairs a push review of `commit` asked about.
    fn asked(&self, commit: &str) -> Vec<(String, String)> {
        let Ok(text) = git(
            &self.root,
            &["notes", &format!("--ref={NOTES}"), "show", commit],
        ) else {
            return Vec::new();
        };
        let Ok(record) = serde_json::from_str::<Value>(&text) else {
            return Vec::new();
        };
        record["questions"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|q| {
                (
                    q["file"].as_str().unwrap_or_default().to_owned(),
                    q["quote"].as_str().unwrap_or_default().trim().to_owned(),
                )
            })
            .collect()
    }

    fn commit(&self, name: &str) -> Option<String> {
        git(
            &self.root,
            &["rev-parse", "--verify", "-q", &format!("{name}^{{commit}}")],
        )
        .ok()
        .filter(|out| !out.is_empty())
    }

    /// Whether a graded file differs from HEAD: staged, unstaged or untracked.
    fn dirty_graded(&self) -> Result<bool, &'static str> {
        let status = git(
            &self.root,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )
        .map_err(|_| "workspace_not_git")?;
        let mut entries = status.split('\0');
        while let Some(entry) = entries.next() {
            if entry.get(3..).is_some_and(graded) {
                return Ok(true);
            }
            if entry
                .as_bytes()
                .get(..2)
                .is_some_and(|xy| xy.iter().any(|&status| matches!(status, b'R' | b'C')))
                && entries.next().is_some_and(graded)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// `NONE`, or 6 to 12 ASCII letters and digits with at least one digit.
pub fn valid_student_id(id: &str) -> bool {
    id == "NONE"
        || ((6..=12).contains(&id.len())
            && id.bytes().all(|b| b.is_ascii_alphanumeric())
            && id.bytes().any(|b| b.is_ascii_digit()))
}

/// A file name the page may name back: no path, no surprises.
pub fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// A private worktree of HEAD, removed when dropped.
#[derive(Debug)]
pub struct Worktree {
    checkout: PathBuf,
    dir: PathBuf,
    /// The worktree's entry under the git directory's `worktrees/`, read when
    /// it was made: a build can rewrite the worktree's `.git` afterwards.
    registration: Option<PathBuf>,
}

impl Worktree {
    /// A detached worktree of `head` in a new temporary directory.
    pub fn create(checkout: &Checkout, head: &str) -> Result<Self, String> {
        // Nothing is pruned: that would drop the student's own worktrees whose
        // directory is away, and a new directory needs no room made for it.
        let root = checkout.root();
        let name = crate::accounts::random_token(8).map_err(|e| e.to_string())?;
        let dir = std::env::temp_dir().join(format!("codetrial-defense-{name}"));
        let path = dir.to_string_lossy().into_owned();
        git(
            root,
            &["worktree", "add", "--detach", "--quiet", &path, head],
        )?;
        let registration = registration_of(root, &dir);
        Ok(Self {
            checkout: root.to_path_buf(),
            dir,
            registration,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Replace line `line` (one-based) of `file` with `with`, or delete it
    /// when `with` is `None`. The returned guard puts the file back.
    pub fn edit(&self, file: &str, line: usize, with: Option<&str>) -> Result<Restore, String> {
        let path = self.dir.join(file);
        let outside = || "not a file of the worktree".to_owned();
        if !path.starts_with(&self.dir) || file.split('/').any(|part| part == "..") {
            return Err(outside());
        }

        // A committed link would carry the edit, and its restore, to wherever
        // it points: the student's own tree, say.
        let resolved = path.canonicalize().map_err(|_| outside())?;
        let root = self.dir.canonicalize().map_err(|_| outside())?;
        let link = std::fs::symlink_metadata(&path).map_err(|_| outside())?;
        if link.file_type().is_symlink() || !resolved.starts_with(&root) {
            return Err(outside());
        }
        let original = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let mut lines: Vec<&str> = original.split('\n').collect();
        let index = line
            .checked_sub(1)
            .filter(|&i| i < lines.len())
            .ok_or("no such line")?;
        match with {
            Some(text) => lines[index] = text,
            None => {
                lines.remove(index);
            }
        }
        std::fs::write(&path, lines.join("\n")).map_err(|e| e.to_string())?;
        Ok(Restore {
            path,
            original,
            root,
        })
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        let (checkout, dir) = (self.checkout.clone(), self.dir.clone());
        let registration = self.registration.take();
        let remove = move || {
            let path = dir.to_string_lossy().into_owned();
            if git(&checkout, &["worktree", "remove", "--force", &path]).is_err() {
                // Only this worktree's registration goes, as recorded when it
                // was made, never a prune, which would take the student's own
                // away with it.
                let _removed = std::fs::remove_dir_all(&dir);
                if let Some(entry) = registration {
                    let _unregistered = std::fs::remove_dir_all(entry);
                }
            }
        };

        // Removing a tree with its builds takes seconds, which an async worker,
        // where a room or a request lets the attempt go, must not spend.
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => drop(runtime.spawn_blocking(remove)),
            Err(_) => remove(),
        }
    }
}

/// The entry `dir`'s `.git` names under the checkout's git directory, if it
/// is exactly one entry of that directory's `worktrees/`.
fn registration_of(root: &Path, dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(dir.join(".git")).ok()?;
    let entry = PathBuf::from(text.trim().strip_prefix("gitdir: ")?)
        .canonicalize()
        .ok()?;
    let common = git(root, &["rev-parse", "--git-common-dir"]).ok()?;
    let worktrees = root.join(common).join("worktrees").canonicalize().ok()?;
    (entry.parent() == Some(worktrees.as_path())).then_some(entry)
}

/// Puts an edited file back as it was when dropped. What the run left at
/// the file's place is replaced, never written through: a build may have
/// put a link there. Nothing is written unless the file's directory is
/// still inside the worktree, the canonical `root`.
#[derive(Debug)]
pub struct Restore {
    path: PathBuf,
    original: String,
    root: PathBuf,
}

impl Drop for Restore {
    fn drop(&mut self) {
        let inside = self
            .path
            .parent()
            .and_then(|parent| parent.canonicalize().ok())
            .is_some_and(|parent| parent.starts_with(&self.root));
        if inside {
            let _unlinked = std::fs::remove_file(&self.path);
            let _written = std::fs::write(&self.path, &self.original);
        }
    }
}

/// `git -C root args`, its stdout without the trailing newline.
fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    // `git status` may otherwise refresh the index it reads, and the student's
    // index is not ours to write. The student's configuration must not change
    // the output parsed here: `git grep` numbering its lines, or a path quoted.
    let output = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "grep.lineNumber=false",
            "-c",
            "grep.column=false",
            "-c",
            "grep.fullName=false",
            "-c",
            "grep.patternType=basic",
            "-c",
            "grep.extendedRegexp=false",
            "-c",
            "core.quotePath=false",
        ])
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    while text.ends_with('\n') {
        text.pop();
    }
    Ok(text)
}

/// A remote URL as the result may keep it: a token in its user part or its
/// query stays out of the file the student hands in.
pub(super) fn without_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        // git's scp-like `user@host:path` has a user part too: whatever ends at
        // the last `@` before the host's `:`, which a local path, whose `/`
        // comes first, never has.
        let head = url.split('/').next().unwrap_or_default();
        return match head.rfind('@') {
            Some(at) if head[at..].contains(':') => url[at + 1..].to_owned(),
            _ => url.to_owned(),
        };
    };
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let (authority, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if path.is_empty() {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}/{path}")
    }
}
