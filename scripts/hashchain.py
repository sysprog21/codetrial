#!/usr/bin/env python3
"""Generate and verify parent-linked Change-Ids using only Python's standard library."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

POLICY = "scripts/hashchain-base"
ID = re.compile(rb"Change-Id: I([0-9a-f]{40})")


def git_env():
    return {**os.environ, "GIT_TERMINAL_PROMPT": "0", "GIT_NO_REPLACE_OBJECTS": "1"}


def require_git():
    version = subprocess.run(
        ["git", "--version"], capture_output=True, check=True, env=git_env()
    ).stdout
    parsed = re.search(rb"git version (\d+)\.(\d+)", version)
    if not parsed or tuple(map(int, parsed.groups())) < (2, 45):
        raise ValueError("Hashchain verification requires Git 2.45 or newer")


def git(*args, data=None, check=True):
    return subprocess.run(
        ["git", "--no-lazy-fetch", *args],
        input=data,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=check,
        env=git_env(),
    ).stdout


def resolve(ref):
    return (
        git("rev-parse", "--verify", "--end-of-options", ref + "^{commit}")
        .decode()
        .strip()
    )


def base(tip="HEAD", staged=False):
    # A working-tree edit must not change the policy used to judge a pushed tip.
    candidates = [":" + POLICY] if staged else []
    candidates.append(tip + ":" + POLICY)
    for candidate in candidates:
        value = git("show", candidate, check=False).strip()
        if value:
            break
    else:
        # Retain enforcement when a commit deletes the policy file.
        history = (
            git("log", "--format=%H", tip, "--", POLICY, check=False)
            .decode()
            .splitlines()
        )
        value = b""
        for commit in history:
            value = git("show", commit + ":" + POLICY, check=False).strip()
            if value:
                break
        if not value:
            # Nothing committed names a policy; a file lying in the working
            # tree must not become one, or a pushed tip would be judged by
            # whatever happens to be on this machine.
            return None
    if not re.fullmatch(rb"[0-9a-f]{40}", value):
        raise ValueError("hashchain-base must contain exactly one full commit ID")
    return resolve(value.decode())


def seed(anchor):
    return hashlib.sha256(("codetrial/" + anchor).encode()).hexdigest()[:40]


def next_id(previous, tree):
    return hashlib.sha256((previous + ":" + tree).encode()).hexdigest()[:40]


def commit(sha):
    headers, message = git("cat-file", "commit", sha).split(b"\n\n", 1)
    tree = next(
        line[5:].decode() for line in headers.splitlines() if line.startswith(b"tree ")
    )
    parents = [
        line[7:].decode()
        for line in headers.splitlines()
        if line.startswith(b"parent ")
    ]
    return headers, message, tree, parents


# The one definition of "Dependabot" for commits: its bot account's address.
# A commit's author is only a claim anyone can write, so on its own it proves
# nothing; CI pairs it with the pull request opener (exempt_pr) before it
# excuses anything that review evidence would otherwise cover.
DEPENDABOT = "49699333+dependabot[bot]@users.noreply.github.com"


def bot(headers):
    author = next((x for x in headers.splitlines() if x.startswith(b"author ")), b"")
    return bool(re.search(b"<" + re.escape(DEPENDABOT.encode()) + b"> ", author))


def bots(commits):
    """The commits among `commits` that name Dependabot as their author."""
    if not commits:
        return []
    shown = git(
        "log",
        "--no-walk=unsorted",
        "--stdin",
        "--format=%H %ae",
        data="\n".join(commits).encode(),
    )
    pairs = (line.partition(" ") for line in shown.decode().splitlines())
    return [sha for sha, _, email in pairs if email == DEPENDABOT]


def exempt_pr(login, kind):
    """Whether GitHub says a pull request was opened by Dependabot.

    The opener comes from the API, which commit metadata cannot forge. The
    caller still has to show that every commit is Dependabot's, or a person's
    commit pushed onto its branch rides along.
    """
    return login == "dependabot[bot]" and kind == "Bot"


def change_id(message):
    raw = [
        line for line in message.splitlines() if line.lower().startswith(b"change-id:")
    ]
    trailers = git("interpret-trailers", "--parse", data=message).splitlines()
    parsed = [line for line in trailers if line.lower().startswith(b"change-id:")]
    if len(raw) != 1 or raw != parsed or not ID.fullmatch(raw[0]):
        raise ValueError(
            "Exactly one final Change-Id: I<40 lowercase hex digits> trailer is required"
        )
    return ID.fullmatch(raw[0])[1].decode()


def shallow():
    return git("rev-parse", "--is-shallow-repository").strip() == b"true"


def history(anchor, tip):
    if shallow():
        raise ValueError("Fetch complete history before verifying the hashchain")
    grafts = Path(git("rev-parse", "--git-path", "info/grafts").decode().strip())
    if grafts.exists() and grafts.read_bytes().strip():
        raise ValueError("Remove local history grafts before verifying the hashchain")

    def ancestor(older, newer):
        result = subprocess.run(
            ["git", "--no-lazy-fetch", "merge-base", "--is-ancestor", older, newer],
            capture_output=True,
            env=git_env(),
        )
        if result.returncode not in (0, 1):
            raise subprocess.CalledProcessError(result.returncode, result.args)
        return result.returncode == 0

    if not ancestor(anchor, tip):
        if ancestor(tip, anchor):
            return []
        raise ValueError(
            "Tip does not descend from hashchain-base; fetch complete history and rebase"
        )
    return (
        git("rev-list", "--reverse", "--topo-order", anchor + ".." + tip)
        .decode()
        .splitlines()
    )


def merge_link(sha):
    return hashlib.sha256(("codetrial/merge:" + sha).encode()).hexdigest()[:40]


def check_merge(tree, parents):
    objects = Path(git("rev-parse", "--git-path", "objects").decode().strip()).resolve()
    env = {
        key: value for key, value in os.environ.items() if not key.startswith("GIT_")
    }
    env.update(
        GIT_CONFIG_NOSYSTEM="1",
        GIT_CONFIG_GLOBAL=os.devnull,
        GIT_ATTR_NOSYSTEM="1",
        GIT_NO_REPLACE_OBJECTS="1",
        GIT_TERMINAL_PROMPT="0",
    )
    # The current index, attributes and local merge drivers must not affect
    # historical integrations. Computed objects live only in this bare context.
    with tempfile.TemporaryDirectory(prefix="codetrial-merge-") as directory:
        subprocess.run(
            ["git", "init", "--bare", "--quiet", "--template=", directory],
            env=env,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        (Path(directory) / "objects/info/alternates").write_text(
            json.dumps(str(objects), ensure_ascii=False) + "\n"
        )
        result = subprocess.run(
            [
                "git",
                "--no-lazy-fetch",
                "--git-dir=" + directory,
                "--attr-source=" + parents[0],
                "-c",
                "core.attributesFile=" + os.devnull,
                "merge-tree",
                "--write-tree",
                *parents,
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
        )
    if result.returncode or result.stdout.splitlines()[:1] != [tree.encode()]:
        raise ValueError(
            "Integration commits must be clean two-parent Git merges without extra edits "
            "(Git 2.45 or newer required); resolve conflicts on the topic branch and rebase before merging"
        )


def verify(tip="HEAD", anchor=None):
    tip = resolve(tip)
    trusted = anchor is not None
    if anchor is not None and not re.fullmatch(r"[0-9a-f]{40}", anchor):
        raise ValueError("Trusted anchor must be a full commit SHA, not a movable ref")
    anchor = resolve(anchor) if anchor is not None else base(tip)
    if anchor is None:
        return
    commits = history(anchor, tip)
    if trusted and commits:
        policy = git("show", tip + ":" + POLICY, check=False).strip()
        # A legacy branch may predate installation; once introduced, the policy
        # must survive so the next CI run cannot adopt a different anchor.
        if policy != anchor.encode() and (
            policy or git("log", "-1", "--format=%H", tip, "--", POLICY).strip()
        ):
            raise ValueError(
                "Submitted hashchain-base must retain the trusted anchor; "
                "policy changes or deletion require a coordinated maintainer migration"
            )
    links = {anchor: seed(anchor)}
    for sha in commits:
        headers, message, tree, parents = commit(sha)
        if not parents or any(parent not in links for parent in parents):
            raise ValueError(
                "All topic branches must descend from the anchor; rebase first"
            )
        if len(parents) == 2:
            check_merge(tree, parents)
            links[sha] = merge_link(sha)
        elif len(parents) == 1:
            want = next_id(links[parents[0]], tree)
            # Dependabot cannot run the hooks; its commits continue the chain
            # without a trailer so that merging one needs no anchor migration.
            if not bot(headers) and change_id(message) != want:
                raise ValueError(
                    f"Change-Id chain breaks at {sha[:12]}; run python3 scripts/hashchain.py rechain"
                )
            links[sha] = want
        else:
            raise ValueError("Only two-parent integration merges are supported")


def link(sha, anchor):
    pending = []
    while sha != anchor:
        headers, message, tree, parents = commit(sha)
        if len(parents) == 2:
            previous = merge_link(sha)
            break
        if len(parents) != 1:
            raise ValueError("A chain parent must descend from the configured anchor")
        if not bot(headers):
            previous = change_id(message)
            break
        # Bot trailers are not the link, even if one is present. Match verify's
        # parent/tree derivation, iteratively for a run of unhooked bot commits.
        pending.append(tree)
        sha = parents[0]
    else:
        previous = seed(anchor)
    for tree in reversed(pending):
        previous = next_id(previous, tree)
    return previous


GENERATED = re.compile(rb"(?i)^(change-id|rechained):")


def clean(message):
    """The message without any line that looks like a generated trailer."""
    return b"\n".join(
        line for line in message.splitlines() if not GENERATED.match(line)
    ).strip()


def lookalikes(message):
    """Lines outside the last paragraph that begin like a generated trailer.

    The verifier accepts exactly one such line in a message, the trailer, so a
    quotation or example in the body would fail verification later. Dropping it
    would lose the contributor's text without telling them; the commit-msg hook
    refuses instead, and says which lines.
    """
    lines = message.strip().splitlines()
    end = len(lines)
    while end > 0 and lines[end - 1].strip():
        end -= 1
    return [
        line.decode(errors="replace") for line in lines[:end] if GENERATED.match(line)
    ]


def stamped(message, value):
    # interpret-trailers appends to a final trailer block (Signed-off-by,
    # Co-authored-by) and starts one only when there is none, so the
    # Change-Id never detaches those from the end of the message.
    return git(
        "interpret-trailers",
        "--where",
        "end",
        "--if-exists",
        "add",
        "--if-missing",
        "add",
        "--trailer",
        "Change-Id: I" + value,
        data=clean(message) + b"\n",
    )


def claim_path(name):
    return Path(
        git("rev-parse", "--git-path", "codetrial-hashchain-" + name + ".json")
        .decode()
        .strip()
    )


def atomic(path, value):
    fd, name = tempfile.mkstemp(dir=path.parent, prefix=".hashchain-")
    try:
        with os.fdopen(fd, "w") as output:
            json.dump(value, output)
        os.replace(name, path)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def replaying():
    # A rebase or cherry-pick commits onto a base that is not yet restamped;
    # `rechain` repairs the series once the replay is over.
    names = ("rebase-merge", "rebase-apply", "sequencer")
    args = [a for name in names for a in ("--git-path", name)]
    return any(
        Path(line).exists() for line in git("rev-parse", *args).decode().splitlines()
    )


def candidate():
    return {"head": resolve("HEAD"), "tree": git("write-tree").decode().strip()}


def precommit():
    # A shallow clone lacks the anchor; pre-push and CI demand the history.
    if not shallow() and base(staged=True) is not None:
        atomic(claim_path("ready"), candidate())


def message(path):
    if shallow():
        return
    anchor = base(staged=True)
    if anchor is None:
        return
    current = candidate()
    if (
        not claim_path("ready").exists()
        or json.loads(claim_path("ready").read_text()) != current
    ):
        raise ValueError(
            "This index has no successful pre-commit check; run git commit normally"
        )
    # Full-history verification belongs to pre-push and CI; a shallow clone
    # can still commit, and pre-push demands the history it lacks.
    if not replaying():
        verify(current["head"])
    text = path.read_bytes()
    # `git commit -v` appends the patch after a scissors line, which comment
    # stripping would otherwise turn into message text.
    text = re.split(rb"(?m)^.*-{8,}\s*>8\s*-{8,}.*$", text, maxsplit=1)[0]
    quoted = lookalikes(git("stripspace", "--strip-comments", data=text))
    if quoted:
        raise ValueError(
            "Only the generated trailer may begin with Change-Id: or Rechained:. "
            "Indent or reword: " + "; ".join(quoted)
        )
    try:
        old = change_id(text)
    except ValueError:
        old = None
    parent = current["head"]
    try:
        previous = link(parent, anchor)
        if parent != anchor and old == previous and len(commit(parent)[3]) == 1:
            parent = commit(parent)[3][0]
            previous = link(parent, anchor)
    except ValueError:
        if not replaying():
            raise
        # The replayed parent is not restamped yet; `rechain` assigns the
        # real link once the series is complete.
        previous = seed(anchor)
    value = next_id(previous, current["tree"])
    cleanup = git("config", "--get", "commit.cleanup", check=False).strip()
    if cleanup != b"verbatim":
        text = git("stripspace", "--strip-comments", data=text)
    text = stamped(text, value)
    path.write_bytes(text)
    current["message"] = hashlib.sha256(git("stripspace", data=text)).hexdigest()
    atomic(claim_path("message"), current)
    claim_path("ready").unlink()


def rewrite(sha, parent, value, *, unsign=False):
    headers, text, _, _ = commit(sha)
    if not unsign and re.search(rb"(?m)^gpgsig(-sha256)? ", headers):
        raise ValueError(
            f"{sha[:12]} is signed and its Change-Id needs correcting; "
            "amend it again with its message and re-sign, or run rechain and sign afterwards"
        )
    result, skipping = [], False
    for line in headers.splitlines():
        if line.startswith(b" ") and skipping:
            continue
        skipping = line.startswith((b"gpgsig ", b"gpgsig-sha256 ", b"mergetag "))
        if skipping:
            continue
        result.append(
            b"parent " + parent.encode() if line.startswith(b"parent ") else line
        )
    raw = (
        b"\n".join(result)
        + b"\n\n"
        + (stamped(text, value) if value is not None else text)
    )
    return (
        git("hash-object", "-t", "commit", "-w", "--stdin", data=raw).decode().strip()
    )


def fix():
    path = claim_path("message")
    if not path.exists():
        return
    claim = json.loads(path.read_text())
    path.unlink()
    head = resolve("HEAD")
    anchor = base(head)
    if anchor is None or head == anchor:
        return
    _, text, tree, parents = commit(head)
    if (
        len(parents) != 1
        or tree != claim["tree"]
        or hashlib.sha256(git("stripspace", data=text)).hexdigest() != claim["message"]
    ):
        return
    before_parents = commit(claim["head"])[3]
    if parents != [claim["head"]] and parents != before_parents:
        return
    want = next_id(link(parents[0], anchor), tree)
    if change_id(text) != want:
        updated = rewrite(head, parents[0], want)
        git(
            "update-ref", "-m", "hashchain: correct amended link", "HEAD", updated, head
        )


def rechain():
    target = git("symbolic-ref", "-q", "HEAD", check=False).decode().strip()
    if not target or target == "refs/heads/stability":
        raise ValueError("Repair history on an attached work branch, never stability")
    head = resolve("HEAD")
    anchor = base(head)
    if anchor is None:
        raise ValueError("Configure scripts/hashchain-base before repairing history")
    boundary = anchor
    merges = (
        git("rev-list", "--first-parent", "--merges", anchor + ".." + head)
        .decode()
        .splitlines()
    )
    if merges:
        boundary = merges[0]
        # Published integrations are immutable. Repair only the linear topic
        # suffix, after verifying every branch already incorporated by the merge.
        verify(boundary, anchor)
    parent, previous = boundary, link(boundary, anchor)
    style = Path(__file__).with_name("git-commit-msg.sh")
    count = 0
    original_parent = boundary
    for sha in history(boundary, head):
        headers, text, tree, parents = commit(sha)
        if parents != [original_parent]:
            raise ValueError(
                "Rechain only a linear topic suffix after validated integrations"
            )
        original_parent = sha
        want = next_id(previous, tree)
        try:
            valid = parents == [parent] and (
                bot(headers)
                or (
                    change_id(text) == want
                    and not re.search(rb"(?im)^Rechained:", text)
                )
            )
        except ValueError:
            valid = False
        if valid:
            parent = sha
        else:
            if not bot(headers):
                checked = subprocess.run(
                    ["sh", str(style), "--stored", "-"], input=clean(text) + b"\n"
                )
                if checked.returncode:
                    raise ValueError(f"Fix the message of {sha[:12]} before rechaining")
            parent = rewrite(sha, parent, None if bot(headers) else want, unsign=True)
            count += 1
        previous = want
    if count:
        if git("symbolic-ref", "-q", "HEAD").decode().strip() != target:
            raise ValueError("The checked-out branch changed during repair")
        git(
            "update-ref",
            "-m",
            "hashchain: restamp reviewed history",
            target,
            parent,
            head,
        )
    verify()
    print(
        f"Hashchain verified; {count} commit(s) restamped. Code trees were preserved."
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "command",
        choices=["verify", "precommit", "message", "fix", "rechain", "bots"],
    )
    parser.add_argument("file", nargs="?")
    parser.add_argument(
        "--all",
        action="store_true",
        help="bots: succeed only if every commit on standard input is Dependabot's",
    )
    parser.add_argument("--tip", default="HEAD")
    parser.add_argument(
        "--anchor", help="Trusted verification anchor; overrides submitted policy"
    )
    args = parser.parse_args()
    try:
        require_git()
        if args.command == "verify":
            verify(args.tip, args.anchor)
        elif args.command == "precommit":
            precommit()
        elif args.command == "message":
            if not args.file:
                parser.error("message requires a message file")
            message(Path(args.file))
        elif args.command == "fix":
            fix()
        elif args.command == "bots":
            listed = sys.stdin.read().split()
            found = bots(listed)
            if args.all:
                return 0 if listed and len(found) == len(listed) else 1
            print("\n".join(found))
        else:
            rechain()
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(
            1,
            "Hashchain: "
            + (
                str(error)
                if not isinstance(error, subprocess.CalledProcessError)
                else "cannot inspect local history; fetch the configured base"
            )
            + "\n",
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
