"""Hook-generated links, skipped hooks and metadata-only history repair."""

import hashlib
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "hashchain_support", ROOT / "tests/support.py"
)
support = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(support)


class HashchainTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.env = {
            **os.environ,
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
        }
        self.git("init", "-q")
        self.git("config", "user.name", "Hook Test")
        self.git("config", "user.email", "hooks@example.test")
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "codetrial.hookSuite", "true")
        (self.repo / "sample.txt").write_text("base\n")
        self.git("add", "sample.txt")
        self.git(
            "commit",
            "-qm",
            "Create the base fixture",
            "-m",
            "Keep the legacy anchor outside the new chain.",
        )
        self.base = self.git("rev-parse", "HEAD").stdout.strip()
        (self.repo / "scripts").mkdir()
        for path in (ROOT / "scripts").glob("git-*"):
            shutil.copy2(path, self.repo / "scripts" / path.name)
        for name in [
            "hashchain.py",
            "check-interactive-review.py",
            "check-commit-log.sh",
            "check-secrets.py",
            "install-git-hooks.sh",
            "indent.sh",
        ]:
            shutil.copy2(ROOT / "scripts" / name, self.repo / "scripts" / name)
        for name in [".editorconfig", ".shellcheckrc"]:
            shutil.copy2(ROOT / name, self.repo / name)
        (self.repo / "scripts/hashchain-base").write_text(self.base + "\n")
        self.git("add", "scripts/hashchain-base")
        self.run_command(["sh", "scripts/install-git-hooks.sh"])
        self.commit("Enable the fixture chain")

    def run_command(self, args, *, data=None, check=True):
        return subprocess.run(
            args,
            cwd=self.repo,
            env=self.env,
            input=data,
            text=True,
            capture_output=True,
            check=check,
        )

    def git(self, *args, **kwargs):
        return self.run_command(["git", *args], **kwargs)

    def commit(self, subject):
        return self.git(
            "commit",
            "-qm",
            subject,
            "-m",
            "Preserve a concrete reason while exercising the hook protocol.",
        )

    def chain(self, *args, **kwargs):
        return self.run_command(
            [sys.executable, "scripts/hashchain.py", *args], **kwargs
        )

    def broken(self, copied=False):
        parent = self.git("rev-parse", "HEAD").stdout.strip()
        tree = self.git("rev-parse", "HEAD^{tree}").stdout.strip()
        text = "Add an unchecked fixture\n\nExercise rejection without running the commit hooks.\n"
        if copied:
            text += (
                "\n"
                + next(
                    x
                    for x in self.git(
                        "show", "-s", "--format=%B", parent
                    ).stdout.splitlines()
                    if x.startswith("Change-Id:")
                )
                + "\n"
            )
        return self.git("commit-tree", tree, "-p", parent, data=text).stdout.strip()

    def integration(self):
        start = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("checkout", "-qb", "topic")
        (self.repo / "topic.txt").write_text("topic\n")
        self.git("add", "topic.txt")
        self.commit("Add the topic fixture")
        topic = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("checkout", "-qb", "integration", start)
        (self.repo / "main.txt").write_text("main\n")
        self.git("add", "main.txt")
        self.commit("Advance the main fixture")
        main = self.git("rev-parse", "HEAD").stdout.strip()
        tree = self.git("merge-tree", "--write-tree", main, topic).stdout.splitlines()[
            0
        ]
        merged = self.git(
            "commit-tree",
            tree,
            "-p",
            main,
            "-p",
            topic,
            data="Merge the reviewed topic\n",
        ).stdout.strip()
        self.git("update-ref", "HEAD", merged)
        self.git("reset", "--hard", merged)
        return merged, main, topic

    def test_clean_github_merge_and_following_hook_commit_verify(self):
        merged, _, _ = self.integration()
        self.assertNotIn(
            "Change-Id:", self.git("show", "-s", "--format=%B", merged).stdout
        )
        self.chain("verify")
        (self.repo / "after.txt").write_text("after\n")
        self.git("add", "after.txt")
        self.commit("Continue after integration")
        self.chain("verify")
        self.git("commit", "--amend", "--no-edit")
        self.chain("verify")

    def test_nested_queue_integrations_verify_every_topic_branch(self):
        merged, main, _ = self.integration()
        self.git("checkout", "-qb", "next-queued-topic", main)
        (self.repo / "queued.txt").write_text("queued topic\n")
        self.git("add", "queued.txt")
        self.commit("Add the next queued change")
        topic = self.git("rev-parse", "HEAD").stdout.strip()
        tree = self.git(
            "merge-tree", "--write-tree", merged, topic
        ).stdout.splitlines()[0]
        queued = self.git(
            "commit-tree",
            tree,
            "-p",
            merged,
            "-p",
            topic,
            data="Merge the next queued topic\n",
        ).stdout.strip()
        self.chain("verify", "--tip", queued)

    def test_merge_cannot_hide_an_unchecked_parent_or_extra_tree_edits(self):
        merged, main, topic = self.integration()
        bad = self.broken()
        tree = self.git("rev-parse", merged + "^{tree}").stdout.strip()
        forged = self.git(
            "commit-tree",
            tree,
            "-p",
            merged,
            "-p",
            bad,
            data="Merge an unchecked parent\n",
        ).stdout.strip()
        self.assertNotEqual(
            self.chain("verify", "--tip", forged, check=False).returncode, 0
        )
        (self.repo / "injected.txt").write_text("not in either reviewed parent\n")
        self.git("add", "injected.txt")
        injected_tree = self.git("write-tree").stdout.strip()
        forged = self.git(
            "commit-tree",
            injected_tree,
            "-p",
            main,
            "-p",
            topic,
            data="Merge with an extra unreviewed edit\n",
        ).stdout.strip()
        result = self.chain("verify", "--tip", forged, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("without extra edits", result.stderr)

    def test_historical_merges_ignore_descendant_and_local_attribute_changes(self):
        lines = [f"line {index}\n" for index in range(10)]
        path = self.repo / "shared.txt"
        path.write_text("".join(lines))
        self.git("add", "shared.txt")
        self.commit("Create the shared merge fixture")
        start = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("checkout", "-qb", "attr-topic")
        path.write_text("topic\n" + "".join(lines[1:]))
        self.git("add", "shared.txt")
        self.commit("Change the first shared line")
        topic = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("checkout", "-qb", "attr-main", start)
        path.write_text("".join(lines[:-1]) + "main\n")
        self.git("add", "shared.txt")
        self.commit("Change the last shared line")
        main = self.git("rev-parse", "HEAD").stdout.strip()
        tree = self.git("merge-tree", "--write-tree", main, topic).stdout.splitlines()[
            0
        ]
        merged = self.git(
            "commit-tree",
            tree,
            "-p",
            main,
            "-p",
            topic,
            data="Merge disjoint changes to the shared file\n",
        ).stdout.strip()
        self.git("reset", "--hard", merged)
        self.chain("verify")
        (self.repo / ".gitattributes").write_text("*.txt -merge\n")
        self.git("add", ".gitattributes")
        self.commit("Set attributes after the integration")
        self.chain("verify")
        (self.repo / ".git/info/attributes").write_text("*.txt merge=fixture\n")
        self.git("config", "merge.fixture.driver", "false")
        self.chain("verify")

    def test_conflicting_integration_and_octopus_merges_are_rejected(self):
        merged, main, _ = self.integration()
        self.git("checkout", "-qb", "conflict", main)
        (self.repo / "main.txt").write_text("conflict\n")
        self.git("add", "main.txt")
        self.commit("Change the shared fixture")
        left = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("checkout", "-qb", "other-conflict", main)
        (self.repo / "main.txt").write_text("other conflict\n")
        self.git("add", "main.txt")
        self.commit("Change the competing fixture")
        right = self.git("rev-parse", "HEAD").stdout.strip()
        tree = self.git("rev-parse", right + "^{tree}").stdout.strip()
        forged = self.git(
            "commit-tree",
            tree,
            "-p",
            left,
            "-p",
            right,
            data="Resolve a conflict in integration\n",
        ).stdout.strip()
        object_count = self.git("count-objects", "-v").stdout
        self.assertNotEqual(
            self.chain("verify", "--tip", forged, check=False).returncode, 0
        )
        self.assertEqual(self.git("count-objects", "-v").stdout, object_count)
        octopus = self.git(
            "commit-tree",
            tree,
            "-p",
            merged,
            "-p",
            left,
            "-p",
            right,
            data="Merge three parents\n",
        ).stdout.strip()
        self.assertNotEqual(
            self.chain("verify", "--tip", octopus, check=False).returncode, 0
        )

    def test_rechain_preserves_integration_and_repairs_only_the_linear_suffix(self):
        merged, _, _ = self.integration()
        original = self.git("cat-file", "commit", merged).stdout
        bad = self.broken()
        self.git("update-ref", "HEAD", bad)
        bad = self.broken()
        self.git("update-ref", "HEAD", bad)
        self.chain("rechain")
        self.chain("verify")
        self.assertEqual(self.git("rev-parse", "HEAD~2").stdout.strip(), merged)
        self.assertEqual(self.git("cat-file", "commit", merged).stdout, original)

    def test_unsupported_git_fails_before_attempting_history_inspection(self):
        bin_dir = self.repo / "bin"
        bin_dir.mkdir()
        executable = bin_dir / "git"
        executable.write_text(
            f"#!{sys.executable}\nimport sys\nfrom pathlib import Path\n"
            "if sys.argv[1:]==['--version']:\n print('git version 2.44.0');sys.exit(0)\n"
            "Path('history-was-read').touch();sys.exit(87)\n"
        )
        executable.chmod(0o700)
        self.env["PATH"] = str(bin_dir) + os.pathsep + self.env["PATH"]
        result = self.chain("verify", check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("requires Git 2.45", result.stderr)
        self.assertFalse((self.repo / "history-was-read").exists())

    def test_normal_commits_and_fresh_message_amends(self):
        self.chain("verify")
        (self.repo / "sample.txt").write_text("changed\n")
        self.git("add", "sample.txt")
        self.commit("Change the sample value")
        self.git(
            "commit",
            "--amend",
            "-qm",
            "Clarify the sample reason",
            "-m",
            "A fresh message must still continue the parent link.",
        )
        self.chain("verify")
        message = self.git("show", "-s", "--format=%B").stdout
        self.assertEqual(message.count("Change-Id:"), 1)

    def test_correcting_a_signed_commit_refuses_instead_of_unsigning_it(self):
        head = self.git("rev-parse", "HEAD").stdout.strip()
        raw = self.git("cat-file", "commit", head).stdout
        headers, text = raw.split("\n\n", 1)
        signed = (
            headers
            + "\ngpgsig -----BEGIN PGP SIGNATURE-----\n \n fixture\n"
            + " -----END PGP SIGNATURE-----\n\n"
            + text
        )
        sha = self.git("hash-object", "-t", "commit", "-w", "--stdin", data=signed)
        code = (
            "import importlib.util,sys;"
            "s=importlib.util.spec_from_file_location('h','scripts/hashchain.py');"
            "m=importlib.util.module_from_spec(s);s.loader.exec_module(m);"
            "m.rewrite(sys.argv[1],m.commit(sys.argv[1])[3][0],'0'*40)"
        )
        result = self.run_command(
            [sys.executable, "-c", code, sha.stdout.strip()], check=False
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("is signed", result.stderr)

    def test_change_id_joins_an_existing_trailer_block(self):
        (self.repo / "sample.txt").write_text("signed\n")
        self.git("add", "sample.txt")
        self.git(
            "commit",
            "-qs",
            "-m",
            "Record a signed-off fixture",
            "-m",
            "The sign-off must stay with the trailers at the end.",
        )
        self.chain("verify")
        trailers = self.git(
            "log", "-1", "--format=%(trailers:only,unfold)"
        ).stdout.splitlines()
        self.assertEqual(
            [t.split(":")[0] for t in trailers if t],
            ["Signed-off-by", "Change-Id"],
        )

    def test_commit_during_a_rebase_does_not_demand_a_repaired_chain(self):
        broken = self.broken()
        self.git("reset", "-q", "--soft", broken)
        (self.repo / "sample.txt").write_text("replayed\n")
        self.git("add", "sample.txt")
        refused = self.run_command(
            ["git", "commit", "-qm", "Reword mid series", "-m", "Body text here."],
            check=False,
        )
        self.assertNotEqual(refused.returncode, 0)
        (self.repo / ".git/rebase-merge").mkdir()
        self.commit("Reword mid series")
        self.assertIn("Change-Id:", self.git("log", "-1", "--format=%B").stdout)

    def dependabot_commit(self, parent=None, message=None):
        parent = parent or self.git("rev-parse", "HEAD").stdout.strip()
        tree = self.git("rev-parse", parent + "^{tree}").stdout.strip()
        env = {
            **self.env,
            "GIT_AUTHOR_NAME": "dependabot[bot]",
            "GIT_AUTHOR_EMAIL": "49699333+dependabot[bot]@users.noreply.github.com",
        }
        return subprocess.run(
            ["git", "commit-tree", tree, "-p", parent],
            cwd=self.repo,
            env=env,
            input=message
            or "Bump the fixture dependency\n\nThe bot cannot run local hooks.\n",
            text=True,
            capture_output=True,
            check=True,
        ).stdout.strip()

    def test_hooked_commit_follows_a_trailerless_bot_without_restamping_it(self):
        parent = self.dependabot_commit()
        self.git("reset", "--hard", parent)
        self.chain("verify")
        (self.repo / "sample.txt").write_text("ordinary child of a trailerless bot\n")
        self.git("add", "sample.txt")
        self.commit("Continue after Dependabot")
        self.chain("verify")
        self.assertEqual(self.git("rev-parse", "HEAD^").stdout.strip(), parent)
        unchecked = self.broken()
        self.assertNotEqual(
            self.chain("verify", "--tip", unchecked, check=False).returncode, 0
        )

    def test_hooked_commit_and_fresh_message_amend_continue_after_bot_parents(self):
        for origin in ("anchor", "ordinary", "merge"):
            with self.subTest(origin=origin):
                if origin == "merge":
                    parent, _, _ = self.integration()
                else:
                    parent = (
                        self.base
                        if origin == "anchor"
                        else self.git("rev-parse", "HEAD").stdout.strip()
                    )
                first = self.dependabot_commit(parent)
                second = self.dependabot_commit(
                    first,
                    "Bump another dependency\n\nPreserve bot metadata.\n\nChange-Id: I"
                    + "0" * 40
                    + "\n",
                )
                self.chain("verify", "--tip", second, "--anchor", self.base)
                self.git("reset", "--hard", second)
                (self.repo / "scripts/hashchain-base").write_text(self.base + "\n")
                (self.repo / "sample.txt").write_text(
                    f"human change after {origin} bot\n"
                )
                self.git("add", "sample.txt", "scripts/hashchain-base")
                self.commit("Continue after the bot updates")
                self.chain("verify")
                self.git(
                    "commit",
                    "--amend",
                    "-qm",
                    "Clarify the following change",
                    "-m",
                    "A fresh message must retain the bot parent link.",
                )
                self.chain("verify")

    def test_rechain_preserves_accepted_bot_metadata_and_relinks_bots_after_repair(
        self,
    ):
        message = "Bump the dependency group with a deliberately long fixture update title\n\nKeep this message byte-for-byte.\n"
        accepted = self.dependabot_commit(message=message)
        self.git("reset", "--hard", accepted)
        self.chain("verify")
        self.chain("rechain")
        self.assertEqual(self.git("rev-parse", "HEAD").stdout.strip(), accepted)
        unchecked = self.broken()
        bot = self.dependabot_commit(unchecked, message)
        self.git("reset", "--hard", bot)
        self.chain("rechain")
        self.chain("verify")
        self.assertNotEqual(self.git("rev-parse", "HEAD").stdout.strip(), bot)
        self.assertEqual(self.git("show", "-s", "--format=%B").stdout, message + "\n")
        self.assertNotIn("Change-Id:", self.git("show", "-s", "--format=%B").stdout)

    def test_dependabot_commits_continue_the_chain_without_a_trailer(self):
        tree = self.git("rev-parse", "HEAD^{tree}").stdout.strip()
        parent = self.git("rev-parse", "HEAD").stdout.strip()
        text = "Bump a dependency\n\nA bot cannot run the commit hooks.\n"
        for author, accepted in (
            ("49699333+dependabot[bot]@users.noreply.github.com", True),
            ("someone@example.test", False),
        ):
            env = {
                **self.env,
                "GIT_AUTHOR_NAME": "dependabot[bot]",
                "GIT_AUTHOR_EMAIL": author,
            }
            sha = subprocess.run(
                ["git", "commit-tree", tree, "-p", parent],
                cwd=self.repo,
                env=env,
                input=text,
                text=True,
                capture_output=True,
                check=True,
            ).stdout.strip()
            result = self.chain("verify", "--tip", sha, check=False)
            self.assertEqual(result.returncode == 0, accepted, result.stderr)

    def test_a_body_line_that_quotes_a_trailer_is_refused_not_dropped(self):
        (self.repo / "sample.txt").write_text("quoted\n")
        self.git("add", "sample.txt")
        body = (
            "The hook writes a line such as:\n\n"
            "Change-Id: I0000000000000000000000000000000000000000\n\n"
            "and this paragraph says why."
        )
        refused = self.git(
            "commit", "-q", "-m", "Explain the trailer", "-m", body, check=False
        )
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("Only the generated trailer may begin with", refused.stderr)
        self.assertIn("Change-Id: I0000", refused.stderr)
        # Indented, the same example commits and verifies.
        self.git(
            "commit",
            "-q",
            "-m",
            "Explain the trailer",
            "-m",
            body.replace("\nChange-Id:", "\n    Change-Id:"),
        )
        self.chain("verify")

    def test_an_untracked_policy_file_is_not_a_policy(self):
        with tempfile.TemporaryDirectory() as other:

            def run(*args):
                return subprocess.run(
                    args, cwd=other, env=self.env, text=True, capture_output=True
                )

            run("git", "init", "-q")
            run("git", "config", "user.name", "t")
            run("git", "config", "user.email", "t@example.test")
            Path(other, "a.txt").write_text("a\n")
            run("git", "add", ".")
            run(
                "git",
                "commit",
                "-qm",
                "No policy yet",
                "-m",
                "Nothing names an anchor.",
            )
            Path(other, "scripts").mkdir()
            Path(other, "scripts/hashchain-base").write_text("f" * 40 + "\n")
            result = run(sys.executable, str(ROOT / "scripts/hashchain.py"), "verify")
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_the_description_check_runs_the_base_branchs_workflow(self):
        text = (ROOT / ".github/workflows/pr-description.yml").read_text()
        self.assertIn("\non:\n  pull_request_target:", text)
        self.assertNotIn("\n  pull_request:", text)
        self.assertIn("permissions:\n  contents: read", text)
        self.assertIn("../base/scripts/check-interactive-review.py", text)
        self.assertIn("../base/scripts/check-queue-review.py", text)
        self.assertIn("\n  merge_group:", text)
        # A job its own condition skips counts as passing, so none may have one.
        self.assertNotRegex(text, r"(?m)^    if:")
        self.assertNotIn("continue-on-error", text)
        self.assertIn("--guard", text)

    def test_one_definition_decides_who_dependabot_is(self):
        chain = importlib.util.module_from_spec(
            importlib.util.spec_from_file_location(
                "hashchain_under_test", ROOT / "scripts/hashchain.py"
            )
        )
        chain.__spec__.loader.exec_module(chain)
        for login, kind, expected in (
            ("dependabot[bot]", "Bot", True),
            ("dependabot[bot]", "User", False),
            ("dependabot[bot]", None, False),
            ("someone", "Bot", False),
        ):
            with self.subTest(login=login, kind=kind):
                self.assertEqual(chain.exempt_pr(login, kind), expected)
        for script in ("check-commit-log.sh", "check-interactive-review.py"):
            self.assertNotIn(
                "dependabot[bot]@", (ROOT / "scripts" / script).read_text(), script
            )

    def test_bots_lists_only_dependabot_authored_commits(self):
        tree = self.git("rev-parse", "HEAD^{tree}").stdout.strip()
        parent = self.git("rev-parse", "HEAD").stdout.strip()
        made = []
        for author in (
            "49699333+dependabot[bot]@users.noreply.github.com",
            "someone@example.test",
        ):
            env = {**self.env, "GIT_AUTHOR_EMAIL": author}
            made.append(
                subprocess.run(
                    ["git", "commit-tree", tree, "-p", parent, "-m", "Bump a"],
                    cwd=self.repo,
                    env=env,
                    text=True,
                    capture_output=True,
                    check=True,
                ).stdout.strip()
            )
        listed = self.chain("bots", data="\n".join(made) + "\n")
        self.assertEqual(listed.stdout.split(), made[:1])
        self.assertEqual(self.chain("bots", "--all", data=made[0] + "\n").returncode, 0)
        self.assertEqual(
            self.chain("bots", "--all", data="\n".join(made), check=False).returncode, 1
        )
        self.assertEqual(
            self.chain("bots", "--all", data="", check=False).returncode, 1
        )

    def test_message_only_amend_preserves_index_and_worktree(self):
        tree = self.git("rev-parse", "HEAD^{tree}").stdout
        (self.repo / "sample.txt").write_text("base\nstaged\n")
        self.git("add", "sample.txt")
        index = self.git("write-tree").stdout
        (self.repo / "sample.txt").write_text("base\nstaged\nunstaged\n")
        self.git(
            "commit",
            "--amend",
            "--only",
            "-qm",
            "Clarify the fixture premise",
            "-m",
            "Amending a message must preserve unrelated file edits.",
        )
        self.chain("verify")
        self.assertEqual(self.git("rev-parse", "HEAD^{tree}").stdout, tree)
        self.assertEqual(self.git("write-tree").stdout, index)
        self.assertTrue((self.repo / "sample.txt").read_text().endswith("unstaged\n"))

    def test_post_commit_never_launders_missing_or_copied_ids(self):
        good = self.git("rev-parse", "HEAD").stdout.strip()
        for copied in [False, True]:
            bad = self.broken(copied)
            self.git("update-ref", "HEAD", bad)
            self.chain("fix")
            self.assertEqual(self.git("rev-parse", "HEAD").stdout.strip(), bad)
            self.assertNotEqual(self.chain("verify", check=False).returncode, 0)
            self.git("update-ref", "HEAD", good)

    def test_rechain_keeps_trees_without_adding_repair_trailers(self):
        bad = self.broken()
        self.git("update-ref", "HEAD", bad)
        tree = self.git("rev-parse", "HEAD^{tree}").stdout
        (self.repo / "sample.txt").write_text("an unstaged edit\n")
        self.chain("rechain")
        self.chain("verify")
        self.assertNotEqual(self.git("rev-parse", "HEAD").stdout.strip(), bad)
        self.assertEqual(self.git("rev-parse", "HEAD^{tree}").stdout, tree)
        self.assertNotIn("Rechained:", self.git("show", "-s", "--format=%B").stdout)
        self.assertEqual((self.repo / "sample.txt").read_text(), "an unstaged edit\n")

    def test_push_checks_the_pushed_tip_instead_of_the_checkout(self):
        bad = self.broken()
        line = f"refs/heads/topic {bad} refs/heads/topic {'0' * 40}\n"
        result = self.run_command(
            ["sh", "scripts/git-pre-push.sh", "backup", "local-fixture"],
            data=line,
            check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("hashchain", result.stderr)
        self.chain("verify")

    def test_uncommitted_base_edit_cannot_disable_tip_verification(self):
        (self.repo / "scripts/hashchain-base").write_text("0" * 40 + "\n")
        self.chain("verify")

    def test_trusted_anchor_must_be_an_immutable_full_sha(self):
        for anchor in ("HEAD", self.base[:12], ""):
            result = self.chain("verify", "--anchor", anchor, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("full commit SHA", result.stderr)

    def test_replacement_objects_cannot_hide_an_unchecked_commit(self):
        tip = self.broken()
        self.git("replace", tip, self.base)
        self.assertNotEqual(
            self.chain("verify", "--tip", tip, check=False).returncode, 0
        )

    def test_grafts_and_shallow_history_cannot_clear_verification(self):
        tip = self.broken()
        grafts = self.repo / ".git/info/grafts"
        grafts.write_text(tip + "\n")
        self.assertNotEqual(
            self.chain("verify", "--tip", tip, check=False).returncode, 0
        )
        grafts.unlink()
        (self.repo / ".git/shallow").write_text(tip + "\n")
        self.assertNotEqual(
            self.chain("verify", "--tip", tip, check=False).returncode, 0
        )

    def test_trusted_anchor_rejects_a_candidate_policy_reset(self):
        trusted = self.git("rev-parse", "HEAD").stdout.strip()
        bad = self.broken()
        (self.repo / "scripts/hashchain-base").write_text(bad + "\n")
        self.git("add", "scripts/hashchain-base")
        tree = self.git("write-tree").stdout.strip()
        seed = hashlib.sha256(("codetrial/" + bad).encode()).hexdigest()[:40]
        link = hashlib.sha256((seed + ":" + tree).encode()).hexdigest()[:40]
        tip = self.git(
            "commit-tree",
            tree,
            "-p",
            bad,
            data=f"Reset the fixture policy\n\nExpose candidate-controlled anchors.\n\nChange-Id: I{link}\n",
        ).stdout.strip()
        self.chain("verify", "--tip", tip)
        self.assertNotEqual(
            self.chain(
                "verify", "--tip", tip, "--anchor", self.base, check=False
            ).returncode,
            0,
        )
        self.git("remote", "add", "upstream", "https://github.com/sysprog21/codetrial")
        self.git("update-ref", "refs/remotes/upstream/main", trusted)
        result = self.run_command(
            ["sh", "scripts/git-pre-push.sh", "backup", "local-fixture"],
            data=f"refs/heads/topic {tip} refs/heads/topic {'0' * 40}\n",
            check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("hashchain", result.stderr)

    def test_trusted_anchor_rejects_policy_edits_with_valid_old_chain_links(self):
        parent = self.git("rev-parse", "HEAD").stdout.strip()
        previous = next(
            line.removeprefix("Change-Id: I")
            for line in self.git(
                "show", "-s", "--format=%B", parent
            ).stdout.splitlines()
            if line.startswith("Change-Id: I")
        )
        self.chain("verify", "--anchor", self.base)
        for value in (parent + "\n", "", "invalid\n", None):
            with self.subTest(policy=value):
                self.git("reset", "--hard", parent)
                policy = self.repo / "scripts/hashchain-base"
                if value is None:
                    policy.unlink()
                else:
                    policy.write_text(value)
                self.git("add", "scripts/hashchain-base")
                tree = self.git("write-tree").stdout.strip()
                link = hashlib.sha256((previous + ":" + tree).encode()).hexdigest()[:40]
                tip = self.git(
                    "commit-tree",
                    tree,
                    "-p",
                    parent,
                    data=f"Change the fixture policy\n\nRetain valid old-anchor links.\n\nChange-Id: I{link}\n",
                ).stdout.strip()
                result = self.chain(
                    "verify", "--tip", tip, "--anchor", self.base, check=False
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("must retain the trusted anchor", result.stderr)


class HookMergeSafetyTests(unittest.TestCase):
    def test_concluding_merge_skips_incoming_scripts_but_runs_user_local_hook(self):
        for incoming in ("hashchain.py", "git-post-commit.sh"):
            with (
                self.subTest(incoming=incoming),
                tempfile.TemporaryDirectory() as directory,
            ):
                repo = Path(directory)
                env = {
                    **os.environ,
                    "GIT_CONFIG_GLOBAL": os.devnull,
                    "GIT_CONFIG_NOSYSTEM": "1",
                }

                def git(*args, data=None):
                    return subprocess.check_output(
                        ["git", *args], cwd=repo, env=env, input=data, text=True
                    ).strip()

                git("init", "-q")
                git("config", "user.name", "Merge Test")
                git("config", "user.email", "merge@example.test")
                git("config", "commit.gpgsign", "false")
                (repo / "scripts").mkdir()
                for name in ("install-git-hooks.sh", "git-post-commit.sh"):
                    shutil.copy2(ROOT / "scripts" / name, repo / "scripts" / name)
                (repo / "scripts/hashchain.py").write_text("pass\n")
                (repo / "f").write_text("base\n")
                git("add", ".")
                git("commit", "-qm", "Create fixture")
                base = git("rev-parse", "HEAD")
                payload = (
                    'from pathlib import Path\nPath("incoming-ran").touch()\n'
                    if incoming.endswith(".py")
                    else "#!/bin/sh\ntouch incoming-ran\n"
                )
                (repo / "scripts" / incoming).write_text(payload)
                git("add", "scripts")
                tip = git(
                    "commit-tree",
                    git("write-tree"),
                    "-p",
                    base,
                    data="Incoming fixture\n",
                )
                git("reset", "--hard", base)
                (repo / "f").write_text("main\n")
                git("add", "f")
                git("commit", "-qm", "Advance main")
                subprocess.run(
                    ["sh", "scripts/install-git-hooks.sh"],
                    cwd=repo,
                    env=env,
                    check=True,
                    capture_output=True,
                )
                local = repo / ".git/hooks/post-commit.local"
                local.write_text("#!/bin/sh\ntouch local-ran\n")
                local.chmod(0o700)
                git("merge", "--no-commit", tip)
                self.assertTrue((repo / ".git/MERGE_HEAD").exists())
                git("commit", "-qm", "Conclude integration")
                self.assertFalse((repo / ".git/MERGE_HEAD").exists())
                self.assertFalse((repo / "incoming-ran").exists())
                self.assertTrue((repo / "local-ran").exists())


class BotLinkDepthTests(unittest.TestCase):
    def test_long_bot_run_does_not_use_python_recursion(self):
        spec = importlib.util.spec_from_file_location(
            "bot_link_depth", ROOT / "scripts/hashchain.py"
        )
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        anchor, tree = "a" * 40, "b" * 40
        header = b"author dependabot[bot] <49699333+dependabot[bot]@users.noreply.github.com> 0 +0000"

        def commit(sha):
            index = int(sha, 16)
            return (
                header,
                b"No trailer\n",
                tree,
                [anchor if index == 0 else f"{index - 1:040x}"],
            )

        expected = hashlib.sha256(("codetrial/" + anchor).encode()).hexdigest()[:40]
        for _ in range(1500):
            expected = hashlib.sha256((expected + ":" + tree).encode()).hexdigest()[:40]
        with patch.object(module, "commit", side_effect=commit):
            self.assertEqual(module.link(f"{1499:040x}", anchor), expected)


class HashchainWorkflowTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.env = {
            **os.environ,
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
            "CHAIN_BOOTSTRAP_HEAD": "",
        }
        self.git("init", "-q")
        self.git("config", "user.name", "Workflow Test")
        self.git("config", "user.email", "workflow@example.test")
        self.git("config", "commit.gpgsign", "false")
        (self.repo / "base.txt").write_text("base\n")
        self.base = self.commit()
        self.script = support.run_block(
            support.step(
                (ROOT / ".github/workflows/check.yml").read_text(),
                "Verify the Change-Id hashchain",
            )
        )

    def git(self, *args):
        return subprocess.check_output(
            ["git", *args], cwd=self.repo, env=self.env, text=True
        ).strip()

    def commit(self):
        self.git("add", ".")
        self.git("commit", "-qm", "Record the workflow fixture")
        return self.git("rev-parse", "HEAD")

    def verifier(self, marker):
        scripts = self.repo / "scripts"
        scripts.mkdir(exist_ok=True)
        (scripts / "hashchain.py").write_text(
            f"from pathlib import Path\nimport sys\nPath({marker!r}).write_text(' '.join(sys.argv[1:]))\n"
        )

    def run_step(self, tip, policy, approval=""):
        return subprocess.run(
            ["bash", "-eo", "pipefail", "-c", self.script],
            cwd=self.repo,
            env={
                **self.env,
                "CHAIN_TIP": tip,
                "CHAIN_POLICY_REF": policy,
                "CHAIN_BOOTSTRAP_HEAD": approval,
            },
            capture_output=True,
            text=True,
        )

    def real_bootstrap(self):
        anchor = (ROOT / "scripts/hashchain-base").read_text().strip()
        self.script = self.script.replace(
            "bootstrap=" + anchor, "bootstrap=" + self.base
        )
        scripts = self.repo / "scripts"
        scripts.mkdir()
        shutil.copy2(ROOT / "scripts/hashchain.py", scripts / "hashchain.py")
        (scripts / "hashchain-base").write_text(self.base + "\n")
        self.git("add", ".")
        tree = self.git("write-tree")
        seed = hashlib.sha256(("codetrial/" + self.base).encode()).hexdigest()[:40]
        value = hashlib.sha256((seed + ":" + tree).encode()).hexdigest()[:40]
        approved = self.raw_commit(
            tree,
            self.base,
            message="Install the reviewed verifier\n\nChange-Id: I" + value + "\n",
        )
        return approved, tree

    def raw_commit(self, tree, *parents, message="Merge the reviewed fixture\n"):
        args = ["git", "commit-tree", tree]
        for parent in parents:
            args.extend(["-p", parent])
        return subprocess.check_output(
            args, input=message, text=True, cwd=self.repo, env=self.env
        ).strip()

    def test_real_bootstrap_accepts_merge_queue_and_main_integration_then_next_pr(self):
        approved, tree = self.real_bootstrap()
        merged = self.raw_commit(tree, self.base, approved)
        result = self.run_step(merged, self.base, approved)
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.run_step(merged, approved)
        self.assertEqual(result.returncode, 0, result.stderr)
        parent_link = hashlib.sha256(
            ("codetrial/merge:" + merged).encode()
        ).hexdigest()[:40]
        value = hashlib.sha256((parent_link + ":" + tree).encode()).hexdigest()[:40]
        following = self.raw_commit(
            tree,
            merged,
            message="Continue from integrated main\n\nChange-Id: I" + value + "\n",
        )
        result = self.run_step(following, merged)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_real_bootstrap_rejects_extra_content_even_with_approved_source(self):
        approved, _ = self.real_bootstrap()
        (self.repo / "injected.txt").write_text("not in the approved head\n")
        self.git("add", ".")
        tree = self.git("write-tree")
        forged = self.raw_commit(tree, self.base, approved)
        result = self.run_step(forged, self.base, approved)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("without extra edits", result.stderr)

    def test_bootstrap_integration_cannot_replace_the_approved_verifier(self):
        approved, _ = self.real_bootstrap()
        self.verifier("candidate-ran")
        self.git("add", ".")
        forged = self.raw_commit(self.git("write-tree"), self.base, approved)
        result = self.run_step(forged, self.base, approved)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("changed the approved verifier", result.stderr)
        self.assertFalse((self.repo / "candidate-ran").exists())

    def test_workflow_checks_merge_groups_against_their_trusted_base(self):
        step = support.step(
            (ROOT / ".github/workflows/check.yml").read_text(),
            "Verify the Change-Id hashchain",
        )
        self.assertIn("github.event_name == 'merge_group'", step)
        self.assertIn("github.event.merge_group.base_sha", step)
        self.assertIn("github.event.merge_group.head_sha", step)

    def test_unapproved_bootstrap_never_executes_candidate_code(self):
        self.verifier("candidate-ran")
        tip = self.commit()
        for approval in ("", self.base):
            result = self.run_step(tip, self.base, approval)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("maintainer approval", result.stderr)
        self.assertFalse((self.repo / "candidate-ran").exists())

    def test_bootstrap_executes_only_the_exact_approved_committed_verifier(self):
        self.verifier("approved-ran")
        (self.repo / "scripts/hashchain-base").write_text(
            (ROOT / "scripts/hashchain-base").read_text()
        )
        tip = self.commit()
        self.verifier("working-tree-ran")
        result = self.run_step(tip, self.base, tip)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.repo / "approved-ran").exists())
        self.assertFalse((self.repo / "working-tree-ran").exists())
        self.assertNotEqual(self.run_step("HEAD", self.base, "HEAD").returncode, 0)

    def test_bootstrap_cannot_change_the_initial_policy_boundary(self):
        self.verifier("candidate-ran")
        (self.repo / "scripts/hashchain-base").write_text(self.base + "\n")
        tip = self.commit()
        result = self.run_step(tip, self.base, tip)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("fixed initial anchor", result.stderr)
        self.assertFalse((self.repo / "candidate-ran").exists())

    def test_installed_policy_uses_trusted_code_and_anchor(self):
        self.verifier("trusted-ran")
        (self.repo / "scripts/hashchain-base").write_text(self.base + "\n")
        trusted = self.commit()
        self.verifier("candidate-ran")
        (self.repo / "scripts/hashchain-base").write_text(trusted + "\n")
        tip = self.commit()
        result = self.run_step(tip, trusted)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--anchor " + self.base, (self.repo / "trusted-ran").read_text())
        self.assertFalse((self.repo / "candidate-ran").exists())

    def test_missing_trusted_verifier_does_not_fall_back_to_candidate(self):
        (self.repo / "scripts").mkdir()
        (self.repo / "scripts/hashchain-base").write_text(self.base + "\n")
        trusted = self.commit()
        self.verifier("candidate-ran")
        tip = self.commit()
        self.assertNotEqual(self.run_step(tip, trusted, tip).returncode, 0)
        self.assertFalse((self.repo / "candidate-ran").exists())
