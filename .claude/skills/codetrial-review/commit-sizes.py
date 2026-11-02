#!/usr/bin/env python3
"""Collect parent-relative commit sizes from a local PR range without network access."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from contextlib import contextmanager


def git(*args):
    return subprocess.check_output(
        ["git", "--no-lazy-fetch", *args],
        stderr=subprocess.PIPE,
        env={**os.environ, "GIT_TERMINAL_PROMPT": "0", "GIT_NO_REPLACE_OBJECTS": "1"},
    )


@contextmanager
def canonical(source, worktree=False):
    objects = Path(git("rev-parse", "--git-path", "objects").decode().strip()).resolve()
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(
        GIT_CONFIG_NOSYSTEM="1",
        GIT_CONFIG_GLOBAL=os.devnull,
        GIT_ATTR_NOSYSTEM="1",
        GIT_NO_REPLACE_OBJECTS="1",
        GIT_TERMINAL_PROMPT="0",
    )
    with tempfile.TemporaryDirectory(prefix="codetrial-diff-") as directory:
        subprocess.run(
            ["git", "init", "--bare", "--quiet", "--template=", directory],
            env=env,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        context = Path(directory)
        (context / "objects/info/alternates").write_text(
            json.dumps(str(objects), ensure_ascii=False) + "\n"
        )
        prefix = [
            "git",
            "--no-lazy-fetch",
            "--git-dir=" + directory,
            "--attr-source=" + source,
            "-c",
            "core.attributesFile=" + os.devnull,
        ]
        if worktree:
            top = git("rev-parse", "--show-toplevel").decode().strip()
            index = Path(
                git("rev-parse", "--git-path", "index").decode().strip()
            ).resolve()
            if index.exists():
                shutil.copy2(index, context / "index")
                shared_path = git("rev-parse", "--shared-index-path").decode().strip()
                if shared_path:
                    shutil.copy2(shared_path, context / Path(shared_path).name)
                for shared in index.parent.glob("sharedindex.*"):
                    shutil.copy2(shared, context / shared.name)
            (context / "HEAD").write_text(source + "\n")
            prefix.extend(["--work-tree=" + top, "-c", "core.bare=false"])

        def run(*args):
            return subprocess.check_output(
                [*prefix, *args], env=env, stderr=subprocess.PIPE
            )

        yield run


def diff(base, head=None, *, numstat=False, attributes=None):
    # `attributes` pins which commit's .gitattributes decide binary and -diff.
    # A change between two commits is read under the base's, so a branch cannot
    # hide what it adds from the reviewer, or from the count, by marking it
    # -diff; the working tree has no base to pin and keeps its own.
    source = head or git("rev-parse", "HEAD").decode().strip()
    with canonical(
        attributes or (base if head else source), worktree=head is None
    ) as run:
        flags = [
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--no-relative",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--full-index",
            "--diff-algorithm=myers",
            "--no-indent-heuristic",
        ]
        flags += (
            ["--numstat", "--no-renames", "-z"]
            if numstat
            else ["--find-renames=50%", "--unified=3"]
        )
        return run("diff", *flags, base, *([head] if head else []), "--")


def resolve(ref):
    return (
        git("rev-parse", "--verify", "--end-of-options", ref + "^{commit}")
        .decode()
        .strip()
    )


def collect(base, head):
    if git("rev-parse", "--is-shallow-repository").strip() == b"true":
        raise ValueError("Fetch complete history before measuring commit sizes")
    grafts = Path(git("rev-parse", "--git-path", "info/grafts").decode().strip())
    if grafts.exists() and grafts.read_bytes().strip():
        raise ValueError("Remove local history grafts before measuring commit sizes")
    head = resolve(head)
    base = resolve(base)
    ancestor = git("merge-base", base, head).decode().strip()
    commits = []
    complete = True
    for sha in (
        git("rev-list", "--reverse", ancestor + ".." + head).decode().splitlines()
    ):
        parents = git("rev-list", "--parents", "-n", "1", sha).decode().split()
        comparison = [parents[1], sha] if len(parents) > 1 else ["--root", sha]
        with canonical(sha) as run:
            if len(parents) == 3:
                # A clean integration (a merge of upstream main, a merge queue)
                # adds nothing of its own; size only what it changes beyond the
                # clean merge, so incoming commits are not counted as this one's.
                try:
                    clean = run("merge-tree", "--write-tree", *parents[1:]).split(
                        b"\n"
                    )[0]
                    comparison = [clean.decode(), sha]
                except subprocess.CalledProcessError:
                    pass
            raw = run(
                "diff-tree",
                "--no-commit-id",
                "--no-ext-diff",
                "--no-textconv",
                "-r",
                "--numstat",
                "--no-renames",
                "-z",
                *comparison,
            )
        files = []
        for record in raw.split(b"\0"):
            if not record:
                continue
            added, deleted, path = record.split(b"\t", 2)
            files.append(
                {
                    "path": os.fsdecode(path),
                    "additions": int(added) if added != b"-" else None,
                    "deletions": int(deleted) if deleted != b"-" else None,
                }
            )
        known = all(f[k] is not None for f in files for k in ("additions", "deletions"))
        complete = complete and known
        commits.append(
            {
                "sha": sha,
                "files": files,
                "known_lines": sum(
                    (f["additions"] or 0) + (f["deletions"] or 0) for f in files
                ),
                "complete": known,
            }
        )
    return {"head": head, "base": ancestor, "commits": commits, "complete": complete}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base")
    parser.add_argument("--diff", help="Local bundle comparison: base or base...HEAD")
    parser.add_argument("--numstat", action="store_true")
    parser.add_argument("--head", default="HEAD")
    args = parser.parse_args()
    try:
        if args.diff:
            if args.diff.endswith("...HEAD"):
                head = resolve("HEAD")
                base = git("merge-base", args.diff[:-7], head).decode().strip()
                raw = diff(base, head, numstat=args.numstat)
            else:
                base = (
                    git(
                        "rev-parse",
                        "--verify",
                        "--end-of-options",
                        args.diff + "^{commit}",
                    )
                    .decode()
                    .strip()
                )
                raw = diff(base, numstat=args.numstat)

            sys.stdout.buffer.write(raw)
            return 0
        if not args.base:
            parser.error("--base or --diff is required")
        result = collect(args.base, args.head)
    except (subprocess.CalledProcessError, ValueError, OSError):
        print(
            json.dumps(
                {
                    "commits": [],
                    "complete": False,
                    "error": "Cannot inspect the full local commit range; use a checkout with its base and head",
                }
            )
        )
        return 1
    print(json.dumps(result))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
