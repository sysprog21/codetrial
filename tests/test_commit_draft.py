"""Check the commit drafter's text handling, then drive it in a scratch repository.

The backend is a stub that records each prompt, so the cases see exactly what
would leave the machine and when nothing does.
"""

import importlib.util
import os
import shlex
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/commit-draft.py"
BACKEND = ROOT / "tests/stubs/commit-draft/backend.py"
FENCE = "`" * 3

_spec = importlib.util.spec_from_file_location("commit_draft", SCRIPT)
commit_draft = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(commit_draft)

GOOD = """Recheck setup after the capture picker closes

The picker can change the devices while it is open, so the setup it
confirmed before opening no longer describes what the call will use."""


class Clean(unittest.TestCase):
    def test_wrappers_a_model_adds_are_removed(self):
        reply = (
            "<think>The diff changes setup.</think>\n"
            "Here is the commit message:\n\n"
            + FENCE
            + "text\n"
            + GOOD
            + "\n"
            + FENCE
            + "\n"
        )
        self.assertEqual(commit_draft.clean(reply), GOOD)

    def test_a_body_label_on_the_first_line_is_removed(self):
        reply = "Subject: Recheck setup\n\nBody: The picker changes the devices."
        self.assertEqual(
            commit_draft.clean(reply),
            "Recheck setup\n\nThe picker changes the devices.",
        )

    def test_labels_quotes_and_copied_trailers_are_removed(self):
        reply = (
            'Subject: "Recheck setup after the capture picker closes"\n\n'
            "Body:\nThe picker can change the devices.\n\n"
            "Change-Id: I" + "0" * 40 + "\n"
        )
        self.assertEqual(
            commit_draft.clean(reply),
            "Recheck setup after the capture picker closes\n\n"
            "The picker can change the devices.",
        )

    def test_a_subject_that_opens_with_the_label_words_is_kept(self):
        self.assertEqual(
            commit_draft.clean(
                "Commit messages keep their width\n\nThe hook counts bytes."
            ),
            "Commit messages keep their width\n\nThe hook counts bytes.",
        )
        self.assertEqual(
            commit_draft.clean(
                "Commit message: Recheck setup\n\nThe picker changes it."
            ),
            "Recheck setup\n\nThe picker changes it.",
        )

    def test_an_unopened_reasoning_block_is_cut_at_its_close(self):
        self.assertEqual(
            commit_draft.clean("thinking about it\n</think>\n" + GOOD), GOOD
        )


class Reflow(unittest.TestCase):
    """The rules are tests/test_commit_reflow.py's; this pins that a draft
    goes through that pass and not a second copy of it."""

    def test_a_ragged_reply_is_joined_by_the_shared_pass(self):
        ragged = "Recheck setup\n\nThe picker can change\nthe devices."
        self.assertEqual(
            commit_draft.reflow(ragged),
            "Recheck setup\n\nThe picker can change the devices.",
        )
        self.assertEqual(commit_draft.REFLOW.reflow.__module__, "commit_reflow")


class Fit(unittest.TestCase):
    def test_small_files_stay_whole_and_the_large_one_is_cut(self):
        diffs = {"small": "s\n" * 10, "large": "l\n" * 5000}
        kept = commit_draft.fit(diffs, 2000)
        self.assertEqual(kept["small"], diffs["small"])
        self.assertIn("more lines of this file left out", kept["large"])
        self.assertLessEqual(sum(len(text) for text in kept.values()), 2000)
        self.assertTrue(kept["large"].split("\n")[-2] == "l")

    def test_a_share_too_small_to_show_a_hunk_becomes_one_line(self):
        # The staged file list names every file, so a file left out of the
        # diff is still in the prompt, and the budget holds however many.
        diffs = {"f%d" % n: "line\n" * 50 for n in range(100)}
        kept = commit_draft.fit(diffs, 2000)
        self.assertLessEqual(sum(len(text) for text in kept.values()), 2000)
        self.assertEqual(kept["f0"], "")


class Backends(unittest.TestCase):
    """The commands carry the drafter's promise that a backend sees the
    prompt and nothing else."""

    def test_claude_runs_with_no_tools_and_no_setting_sources(self):
        command = commit_draft.backend_command("claude:haiku")
        self.assertEqual(command[:2], ["claude", "-p"])
        self.assertEqual(command[command.index("--tools") + 1], "")
        self.assertIn("--setting-sources=", command)
        self.assertIn("--strict-mcp-config", command)
        self.assertEqual(command[-2:], ["--model", "haiku"])

    def test_an_agent_that_reads_the_tree_is_not_a_backend(self):
        for spec in ("codex", "codex:o3", "ollama", "cmd:", "gpt"):
            with self.subTest(spec=spec):
                with self.assertRaises(commit_draft.DraftError):
                    commit_draft.backend_command(spec)


class Scratch(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "repo"
        self.root.mkdir()
        self.env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("GIT_")
        }
        self.log = Path(self.temp.name) / "prompts"
        self.replies = Path(self.temp.name) / "replies"
        self.env.update(
            GIT_CONFIG_GLOBAL=os.devnull,
            GIT_CONFIG_NOSYSTEM="1",
            DRAFT_LOG=str(self.log),
            DRAFT_REPLIES=str(self.replies),
        )
        self.git("init", "-q")
        self.git("config", "user.email", "fixture@example.org")
        self.git("config", "user.name", "Fixture")
        self.git("config", "core.hooksPath", os.devnull)
        (self.root / "setup.js").write_text("export const ready = 1;\n")
        self.git("add", ".")
        self.git("commit", "-q", "-m", "Add the setup check\n\nIt starts here.")
        (self.root / "setup.js").write_text("export const ready = 2;\n")
        (self.root / "package-lock.json").write_text('{"lockfileVersion": 3}\n')
        self.git("add", ".")

    def git(self, *args):
        return subprocess.run(
            ["git", *args],
            cwd=self.root,
            env=self.env,
            check=True,
            capture_output=True,
            text=True,
        ).stdout

    STUB = "cmd:%s %s" % (sys.executable, BACKEND)

    def draft(self, *args, backend=STUB):
        """The drafter with the stub backend, another one, or None for the
        configured one."""
        named = ["--backend", backend] if backend else []
        return subprocess.run(
            [sys.executable, str(SCRIPT), *named, *args],
            cwd=self.root,
            env=self.env,
            capture_output=True,
            text=True,
            check=False,
        )

    def prompts(self):
        if not self.log.exists():
            return []
        return [
            (self.log / name).read_text()
            for name in sorted(os.listdir(self.log), key=int)
        ]

    def test_a_refused_draft_goes_back_with_the_refusal(self):
        self.replies.write_text("Updated setup.js\n=====\n" + GOOD)
        result = self.draft("--why", "the picker changes devices")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), GOOD)
        first, second = self.prompts()
        self.assertIn("Author's note on why: the picker changes devices", first)
        self.assertIn("Add the setup check", first)
        self.assertIn("ready = 2", first)
        self.assertIn("lock file package-lock.json changed", first)
        self.assertNotIn("lockfileVersion", first)
        self.assertIn("Updated setup.js", second)
        self.assertIn("imperative mood", second)
        draft_file = self.root / ".git/CODETRIAL_DRAFT_MSG"
        self.assertEqual(draft_file.read_text().strip(), GOOD)

    def test_a_draft_that_never_passes_is_kept_and_reported(self):
        self.replies.write_text("Updated setup.js")
        result = self.draft("--attempts", "2")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(len(self.prompts()), 2)
        self.assertIn("still breaks the rules", result.stderr)

    def test_an_attribution_is_refused_like_any_rule(self):
        self.replies.write_text(
            GOOD + "\n\nCo-Authored-By: Claude <noreply@anthropic.com>\n=====\n" + GOOD
        )
        result = self.draft()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("Co-Authored-By", result.stdout)
        self.assertEqual(len(self.prompts()), 2)

    def test_the_backend_runs_from_an_empty_directory(self):
        self.replies.write_text(GOOD)
        listing = Path(self.temp.name) / "listing"
        stub = " ".join(shlex.quote(str(part)) for part in (sys.executable, BACKEND))
        result = self.draft(
            backend="cmd:ls -A > %s; %s" % (shlex.quote(str(listing)), stub)
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(listing.read_text(), "")

    def test_the_named_backend_wins_over_the_configured_one(self):
        self.git("config", "codetrial.draftBackend", "cmd:exit 3")
        self.replies.write_text(GOOD)
        self.assertEqual(self.draft().returncode, 0)
        result = self.draft(backend=None)
        self.assertEqual(result.returncode, 1)
        self.assertIn("exited 3", result.stderr)

    def test_nothing_is_sent_without_a_backend(self):
        result = self.draft(backend=None)
        self.assertEqual(result.returncode, 1)
        self.assertIn("nothing was sent", result.stderr)
        self.assertEqual(self.prompts(), [])

    def test_the_prompt_can_be_read_without_sending_it(self):
        result = self.draft("--print-prompt")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Rules the repository's commit-msg hook enforces", result.stdout)
        self.assertIn("imperative mood", result.stdout)
        self.assertEqual(self.prompts(), [])

    def test_a_staged_credential_stops_the_draft_before_it_is_sent(self):
        # Split, so the hooks that scan this file do not refuse it.
        (self.root / "key.pem").write_text(
            "-----" + "BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n"
        )
        self.git("add", "key.pem")
        self.replies.write_text(GOOD)
        result = self.draft()
        self.assertEqual(result.returncode, 1)
        self.assertIn("nothing was sent", result.stderr)
        self.assertEqual(self.prompts(), [])

    def test_removed_credential_is_not_sent(self):
        key = "-----" + "BEGIN PRIVATE KEY-----\nsecret\n"
        (self.root / "old.txt").write_text(key)
        self.git("add", "old.txt")
        self.git("commit", "-q", "-m", "Add old content")
        (self.root / "old.txt").write_text("clean\n")
        self.git("add", "old.txt")
        self.replies.write_text(GOOD)
        result = self.draft()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(self.prompts(), [])

    def test_retry_does_not_send_a_credential_from_the_first_reply(self):
        key = "-----" + "BEGIN PRIVATE KEY-----"
        self.replies.write_text("Updated setup.js\n\n" + key)
        result = self.draft()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(len(self.prompts()), 1)
        self.assertNotIn(key, result.stderr)

    def test_quoted_paths_include_their_diffs(self):
        for name in ("caf\u00e9.txt", "tab\tname.txt"):
            (self.root / name).write_text("distinct content\n")
            self.git("add", name)
        result = self.draft("--print-prompt")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.count("+distinct content"), 2)

    def test_nothing_staged_is_said_plainly(self):
        self.git("reset", "-q")
        self.replies.write_text(GOOD)
        result = self.draft()
        self.assertEqual(result.returncode, 1)
        self.assertIn("nothing is staged", result.stderr)


if __name__ == "__main__":
    unittest.main()
