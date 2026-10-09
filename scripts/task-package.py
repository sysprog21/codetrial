#!/usr/bin/env python3
"""Start, validate, preview and build an instructor task set; never publish
it.

    task-package.py references                    tasks to start from
    task-package.py new --bank BANK --from REF    add one to BANK
    task-package.py build --bank BANK --site URL  encrypt it for a site
"""

import sys

from problem_bank import runtime

# Before anything that needs a newer Python: only `build` encrypts.
runtime.ensure(needs_cryptography=sys.argv[1:2] == ["build"])

import argparse  # noqa: E402
import getpass  # noqa: E402
import json  # noqa: E402
import os  # noqa: E402
import shlex  # noqa: E402
from pathlib import Path  # noqa: E402

from problem_bank.task_authoring import new_task, next_version, references  # noqa: E402
from problem_bank.task_package import (  # noqa: E402
    assignment_link,
    build_package,
    load_bank,
    preview,
    publish_scan,
    render_prompt,
    site_base,
    validate_package,
)
from problem_bank.tasks import IDENTIFIER  # noqa: E402


def read_pin():
    """The PIN from a non-echoing prompt, asked twice, or once from stdin."""
    if not sys.stdin.isatty():
        return sys.stdin.readline().rstrip("\r\n")
    pin = getpass.getpass("PIN: ")
    if getpass.getpass("PIN again: ") != pin:
        raise ValueError("the two PINs differ")
    return pin


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("references", help="list the tasks `new` can start from")
    arg = sub.add_parser("new", help="add a copy of a reference task to a bank")
    arg.add_argument("--bank", type=Path, required=True)
    arg.add_argument("--from", dest="reference", required=True, metavar="REF")
    arg.add_argument("--id", help="the new task's id; the reference's by default")
    for command in ("validate", "render-prompt", "preview", "build"):
        arg = sub.add_parser(command)
        arg.add_argument("--bank", type=Path, required=True)
        # Authoring commands read the bank alone. A build names the set (the
        # bank's directory name unless given) and its version (the next one
        # the output does not hold unless given).
        building = command == "build"
        arg.add_argument("--set-id", default=None if building else "preview")
        arg.add_argument("--version", type=int, default=None if building else 1)
        if command in {"render-prompt", "preview"}:
            arg.add_argument("--task-id", required=True)
        if building:
            arg.add_argument("--site", required=True)
            arg.add_argument("--output", type=Path, required=True)
    arg = sub.add_parser("publish-scan")
    arg.add_argument("directory", type=Path)
    args = parser.parse_args()
    try:
        if args.command == "publish-scan":
            print(f"Validated {publish_scan(args.directory)} encrypted packages")
        elif args.command == "references":
            try:
                for task_id, title, kind in references():
                    print(f"{task_id:32} {kind:8} {title}")
                sys.stdout.flush()
            except BrokenPipeError:
                # Read by `head` or a pager that has seen enough.
                os.dup2(os.open(os.devnull, os.O_WRONLY), sys.stdout.fileno())
        elif args.command == "new":
            task_id, notes = new_task(args.bank, args.reference, args.id)
            preview_command = shlex.join(
                [sys.executable, sys.argv[0], "preview", "--bank", str(args.bank)]
                + ["--task-id", task_id]
            )
            print(
                f"Added {task_id} to {args.bank}. Edit its files there"
                " (task-mode.json holds the sidecar, task-set.json the rules),"
                f" then check what the learner sees with\n  {preview_command}"
            )
            for note in notes:
                print(f"Note: {note}.")
        elif args.command == "build":
            set_id = args.set_id
            if set_id is None:
                set_id = args.bank.resolve().name
                if not IDENTIFIER.fullmatch(set_id):
                    raise ValueError(
                        f"the bank directory name {set_id!r} is not a valid set"
                        " id (a lowercase letter, then lowercase letters, digits"
                        " or dashes); pass --set-id"
                    )
            version, first = args.version, False
            if version is None:
                version, first = next_version(args.output, set_id)
            # Everything but the PIN checked first, so a mistake costs no PIN.
            site = site_base(args.site)
            package = load_bank(args.bank, set_id, version)
            entries, _ = validate_package(package)
            print(f"Building set {set_id}, version {version}.")
            if first:
                print(
                    f"{args.output} holds no version of {set_id} yet. If you"
                    " published this set from another directory, stop and"
                    " pass --version above its last one."
                )
            directory = build_package(package, read_pin(), site, args.output)
            landing = f"{site.rstrip('/')}/sets/{set_id}/{version}/"
            print(f"Built {directory}.")
            print(
                f"Publish {args.output} as the root of {site} (for GitHub Pages,"
                " push it to a repository and turn Pages on for that branch),"
                f" then give learners the page {landing} or these links and,"
                " separately, the PIN:"
            )
            for task_id, entry in sorted(entries.items()):
                link = assignment_link(site, set_id, task_id, version)
                print(f"  {entry['variant']['title']}: {link}")
        else:
            package = load_bank(args.bank, args.set_id, args.version)
            if args.command == "validate":
                print(f"Validated {len(package['problems'])} tasks")
            elif args.command == "render-prompt":
                print(render_prompt(package, args.task_id))
            else:
                print(json.dumps(preview(package, args.task_id), indent=2))
    except (
        OSError,
        ValueError,
        RuntimeError,
        ImportError,
        KeyError,
        TypeError,
    ) as error:
        print(f"task-package: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
