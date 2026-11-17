#!/usr/bin/env python3
"""Contract tests for scripts/fetch-gate-note.sh, the one place a fork's gate
note is fetched.

The script builds its own URL, so a case cannot point it at a local path. A
stub `git` on PATH, which no PR can reach, logs the argument vector and the
environment, answers canned fetch failures, or rewrites the one github.com URL
to a local directory and runs the real git against real objects.
"""

import importlib.util
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "support", Path(__file__).with_name("support.py")
)
support = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(support)

SCRIPT = support.ROOT / "scripts" / "fetch-gate-note.sh"
FAKE_GIT = support.stub("fetch-gate-note/git.sh")
REAL_GIT = shutil.which("git")
REF = "refs/notes/codetrial-gate"
NOTE = b'{"version": 1, "worktree_tree": "' + b"a" * 40 + b'", "receipts": []}'
GIT_IDENTITY = {
    "GIT_AUTHOR_NAME": "t",
    "GIT_AUTHOR_EMAIL": "t@example.invalid",
    "GIT_COMMITTER_NAME": "t",
    "GIT_COMMITTER_EMAIL": "t@example.invalid",
    "GIT_CONFIG_GLOBAL": "/dev/null",
    "GIT_CONFIG_NOSYSTEM": "1",
}


def git(*args, cwd=None, env=()):
    return subprocess.run(
        [REAL_GIT, *args],
        cwd=cwd,
        capture_output=True,
        text=True,
        check=True,
        env=os.environ | GIT_IDENTITY | dict(env),
    ).stdout.strip()


@unittest.skipUnless(REAL_GIT and shutil.which("timeout"), "git or timeout missing")
class FetchGateNoteTests(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.scratch = Path(scratch.name)
        (self.scratch / "bin").mkdir()
        support.write_executable(self.scratch / "bin" / "git", FAKE_GIT)
        (self.scratch / "tmp").mkdir()
        self.log = self.scratch / "log"
        self.log.write_text("")
        self.remote = self.scratch / "remote"

    def publish(self, note=NOTE, notes=True, owner="octo", name="repo"):
        """A remote at owner/name.git whose head commit carries the note."""
        work = self.scratch / "work"
        work.mkdir(exist_ok=True)
        git("init", "-q", "-b", "main", cwd=work)
        (work / "f").write_text(os.urandom(4).hex())
        git("add", "f", cwd=work)
        git("commit", "-q", "-m", "c", cwd=work)
        head = git("rev-parse", "HEAD", cwd=work)
        refs = ["refs/heads/main"]
        if notes:
            (self.scratch / "note").write_bytes(note)
            # -C stores the blob as given; -F would append a newline.
            blob = git("hash-object", "-w", str(self.scratch / "note"), cwd=work)
            git("notes", "--ref", REF, "add", "-C", blob, head, cwd=work)
            refs.append(REF)
        bare = self.remote / owner / f"{name}.git"
        bare.mkdir(parents=True)
        git("init", "-q", "--bare", cwd=bare)
        git("config", "uploadpack.allowFilter", "true", cwd=bare)
        git("push", "-q", str(bare), *refs, cwd=work)
        return head

    def fetch(self, *args, **env):
        base = {
            "PATH": f"{self.scratch / 'bin'}:{os.environ['PATH']}",
            "FAKE_LOG": str(self.log),
            "FAKE_REMOTE": str(self.remote),
            "REAL_GIT": REAL_GIT,
            "TMPDIR": str(self.scratch / "tmp"),
        }
        return subprocess.run(
            ["sh", str(SCRIPT), *args],
            capture_output=True,
            env=os.environ | base | env,
        )

    def calls(self):
        out, current = [], None
        for line in self.log.read_text().splitlines():
            if line == "--call--":
                current = {"args": [], "env": []}
                out.append(current)
            elif line.startswith("arg="):
                current["args"].append(line[4:])
            elif line.startswith("env="):
                current["env"].append(line[4:])
        return out

    def test_prints_the_note_of_the_head_commit(self):
        head = self.publish()
        result = self.fetch("octo", "repo", head)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, NOTE)

    def test_a_note_of_exactly_the_limit_is_read(self):
        note = b"x" * 16384
        head = self.publish(note=note)
        result = self.fetch("octo", "repo", head)
        self.assertEqual((result.returncode, result.stdout), (0, note))

    def test_the_argument_vector_pins_the_url_protocol_and_ref(self):
        head = self.publish()
        self.fetch("octo", "re.po-1", head)
        fetch = next(c for c in self.calls() if "fetch" in c["args"])
        args = fetch["args"]
        for want in (
            "protocol.allow=never",
            "protocol.https.allow=always",
            "credential.helper=",
            "--no-tags",
            "--depth=1",
            "--no-recurse-submodules",
            "--filter=blob:limit=16385",
            "https://github.com/octo/re.po-1.git",
        ):
            self.assertIn(want, args)
        self.assertIn(f"{REF}:{REF}", " ".join(args).replace("+", ""))
        # Nothing but the one notes ref is asked for.
        self.assertEqual(sum(a.startswith("https://") for a in args), 1)
        for var in ("GIT_TERMINAL_PROMPT=0", "GIT_CONFIG_NOSYSTEM=1"):
            self.assertIn(var, fetch["env"])

    def test_every_git_call_runs_in_a_fresh_directory_with_an_empty_home(self):
        head = self.publish()
        real_home = self.scratch / "realhome"
        real_home.mkdir()
        (real_home / ".gitconfig").write_text("[user]\n\tname = leaked\n")
        self.fetch(
            "octo",
            "repo",
            head,
            HOME=str(real_home),
            GIT_DIR=str(self.scratch / "elsewhere"),
            GIT_CONFIG_COUNT="1",
            GIT_CONFIG_KEY_0="url.https://evil.invalid/.insteadOf",
            GIT_CONFIG_VALUE_0="https://github.com/",
        )
        self.assertTrue(self.calls())
        for call in self.calls():
            env = dict(e.split("=", 1) for e in call["env"])
            self.assertNotEqual(env["HOME"], str(real_home))
            self.assertTrue(env["HOME"].startswith(str(self.scratch / "tmp")))
            for leaked in ("GIT_DIR", "GIT_CONFIG_COUNT", "GIT_CONFIG_KEY_0"):
                self.assertNotIn(leaked, env)
            where = call["args"][call["args"].index("-C") + 1]
            self.assertTrue(where.startswith(str(self.scratch / "tmp")))
            # No name from the API reaches a path.
            self.assertNotIn("octo", where)

    def test_a_fork_cannot_make_git_write_an_unbounded_file(self):
        head = self.publish()
        self.fetch("octo", "repo", head)
        self.assertTrue(self.calls())
        for call in self.calls():
            env = dict(e.split("=", 1) for e in call["env"])
            # The pack a fetch writes is one file, so a cap on any file bounds
            # what the fork's notes can put on the disk.
            self.assertNotEqual(env["FILE_SIZE_LIMIT"], "unlimited")

    def test_a_ca_bundle_the_caller_names_reaches_git_but_verification_stays_on(self):
        # The caller's trust anchors are theirs to choose (a maintainer behind a
        # TLS-intercepting proxy needs one); switching verification off is not.
        head = self.publish()
        self.fetch(
            "octo",
            "repo",
            head,
            GIT_SSL_CAINFO="/ca/bundle.pem",
            GIT_SSL_CAPATH="/ca/dir",
            GIT_SSL_NO_VERIFY="1",
        )
        self.assertTrue(self.calls())
        for call in self.calls():
            self.assertIn("GIT_SSL_CAINFO=/ca/bundle.pem", call["env"])
            self.assertIn("GIT_SSL_CAPATH=/ca/dir", call["env"])
            self.assertNotIn("GIT_SSL_NO_VERIFY=1", call["env"])

    def test_the_scratch_directory_is_removed(self):
        head = self.publish()
        self.fetch("octo", "repo", head)
        self.fetch("octo", "repo", "b" * 40)
        self.assertEqual(list((self.scratch / "tmp").iterdir()), [])

    def test_a_refused_name_or_sha_never_reaches_git(self):
        head = self.publish()
        bad_names = (
            "",
            ".hidden",
            "-x",
            "a..b",
            "..",
            ".",
            "a/b",
            "a b",
            "a;b",
            "a\nb",
            "$(id)",
            "a:b",
            "a@b",
            "a%2Fb",
            "https://github.com/x/y",
            "x" * 101,
        )
        for bad in bad_names:
            for position in (0, 1):
                args = ["octo", "repo", head]
                args[position] = bad
                result = self.fetch(*args)
                self.assertEqual(result.returncode, 2, (position, bad))
        for sha in ("", "abc", head.upper(), "--upload-pack=x", head + "0", "g" * 40):
            result = self.fetch("octo", "repo", sha)
            self.assertEqual(result.returncode, 2, sha)
        self.assertEqual(self.fetch("octo", "repo").returncode, 2)
        self.assertEqual(self.fetch("octo", "repo", head, "extra").returncode, 2)
        self.assertEqual(self.calls(), [])

    def test_a_missing_ref_is_exit_3(self):
        head = self.publish(notes=False)
        result = self.fetch("octo", "repo", head)
        self.assertEqual(result.returncode, 3)
        self.assertEqual(result.stdout, b"")
        self.assertEqual(len(result.stderr.decode().strip().splitlines()), 1)

    def test_a_commit_without_a_note_is_exit_6(self):
        self.publish()
        result = self.fetch("octo", "repo", "c" * 40)
        self.assertEqual(result.returncode, 6)
        self.assertEqual(result.stdout, b"")

    def test_an_unreadable_notes_tree_is_exit_5_not_a_missing_note(self):
        head = self.publish()
        result = self.fetch(
            "octo", "repo", head, FAKE_NOTES_ERR="fatal: bad tree object"
        )
        self.assertEqual(result.returncode, 5)

    def test_a_missing_or_private_repository_is_exit_4(self):
        for message in (
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
            "remote: Repository not found.\nfatal: repository 'https://github.com/o/n.git/' not found",
        ):
            result = self.fetch(
                "octo", "gone", "a" * 40, FAKE_FETCH_ERR=message, FAKE_FETCH_RC="128"
            )
            self.assertEqual(result.returncode, 4, message)
            self.assertEqual(result.stdout, b"")
            self.assertEqual(len(result.stderr.decode().strip().splitlines()), 1)

    def test_any_other_fetch_failure_is_exit_5(self):
        result = self.fetch(
            "octo",
            "repo",
            "a" * 40,
            FAKE_FETCH_ERR="fatal: unable to access: Could not resolve host: github.com",
        )
        self.assertEqual(result.returncode, 5)

    def test_a_fetch_that_outlives_the_timeout_is_exit_5(self):
        result = self.fetch(
            "octo",
            "repo",
            "a" * 40,
            FAKE_FETCH_SLEEP="10",
            FETCH_GATE_NOTE_TIMEOUT="1",
        )
        self.assertEqual(result.returncode, 5)
        self.assertIn(b"timed out", result.stderr)

    def test_the_timeout_can_be_lowered_but_not_raised_or_garbled(self):
        head = self.publish()
        for value in ("0", "61", "abc", "1;id", "-1", ""):
            result = self.fetch("octo", "repo", head, FETCH_GATE_NOTE_TIMEOUT=value)
            self.assertEqual(result.returncode, 2, value)

    def test_a_note_over_the_limit_is_refused_when_the_filter_omits_it(self):
        head = self.publish(note=b"x" * 16385)
        result = self.fetch("octo", "repo", head)
        self.assertEqual(result.returncode, 7)
        self.assertEqual(result.stdout, b"")

    def test_a_failed_size_probe_is_not_a_missing_note(self):
        head = self.publish(note=b"{}")
        result = self.fetch(
            "octo", "repo", head, FAKE_CATFILE_ERR="fatal: unable to read the object"
        )
        self.assertEqual(result.returncode, 5, result.stderr)

    def test_the_size_is_checked_before_reading_when_the_filter_is_ignored(self):
        head = self.publish(note=b"x" * 40000)
        result = self.fetch("octo", "repo", head, FAKE_DROP_FILTER="1")
        self.assertEqual(result.returncode, 7)
        self.assertEqual(result.stdout, b"")
        self.assertIn(b"16384", result.stderr)
        # The blob was never read: the size came first.
        for call in self.calls():
            self.assertFalse(
                "cat-file" in call["args"] and "blob" in call["args"],
                call["args"],
            )
        self.assertTrue(any("--batch-check" in c["args"] for c in self.calls()))


if __name__ == "__main__":
    unittest.main()
