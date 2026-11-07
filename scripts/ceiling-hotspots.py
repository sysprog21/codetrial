#!/usr/bin/env python3
"""Rank oversized files by how often they change times how large they are.

`scripts/check-rust-style.py` says which Rust files are over the size ceiling;
it does not say which to split first. A big file nobody touches costs nothing,
and a big file every change goes through costs on every change. This prints,
per top-level directory, each file with its lines, its ceiling, the commits
that touched it in a window, and score = commits x lines.

  default             every Rust file the style checker sees above its default
                      ceiling, with the limit it derives from HEAD
  --under DIR         instead, every tracked file under DIR (a directory such as
                      `web`) with at least --min-lines lines; there is no ceiling
                      column to fill, so it shows `-`. `--suffix .js` keeps only
                      those extensions and `--exclude web/vendor/` drops a prefix;
                      both repeat

Commits are counted from one `git log --name-only` over the paths: a path is
counted once per commit that changed it, a merge commit shows no files and so
counts nothing, and a file that was renamed counts only under its current name.
Lines come from the working tree, the commits from history; the header says
which revision the history was read at. The window is git's own date parser, so
a spec git cannot read is not rejected. Nothing is written and the exit status
is 0, or 2 when git fails, the arguments are unusable, or `--under` matches no
file.
"""

import argparse
import importlib.util
import subprocess
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SINCE = "90.days"

# The gate's own line counts and the limits it derives from HEAD. The gate run
# with `--baseline` can be stricter: it also takes the smallest size since that
# ref, so a file that shrank on a branch and grew back shows a larger ceiling
# here than the one the gate enforces.
_SPEC = importlib.util.spec_from_file_location(
    "check_rust_style", Path(__file__).with_name("check-rust-style.py")
)
STYLE = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(STYLE)


class GitError(Exception):
    """git exited non-zero; the message is its standard error."""


def git(repo, *args):
    done = subprocess.run(
        ["git", "-C", str(repo), *args], capture_output=True, text=True, check=False
    )
    if done.returncode != 0:
        raise GitError(done.stderr.strip() or f"git {' '.join(args)} failed")
    return done.stdout


def line_count(path):
    """Lines the way `check-rust-style.py` counts them, so a row's `lines` can be
    compared with its ceiling: `splitlines`, which counts an unterminated last
    line that `wc -l` would not."""
    return len(path.read_text(errors="replace").splitlines())


def commit_counts(repo, since, paths):
    """Commits in the window that changed each path, one count per path."""
    if not paths:
        return Counter()
    # NUL-separated and unquoted, because git quotes a name with non-ASCII bytes
    # and the quoted form never matches the key. `--full-history` because a
    # path-limited log otherwise drops a branch that changed a file and reverted
    # it before merging, and that churn is the cost being measured.
    out = git(
        repo,
        "--literal-pathspecs",
        "-c",
        "core.quotePath=false",
        "log",
        f"--since={since}",
        "--full-history",
        "--name-only",
        "-z",
        "--format=",
        "--",
        *paths,
    )
    return Counter(name for name in out.split("\0") if name)


def ceiling_rows(repo):
    """{path: ceiling} for each Rust file over the default ceiling. A file with
    no committed version, or one at or under the default in HEAD, gets the
    default, as the gate gives it."""
    current = STYLE.sizes(repo)
    limits = STYLE.git_ceilings(repo, current)
    return {
        name: limits.get(name, STYLE.DEFAULT_CEILING)
        for name, lines in sorted(current.items())
        if lines > STYLE.DEFAULT_CEILING
    }


def under_rows(repo, directory, min_lines, suffixes=(), excludes=()):
    """{path: None} for each tracked file under directory with enough lines."""
    names = git(repo, "ls-files", "-z", "--", directory).split("\0")
    rows = {}
    for name in filter(None, names):
        if suffixes and not name.endswith(tuple(suffixes)):
            continue
        if name.startswith(tuple(excludes)):
            continue
        path = repo / name
        if path.is_file() and line_count(path) >= min_lines:
            rows[name] = None
    return rows


def rank(repo, rows, since):
    """Rows as dicts, highest score first; ties break on path for stable output."""
    counts = commit_counts(repo, since, list(rows))
    ranked = []
    for name, ceiling in rows.items():
        lines = line_count(repo / name)
        commits = counts[name]
        ranked.append(
            {
                "file": name,
                "lines": lines,
                "ceiling": ceiling,
                "commits": commits,
                "score": commits * lines,
            }
        )
    ranked.sort(key=lambda row: (-row["score"], row["file"]))
    return ranked


def render(ranked):
    """One table per top-level directory, in the order the directories first rank."""
    groups = {}
    for row in ranked:
        head, sep, _ = row["file"].partition("/")
        groups.setdefault(head if sep else ".", []).append(row)
    out = []
    for group, rows in groups.items():
        out.append(f"## {group}/ ({len(rows)} files)")
        width = max(len(row["file"]) for row in rows)
        out.append(
            f"{'file'.ljust(width)}  {'lines':>6}  {'ceiling':>7}  {'commits':>7}  {'score':>9}"
        )
        for row in rows:
            ceiling = "-" if row["ceiling"] is None else str(row["ceiling"])
            out.append(
                f"{row['file'].ljust(width)}  {row['lines']:>6}  {ceiling:>7}"
                f"  {row['commits']:>7}  {row['score']:>9}"
            )
        out.append("")
    return "\n".join(out)


def parser():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--repo", type=Path, default=ROOT, help="repository root")
    ap.add_argument("--since", default=DEFAULT_SINCE, help="git date window")
    ap.add_argument("--under", help="rank tracked files under this directory")
    ap.add_argument("--min-lines", type=int, default=1501, help="with --under")
    ap.add_argument("--suffix", action="append", default=[], help="with --under")
    ap.add_argument("--exclude", action="append", default=[], help="with --under")
    return ap


def main(argv=None):
    args = parser().parse_args(argv)
    repo = args.repo.resolve()
    try:
        revision = git(repo, "rev-parse", "--short", "HEAD").strip()
        if args.under:
            rows = under_rows(
                repo, args.under, args.min_lines, args.suffix, args.exclude
            )
        else:
            rows = ceiling_rows(repo)
        ranked = rank(repo, rows, args.since)
    except (GitError, OSError, ValueError) as error:
        print(f"ceiling-hotspots: {error}", file=sys.stderr)
        return 2
    if args.under and not rows:
        print(
            f"ceiling-hotspots: no tracked file under {args.under} matches",
            file=sys.stderr,
        )
        return 2
    print(f"# history at {revision} since {args.since}; lines from the working tree")
    print(render(ranked))
    return 0


if __name__ == "__main__":
    sys.exit(main())
