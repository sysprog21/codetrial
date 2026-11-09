"""Large-push rejection and durable interactive evidence using real Git objects."""

import importlib.util
import json
import os
import re
from pathlib import Path
import select
import shutil
import subprocess
import sys
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    "support", Path(__file__).with_name("support.py")
)
support = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(support)

ROOT = Path(__file__).resolve().parents[1]
GATE = ROOT / "scripts/check-interactive-review.py"


def load_gate(name="gate_under_test"):
    spec = importlib.util.spec_from_file_location(name, GATE)
    gate = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(gate)
    return gate


SKILL = ROOT / ".claude/skills/codetrial-review"


class InteractiveHookTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.env = {
            **os.environ,
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
        }
        self.git("init", "-q")
        self.git("config", "user.name", "Review Test")
        self.git("config", "user.email", "review@example.test")
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "codetrial.hookSuite", "true")
        # The contributor's own SSH key, the one on their GitHub account.
        self.key = self.root / "id_ed25519"
        subprocess.run(
            ["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "contributor"]
            + ["-f", str(self.key)],
            check=True,
        )
        self.git("config", "codetrial.signingKey", str(self.key))
        self.author_keys = self.root / "contributor.keys"
        self.author_keys.write_text(Path(str(self.key) + ".pub").read_text())
        (self.repo / "change.txt").write_text("base\n")
        self.commit("Create the review fixture")
        self.base = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("remote", "add", "upstream", "https://github.com/sysprog21/codetrial")
        self.git("update-ref", "refs/remotes/upstream/main", self.base)
        # Where the branch goes: the pre-push hook sends the review record here.
        self.fork = self.root / "fork.git"
        self.git("init", "--bare", "-q", str(self.fork))
        self.git("remote", "add", "fork", str(self.fork))
        (self.repo / "scripts").mkdir()
        for name in [
            "git-pre-push.sh",
            "git-commit-msg.sh",
            "git-commit-msg.awk",
            "check-commit-log.sh",
            "check-secrets.py",
            "check-interactive-review.py",
            "hashchain.py",
            "indent.sh",
        ]:
            shutil.copy2(ROOT / "scripts" / name, self.repo / "scripts" / name)
        (self.repo / ".claude/skills/codetrial-review").mkdir(parents=True)
        (self.repo / ".claude/skills/codetrial-review/SKILL.md").write_text("fixture\n")
        shutil.copy2(
            SKILL / "commit-sizes.py",
            self.repo / ".claude/skills/codetrial-review/commit-sizes.py",
        )
        (self.repo / ".git/info/exclude").write_text("/scripts/\n/.claude/\n")

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

    def commit(self, title):
        self.git("add", "change.txt")
        self.git(
            "commit",
            "-qm",
            title,
            "-m",
            "Exercise the review boundary with local fixture data.",
        )

    def push(self, check=False):
        head = self.git("rev-parse", "HEAD").stdout.strip()
        return self.run_command(
            ["sh", "scripts/git-pre-push.sh", "fork", str(self.fork)],
            data=f"refs/heads/topic {head} refs/heads/topic {'0' * 40}\n",
            check=check,
        )

    def large(self, count=200):
        (self.repo / "change.txt").write_text("base\n" + "line\n" * count)
        self.commit("Add the reviewable fixture")

    def prepare(self, approved=True, questions=None):
        bundle = self.root / "bundle"
        self.run_command(
            ["sh", str(SKILL / "review-bundle.sh"), "--local", str(bundle), self.base]
        )
        head = self.git("rev-parse", "HEAD").stdout.strip()
        (bundle / "questions.json").write_text(
            json.dumps(
                questions
                if questions is not None
                else [
                    {
                        "id": "motivation",
                        "path": "change.txt",
                        "prompt": "Which fixture boundary is exercised?",
                    },
                ]
            )
        )
        code = """
import importlib.util,sys
from pathlib import Path
spec=importlib.util.spec_from_file_location('fixture_workbench',sys.argv[1])
m=importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
bundle=Path(sys.argv[2]); app=m.Workbench(bundle,bundle/'questions.json')
s=app.snapshot(); app.save({'identity':s['identity'],'revision':s['revision'],'answers':{k:'The fixture tests this boundary against local Git objects.' for k in app.keys}})
s=app.snapshot(); app.request_review({'identity':s['identity'],'revision':s['revision']})
s=app.snapshot(); app.feedback({'identity':s['identity'],'answer_digest':s['answer_digest'],'notes':[{'field':'motivation','text':'Fixture checks are complete.'}]})
if sys.argv[3]=='yes':
 s=app.snapshot(); app.reflect({'identity':s['identity'],'revision':s['revision'],'feedback_digest':s['feedback_digest'],'grade':'got','correction':''})
"""
        self.run_command(
            [
                sys.executable,
                "-c",
                code,
                str(SKILL / "workbench.py"),
                str(bundle),
                "yes" if approved else "no",
            ]
        )
        report = {
            "schema_version": 1,
            "head": head,
            "outcome": "complete",
            "paths": ["change.txt"],
            "skills_used": ["codetrial-review"],
            "findings": [],
            "refuted": [],
            "questions": [],
            "history_evidence": {"used": [], "consulted": [], "rejected": []},
            "coverage": "Complete local fixture review; transport and receipt validation only.",
        }
        report_path = self.root / "report.json"
        report_path.write_text(json.dumps(report))
        self.run_command(
            [
                sys.executable,
                str(SKILL / "review-lifecycle.py"),
                "finish",
                "--db",
                str(self.root / "memory.db"),
                "--bundle",
                str(bundle),
                "--report",
                str(report_path),
            ]
        )
        return bundle, head

    def test_textconv_is_not_used_by_the_bundle_or_certificate(self):
        converter = self.root / "converter.py"
        converter.write_text(
            'from pathlib import Path\nPath("converter-ran").touch()\nprint("converted")\n'
        )
        self.git("config", "diff.fixture.textconv", f"{sys.executable} {converter}")
        (self.repo / ".gitattributes").write_text("change.txt diff=fixture\n")
        self.git("add", ".gitattributes")
        self.large()
        self.prepare()
        self.assertEqual(self.push().returncode, 0)
        self.assertFalse((self.repo / "converter-ran").exists())

    def test_certificate_ignores_unrelated_checkout_attributes_and_diff_config(self):
        self.large()
        self.prepare()
        self.assertEqual(self.push().returncode, 0)
        (self.repo / ".gitattributes").write_text("change.txt -diff\n")
        (self.repo / ".git/info/attributes").write_text("change.txt -diff\n")
        self.git("config", "diff.context", "100")
        self.git("config", "core.abbrev", "4")
        self.assertEqual(self.push().returncode, 0)

    def test_renamed_critical_file_accepts_its_changed_quote(self):
        self.large()
        self.base = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("update-ref", "refs/remotes/upstream/main", self.base)
        self.git("mv", "change.txt", "renamed.txt")
        p = self.repo / "renamed.txt"
        p.write_text(p.read_text().replace("base", "changed", 1))
        self.git("add", "renamed.txt")
        self.git(
            "commit",
            "-qm",
            "Rename the reviewed fixture",
            "-m",
            "Preserve behavior through the renamed review fixture.",
        )
        bundle, _ = self.prepare(
            questions=[
                {
                    "id": "renamed",
                    "path": "renamed.txt",
                    "prompt": "Why preserve this behavior?",
                    "quote": "+changed",
                }
            ]
        )
        self.assertIn("rename to renamed.txt", (bundle / "pr.diff").read_text())
        self.assertEqual(self.push().returncode, 0)

    def test_feedback_retry_keeps_proof_and_revocation_syncs_before_state_ack(self):
        self.large()
        bundle, head = self.prepare()
        before = (bundle / "explanation.json").read_bytes()
        code = """
import importlib.util,json,os,sys
from pathlib import Path
s=importlib.util.spec_from_file_location('w',sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
a=m.Workbench(Path(sys.argv[2]),Path(sys.argv[2])/'questions.json');a.feedback(a.snapshot()['feedback'])
"""
        self.run_command(
            [sys.executable, "-c", code, str(SKILL / "workbench.py"), str(bundle)]
        )
        self.assertEqual((bundle / "explanation.json").read_bytes(), before)
        self.assertEqual(self.push().returncode, 0)
        code += """
seen=[];original=os.fsync
directories=[Path(sys.argv[2]), Path.cwd()/'.git/codetrial-review/interactive']
names={(p.stat().st_dev,p.stat().st_ino):str(p) for p in directories}
def synced(fd):
 stat=os.fstat(fd);seen.append(names.get((stat.st_dev,stat.st_ino),'<state file>'));return original(fd)
os.fsync=synced
v=a.snapshot();v['answers']['motivation']='A changed explanation must revoke confirmation first.'
a.save({'identity':v['identity'],'revision':v['revision'],'answers':v['answers']})
print(json.dumps(seen))
"""
        result = self.run_command(
            [sys.executable, "-c", code, str(SKILL / "workbench.py"), str(bundle)]
        )
        seen = json.loads(result.stdout)
        parent = str(self.repo / ".git/codetrial-review/interactive")
        self.assertIn(parent, seen)
        self.assertLess(seen.index(parent), seen.index(str(bundle)))
        self.assertNotEqual(self.push().returncode, 0)

    def gate(self, command, *args, base=None):
        return self.run_command(
            [sys.executable, "scripts/check-interactive-review.py", command]
            + ["--base", base or self.base, *args],
            check=False,
        )

    def test_a_small_change_is_under_the_thresholds(self):
        self.large(10)
        result = self.gate("pr")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("under the interactive review thresholds", result.stdout)
        self.assertNotIn("::notice", result.stdout)

    def fork_notes(self, *args):
        return self.git(
            "--git-dir",
            str(self.fork),
            "notes",
            "--ref=codetrial-review",
            *args,
            check=False,
        )

    def fork_note(self, head):
        found = self.fork_notes("show", head)
        return json.loads(found.stdout) if found.returncode == 0 else None

    def as_ci(self):
        """Keep only what CI sees: the record the fork holds, not this clone's."""
        self.git("update-ref", "-d", "refs/notes/codetrial-review")
        self.git(
            "fetch",
            "-q",
            "fork",
            "+refs/notes/codetrial-review:refs/notes/codetrial-review",
            check=False,
        )

    def pr(
        self,
        base=None,
        head="HEAD",
        keys=None,
        login="contributor",
        authors=None,
        unavailable=False,
    ):
        """`authors`: the GitHub login of each commit's author, in rev-list
        order, as GitHub lists them; None when GitHub did not. Entries past the
        branch's commits stand for commits it does not have."""
        extra = []
        if authors is not None:
            commits = self.git(
                "rev-list", f"{base or self.base}..{head}"
            ).stdout.split()
            commits += [f"{n:040x}" for n in range(len(authors) - len(commits))]
            listed = self.root / "commit.authors"
            listed.write_text(
                "".join(f"{sha} {name}\n" for sha, name in zip(commits, authors))
            )
            extra = ["--commit-authors", str(listed)]
        return self.gate(
            "pr",
            *["--head", head, "--author-login", login, "--author-type", "User"],
            *["--author-keys", str(keys or self.author_keys)],
            *extra,
            *(["--keys-unavailable"] if unavailable else []),
            base=base,
        )

    def other_key(self):
        other = self.root / "other_ed25519"
        if not other.exists():
            subprocess.run(
                ["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(other)],
                check=True,
            )
        return other

    def reviewed_push(self):
        self.large()
        self.prepare()
        pushed = self.push()
        self.assertEqual(pushed.returncode, 0, pushed.stderr)
        return self.git("rev-parse", "HEAD").stdout.strip()

    def test_pushing_the_same_review_again_does_not_sign_it_again(self):
        head = self.reviewed_push()
        signed = self.git("rev-parse", "refs/notes/codetrial-review").stdout
        self.assertEqual(self.push().returncode, 0)
        self.assertEqual(
            self.git("rev-parse", "refs/notes/codetrial-review").stdout, signed
        )
        # Another key still signs it again.
        self.git("config", "codetrial.signingKey", str(self.other_key()))
        self.assertEqual(self.push().returncode, 0)
        self.assertNotEqual(
            self.review_note(head)["signature"],
            json.loads(self.git("show", f"{signed.strip()}:{head}").stdout)[
                "signature"
            ],
        )

    def test_a_large_change_without_a_review_record_says_how_to_fix_it(self):
        self.large()
        head = self.git("rev-parse", "HEAD").stdout.strip()
        result = self.pr()
        self.assertEqual(result.returncode, 1, result.stdout)
        error = result.stdout.strip()
        self.assertEqual(len(error.splitlines()), 1, error)
        self.assertTrue(
            error.startswith("::error title=Interactive review missing::"), error
        )
        self.assertIn(head[:12], error)
        self.assertIn("make hooks", error)
        self.assertIn("/codetrial-review", error)
        # The record can be sent alone, but that does not rerun the check.
        self.assertIn("--publish-review <fork-remote>", error)
        self.assertIn("close and reopen the pull request", error)
        self.assertNotIn("if nothing changed", error)
        self.assertNotIn("Traceback", result.stderr)

    def publish(self, *args):
        return self.run_command(
            ["sh", "scripts/git-pre-push.sh", "--publish-review", "fork", *args],
            check=False,
        )

    def pushed_without_record(self):
        """A reviewed branch pushed with no hook installed: no record went."""
        self.large()
        self.prepare()
        head = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("push", "-q", "fork", "HEAD:refs/heads/topic")
        self.assertIsNone(self.fork_note(head))
        return head

    def test_publish_review_sends_the_record_a_pushed_branch_lacks(self):
        head = self.pushed_without_record()
        result = self.publish("topic")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(
            f"Sent the interactive review record for {head[:12]}", result.stderr
        )
        self.assertIn("Close and reopen the pull request", result.stderr)
        self.assertEqual(self.fork_note(head)["status"], "complete")
        self.assertEqual(self.git("rev-parse", "HEAD").stdout.strip(), head)
        # The fork's branch was left alone; CI now accepts the record.
        self.assertEqual(
            self.git("--git-dir", str(self.fork), "rev-parse", "topic").stdout.strip(),
            head,
        )
        self.as_ci()
        self.assertEqual(self.pr().returncode, 0)

    def test_publish_review_replaces_a_record_signed_with_another_key(self):
        head = self.pushed_without_record()
        self.git("config", "codetrial.signingKey", str(self.other_key()))
        self.assertEqual(self.publish("topic").returncode, 0)
        wrong = self.fork_note(head)["key_fingerprint"]
        self.as_ci()
        result = self.pr()
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("does not verify", result.stdout)
        self.assertIn("--publish-review <fork-remote>", result.stdout)
        self.assertIn("close and reopen the pull request", result.stdout)
        self.git("config", "codetrial.signingKey", str(self.key))
        result = self.publish("topic")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotEqual(self.fork_note(head)["key_fingerprint"], wrong)
        self.as_ci()
        self.assertEqual(self.pr().returncode, 0)

    def test_publish_review_refuses_a_head_the_fork_does_not_have(self):
        self.large()
        self.prepare()
        result = self.publish("topic")
        self.assertEqual(result.returncode, 1)
        self.assertIn("fork has no branch topic.", result.stderr)
        self.assertIn("--publish-review fork <branch>", result.stderr)
        self.git("push", "-q", "fork", f"{self.base}:refs/heads/topic")
        result = self.publish("topic")
        self.assertEqual(result.returncode, 1)
        self.assertIn(f"fork has {self.base[:12]} on topic, not HEAD", result.stderr)
        self.assertIn("Push the branch instead", result.stderr)
        self.assertEqual(self.fork_notes("list").stdout, "")

    def test_publish_review_finds_the_fork_branch_by_its_exact_name(self):
        head = self.pushed_without_record()
        # Only a branch of exactly that name counts, not one ending in it.
        self.git("push", "-q", "fork", f"{head}:refs/heads/x/refs/heads/other")
        self.assertIn("fork has no branch other.", self.publish("other").stderr)
        # The local branch has another name; the fork's is given, as a name or
        # as a full ref.
        for name in ("topic", "refs/heads/topic"):
            with self.subTest(name=name):
                self.git("push", "fork", ":refs/notes/codetrial-review", check=False)
                result = self.publish(name)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.fork_note(head)["status"], "complete")

    def test_publish_review_says_when_the_fork_cannot_be_read(self):
        self.large()
        self.prepare()
        self.git("remote", "set-url", "fork", str(self.root / "missing.git"))
        result = self.publish("topic")
        self.assertEqual(result.returncode, 1)
        self.assertIn("Cannot read topic from fork", result.stderr)
        self.assertNotIn("has no branch", result.stderr)

    def test_publish_review_says_nothing_was_sent_when_the_check_fails(self):
        self.large()
        self.git("push", "-q", "fork", "HEAD:refs/heads/topic")
        result = self.publish("topic")
        self.assertEqual(result.returncode, 1)
        self.assertIn("Interactive review required", result.stdout + result.stderr)
        self.assertIn(
            "Nothing was sent; the branch on fork is unchanged.", result.stderr
        )

    def test_publish_review_says_why_it_sends_nothing(self):
        self.large(10)
        self.git("push", "-q", "fork", "HEAD:refs/heads/topic")
        result = self.publish("topic")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(
            "has no completed interactive review record to send", result.stderr
        )
        self.git("checkout", "-q", "--detach")
        result = self.publish("topic")
        self.assertEqual(result.returncode, 1)
        self.assertIn("HEAD is detached", result.stderr)
        self.git("checkout", "-q", "-")
        result = self.publish("main")
        self.assertEqual(result.returncode, 1)
        self.assertIn("not main", result.stderr)
        result = self.publish("fork", "topic", "extra")
        self.assertEqual(result.returncode, 1)
        self.assertIn("Usage:", result.stderr)

    def test_a_reviewed_push_sends_the_record_the_check_reads(self):
        head = self.reviewed_push()
        record = self.fork_note(head)
        self.assertEqual(record["status"], "complete")
        # The record says that a review happened, never what was said in it.
        for private in ("answers", "feedback", "questions", "reflection"):
            self.assertNotIn(private, record)
        self.as_ci()
        result = self.pr()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        notice = result.stdout.strip()
        self.assertEqual(len(notice.splitlines()), 1, notice)
        self.assertTrue(notice.startswith("::notice title=Large change::"), notice)
        self.assertIn(head[:12], notice)
        self.assertIn("recorded at push", notice)
        self.assertNotIn("explain one passage", notice)

    def test_a_push_keeps_the_records_the_fork_holds_for_other_branches(self):
        subprocess.run(
            ["git", "--git-dir", str(self.fork), "-c", "user.name=Other"]
            + ["-c", "user.email=other@example.test", "notes"]
            + ["--ref=codetrial-review", "add", "-m", '{"other": true}', self.base],
            check=True,
            capture_output=True,
            env=self.env,
        )
        head = self.reviewed_push()
        self.assertEqual(self.fork_note(self.base), {"other": True})
        self.assertEqual(self.fork_note(head)["status"], "complete")

    def test_the_record_carries_two_fingerprints_and_a_signature(self):
        head = self.reviewed_push()
        record = self.fork_note(head)
        self.assertRegex(record["review_fingerprint"], "^[0-9a-f]{64}$")
        self.assertRegex(record["answer_digest"], "^[0-9a-f]{64}$")
        listed = subprocess.run(
            ["ssh-keygen", "-lf", str(self.key) + ".pub"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
        self.assertIn(record["key_fingerprint"], listed)
        self.assertTrue(record["signature"].startswith("-----BEGIN SSH SIGNATURE-----"))

    def test_a_record_signed_by_a_key_not_on_the_account_is_refused(self):
        self.reviewed_push()
        self.as_ci()
        elsewhere = self.root / "elsewhere.keys"
        elsewhere.write_text(Path(str(self.other_key()) + ".pub").read_text())
        result = self.pr(keys=elsewhere)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn(
            "does not verify against any SSH key on GitHub of contributor",
            result.stdout,
        )
        self.assertIn(self.review_note()["key_fingerprint"], result.stdout)
        for step in ("ssh-keygen -t ed25519", "Key type: Signing Key"):
            self.assertIn(step, result.stdout)
        self.assertIn("codetrial.signingKey", result.stdout)
        nobody = self.root / "none.keys"
        nobody.write_text("")
        result = self.pr(keys=nobody)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("neither contributor nor the commits' authors", result.stdout)

    def test_signing_with_a_key_on_the_account_fixes_the_record(self):
        self.git("config", "codetrial.signingKey", str(self.other_key()))
        self.reviewed_push()
        self.as_ci()
        self.assertEqual(self.pr().returncode, 1)
        self.git("config", "codetrial.signingKey", str(self.key))
        self.assertEqual(self.push().returncode, 0)
        self.as_ci()
        result = self.pr()
        self.assertEqual(result.returncode, 0, result.stdout)

    def test_a_record_altered_after_signing_is_refused(self):
        head = self.reviewed_push()
        original = self.review_note()
        unsigned = {k: v for k, v in original.items() if k != "signature"}
        header, body, *rest = original["signature"].split("\n")
        corrupt = "\n".join(
            [header, ("B" if body[0] != "B" else "C") + body[1:], *rest]
        )
        for record, reason in (
            (unsigned, "is not signed"),
            ({**original, "answer_digest": "0" * 64}, "made for different code"),
            ({**original, "review_fingerprint": "0" * 64}, "made for different code"),
            ({**original, "signature": corrupt}, "does not verify"),
        ):
            with self.subTest(reason=reason):
                self.git(
                    "notes",
                    "--ref=codetrial-review",
                    "add",
                    "-f",
                    "-m",
                    json.dumps(record),
                    head,
                )
                result = self.pr()
                self.assertEqual(result.returncode, 1, result.stdout)
                self.assertIn(reason, result.stdout)
                self.assertNotIn("Traceback", result.stderr)

    def test_a_push_without_a_signing_key_is_refused_with_how_to_set_one(self):
        self.git("config", "--unset", "codetrial.signingKey")
        self.env = {k: v for k, v in self.env.items() if k != "SSH_AUTH_SOCK"}
        self.large()
        self.prepare()
        refused = self.push()
        self.assertNotEqual(refused.returncode, 0)
        # The three one-time steps, spelled out where the contributor hits it.
        for step in (
            "ssh-keygen -t ed25519 -f ~/.ssh/codetrial_signing",
            "Key type: Signing Key",
            "git config codetrial.signingKey ~/.ssh/codetrial_signing",
        ):
            self.assertIn(step, refused.stderr)
        self.assertIsNone(self.fork_note(self.git("rev-parse", "HEAD").stdout.strip()))

    def owners(self):
        # The list counts as the base has it, so it is committed there.
        (self.repo / ".github").mkdir(exist_ok=True)
        (self.repo / ".github/CODEOWNERS").write_text(
            "# @commented-out\n/src/ @Owner @org/team\n/web/ @owner2\n"
        )
        self.git("add", "-f", ".github/CODEOWNERS")
        self.git("commit", "-qm", "Name the owners", "-m", "Fixture owners.")
        self.base = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("update-ref", "refs/remotes/upstream/main", self.base)

    def test_a_branch_cannot_name_its_own_author_a_code_owner(self):
        self.owners()
        (self.repo / ".github/CODEOWNERS").write_text("/src/ @contributor\n")
        self.git("add", "-f", ".github/CODEOWNERS")
        self.large()
        commits = int(self.git("rev-list", "--count", f"{self.base}..HEAD").stdout)
        # GitHub lists every commit as the contributor's, so only the owner
        # list decides; the branch's own CODEOWNERS must not count.
        result = self.pr(login="contributor", authors=["contributor"] * commits)
        self.assertEqual(result.returncode, 1)
        self.git("config", "codetrial.codeOwner", "contributor")
        self.assertNotEqual(self.push().returncode, 0)

    def test_an_unreadable_key_list_is_not_reported_as_missing_setup(self):
        self.reviewed_push()
        self.as_ci()
        empty = self.root / "none.keys"
        empty.write_text("")
        result = self.pr(keys=empty, unavailable=True)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("could not read the SSH keys of contributor", result.stdout)
        self.assertNotIn("ssh-keygen -t ed25519", result.stdout)
        # Keys that were read but do not match are still an outage while some
        # were not read: the matching one may be among those.
        elsewhere = self.root / "elsewhere.keys"
        elsewhere.write_text(Path(str(self.other_key()) + ".pub").read_text())
        result = self.pr(keys=elsewhere, unavailable=True)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("could not read the SSH keys of contributor", result.stdout)
        self.assertNotIn("ssh-keygen -t ed25519", result.stdout)

    def test_a_record_made_against_a_commit_of_the_branch_is_refused(self):
        # Signed honestly, but against the branch's own first commit, so it
        # covers only what came after it.
        (self.repo / "early.txt").write_text("early\n")
        self.git("add", "early.txt")
        self.git("commit", "-qm", "Add an early file", "-m", "Fixture commit.")
        start, self.base = self.base, self.git("rev-parse", "HEAD").stdout.strip()
        self.git("update-ref", "refs/remotes/upstream/main", self.base)
        self.reviewed_push()
        self.base = start
        self.git("update-ref", "refs/remotes/upstream/main", start)
        self.as_ci()
        result = self.pr()
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("was made against another base", result.stdout)

    def test_an_owner_list_naming_another_commit_withholds_the_exemption(self):
        self.owners()
        self.large()
        listed = self.root / "stale.authors"
        listed.write_text(f"{'0' * 40} owner\n")
        result = self.gate(
            "pr",
            *["--head", "HEAD", "--author-login", "owner", "--author-type", "User"],
            *["--author-keys", str(self.author_keys), "--commit-authors", str(listed)],
        )
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("are not the 1 checked here", result.stdout)

    def test_a_push_replaces_a_different_record_the_fork_holds_for_its_head(self):
        head = self.reviewed_push()
        subprocess.run(
            ["git", "--git-dir", str(self.fork), "-c", "user.name=Other"]
            + ["-c", "user.email=other@example.test", "notes"]
            + ["--ref=codetrial-review", "add", "-f", "-m", '{"stale": true}', head],
            check=True,
            capture_output=True,
            env=self.env,
        )
        self.assertEqual(self.push().returncode, 0)
        self.assertEqual(self.fork_note(head)["status"], "complete")

    def test_publish_review_sends_to_the_push_url(self):
        # Fetched from one repository and pushed to another, as a clone of
        # upstream that pushes to a fork is set up; only the fork has the branch.
        head = self.pushed_without_record()
        fetched = self.root / "fetched.git"
        self.git("init", "-q", "--bare", str(fetched))
        self.git("config", "remote.fork.pushurl", str(self.fork))
        self.git("config", "remote.fork.url", str(fetched))
        result = self.publish("topic")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.fork_note(head)["status"], "complete")

    def test_a_push_whose_record_cannot_be_sent_is_refused(self):
        self.large()
        self.prepare()
        self.git("remote", "set-url", "fork", str(self.root / "missing.git"))
        refused = self.push()
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("cannot send the interactive review record", refused.stderr)

    def test_git_ssh_signing_key_is_used_when_none_is_set_for_the_review(self):
        self.git("config", "--unset", "codetrial.signingKey")
        self.git("config", "user.signingKey", str(self.key))
        self.env = {k: v for k, v in self.env.items() if k != "SSH_AUTH_SOCK"}
        self.large()
        self.prepare()
        # Without gpg.format=ssh, user.signingKey names a GPG key: not used.
        self.assertNotEqual(self.push().returncode, 0)
        self.git("config", "gpg.format", "ssh")
        pushed = self.push()
        self.assertEqual(pushed.returncode, 0, pushed.stderr)
        head = self.git("rev-parse", "HEAD").stdout.strip()
        self.assertTrue(self.fork_note(head)["key_fingerprint"].startswith("SHA256:"))
        self.as_ci()
        self.assertEqual(self.pr().returncode, 0)

    def test_the_documents_state_what_the_checker_enforces(self):
        gate = load_gate("gate_constants")
        steps = (
            "ssh-keygen -t ed25519 -f ~/.ssh/codetrial_signing",
            "Key type: Signing Key",
            "git config codetrial.signingKey ~/.ssh/codetrial_signing",
        )
        for step in steps:
            self.assertIn(step, gate.SETUP)
        documents = {
            "CONTRIBUTING.md": (
                f"at least {gate.LARGE_COMMIT} added plus deleted",
                f"more than {gate.PR_TOTAL}",
                "Key type: Signing Key",
                "git config codetrial.codeOwner",
                "docs/large-change-explanation.md#troubleshooting",
                *steps[::2],
            ),
            "docs/large-change-explanation.md": (
                f"at least {gate.LARGE_COMMIT} added plus deleted",
                f"more than {gate.PR_TOTAL}",
                gate.NAMESPACE,
                "CODEOWNERS",
                gate.PUBLISH,
            ),
            ".claude/skills/codetrial-contribute/SKILL.md": (
                "large-change-explanation.md#troubleshooting",
            ),
            "docs/development.md": (gate.NAMESPACE, gate.NOTES),
            ".claude/skills/codetrial-review/workbench.md": (gate.NOTES,),
        }
        for name, phrases in documents.items():
            # Compared with whitespace folded, so rewrapping a paragraph passes.
            text = " ".join((ROOT / name).read_text().split())
            for phrase in phrases:
                with self.subTest(document=name, phrase=phrase):
                    self.assertIn(phrase, text)
        # The removed label is gone from everything a contributor reads.
        for path in [ROOT / "CONTRIBUTING.md", *(ROOT / "docs").glob("*.md")] + [
            *(ROOT / ".claude/skills/codetrial-review").rglob("*.*")
        ]:
            if path.suffix in (".md", ".html", ".js", ".py"):
                with self.subTest(path=path.name):
                    self.assertNotIn("`explained` label", path.read_text())
                    self.assertNotIn("<code>explained</code>", path.read_text())

    def test_a_code_owner_may_skip_the_review_and_others_may_not(self):
        self.owners()
        self.large()
        for login in ("owner", "OWNER2"):
            with self.subTest(login=login):
                result = self.pr(login=login, authors=[login])
                self.assertEqual(result.returncode, 0, result.stdout)
                self.assertIn("every commit's author are code owners", result.stdout)
                self.assertIn("::notice title=Large change::", result.stdout)
        for login in ("contributor", "commented-out", "org", "team", "own"):
            with self.subTest(login=login):
                self.assertEqual(self.pr(login=login, authors=[login]).returncode, 1)

    def test_a_code_owner_opening_someone_elses_work_does_not_excuse_it(self):
        self.owners()
        self.large()
        for authors, said in (
            (["contributor"], "commits here are by contributor"),
            ([""], "an author linked to no GitHub account"),
            (None, "GitHub did not list the commits' authors"),
            (["owner", "owner"], "the 2 commits GitHub listed are not the 1"),
            ([], "the 0 commits GitHub listed are not the 1"),
        ):
            with self.subTest(authors=authors):
                result = self.pr(login="owner", authors=authors)
                self.assertEqual(result.returncode, 1, result.stdout)
                self.assertIn(
                    "::error title=Interactive review missing::", result.stdout
                )
                self.assertIn("owner is a code owner, but", result.stdout)
                self.assertIn(said, result.stdout)
        # Every commit by one code owner or another, in any case, is exempt.
        self.assertEqual(self.pr(login="owner", authors=["OWNER2"]).returncode, 0)
        # The review itself still lets it through.
        self.prepare()
        pushed = self.push()
        self.assertEqual(pushed.returncode, 0, pushed.stderr)
        self.as_ci()
        result = self.pr(login="owner", authors=["contributor"])
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("recorded at push", result.stdout)

    def test_a_code_owner_pushes_without_the_review_after_saying_who_they_are(self):
        self.owners()
        self.large()
        refused = self.push()
        self.assertNotEqual(refused.returncode, 0)
        self.git("config", "codetrial.codeOwner", "someone-else")
        self.assertNotEqual(self.push().returncode, 0)
        self.git("config", "codetrial.codeOwner", "owner")
        pushed = self.push()
        self.assertEqual(pushed.returncode, 0, pushed.stderr)
        self.assertIn("code owner", pushed.stdout + pushed.stderr)
        # Said at the moment it could mislead: the pull request judges accounts.
        self.assertIn("does not read this setting", pushed.stdout + pushed.stderr)
        head = self.git("rev-parse", "HEAD").stdout.strip()
        self.assertIsNone(self.fork_note(head))

    def test_a_code_owner_who_opts_in_still_gets_a_signed_record(self):
        self.owners()
        self.git("config", "codetrial.codeOwner", "owner")
        head = self.reviewed_push()
        self.assertEqual(self.fork_note(head)["status"], "complete")
        self.assertIn("signature", self.fork_note(head))

    def test_a_record_that_does_not_match_the_change_is_refused(self):
        head = self.reviewed_push()
        original = self.review_note()
        for field, value in (
            ("status", "pending"),
            ("diff_hash", "0" * 64),
            ("head", self.base),
            ("base", "f" * 40),
        ):
            with self.subTest(field=field):
                self.git(
                    "notes",
                    "--ref=codetrial-review",
                    "add",
                    "-f",
                    "-m",
                    json.dumps({**original, field: value}),
                    head,
                )
                result = self.pr()
                self.assertEqual(result.returncode, 1, result.stdout)
                self.assertIn(
                    "::error title=Interactive review missing::", result.stdout
                )
                self.assertNotIn("Traceback", result.stderr)
        self.git("notes", "--ref=codetrial-review", "add", "-f", "-m", "not json", head)
        self.assertEqual(self.pr().returncode, 1)

    def test_updating_the_branch_from_main_keeps_the_record(self):
        head = self.reviewed_push()
        # Main moves on, and GitHub's "Update branch" merges it in.
        self.git("checkout", "-q", "--detach", self.base)
        (self.repo / "other.txt").write_text("main moved\n")
        self.git("add", "other.txt")
        self.git("commit", "-qm", "Move main", "-m", "Advance the fixture main.")
        main = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("checkout", "-q", "--detach", head)
        self.git("merge", "-q", "--no-ff", "--no-edit", main)
        merged = self.git("rev-parse", "HEAD").stdout.strip()
        self.as_ci()
        result = self.pr(base=main, head=merged)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        # A merge that also changes something of its own is not just an update.
        (self.repo / "change.txt").write_text("edited in the merge\n")
        self.git("add", "change.txt")
        self.git("commit", "-q", "--amend", "--no-edit")
        amended = self.git("rev-parse", "HEAD").stdout.strip()
        self.assertEqual(self.pr(base=main, head=amended).returncode, 1)
        # Nor is a merge of something that is not on main.
        self.git("checkout", "-q", "--detach", head)
        (self.repo / "side.txt").write_text("side\n")
        self.git("add", "side.txt")
        self.git("commit", "-qm", "Side work", "-m", "Not on main.")
        side = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("checkout", "-q", "--detach", head)
        self.git("merge", "-q", "--no-ff", "--no-edit", side)
        self.assertEqual(self.pr(base=main).returncode, 1)

    def test_the_notice_carries_no_text_from_the_branch(self):
        # The workflow prints the result as a workflow command, so a path that
        # held one would be read as a second command.
        name = "notes-%0A::error::forged.bin"
        (self.repo / name).write_bytes(b"\0binary\0" * 10)
        self.git("add", "--", name)
        self.large(1)
        for result in (self.pr(), self.gate("pr")):
            self.assertIn("cannot be counted as text", result.stdout)
            self.assertNotIn("forged", result.stdout)
            self.assertEqual(len(result.stdout.strip().splitlines()), 1)

    def test_only_a_reviewed_push_produces_the_record(self):
        self.large()
        self.assertEqual(self.pr().returncode, 1)
        refused = self.push()
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("Interactive review required", refused.stderr)
        self.assertIsNone(self.fork_note(self.git("rev-parse", "HEAD").stdout.strip()))
        self.prepare()
        self.assertEqual(self.push().returncode, 0)
        self.as_ci()
        self.assertEqual(self.pr().returncode, 0)

    def test_a_push_writes_no_description_block(self):
        self.large(10)
        pushed = self.push()
        self.assertEqual(pushed.returncode, 0)
        self.assertNotIn("description", pushed.stderr.lower())
        directory = self.repo / ".git/codetrial-review"
        self.assertEqual(list(directory.glob("pr-description*")), [])

    def test_a_base_diff_rule_removed_by_the_branch_cannot_hide_its_size(self):
        (self.repo / ".gitattributes").write_text("change.txt -diff\n")
        self.git("add", ".gitattributes")
        self.git(
            "commit",
            "-qm",
            "Mark the fixture file as not diffable",
            "-m",
            "The base carries a rule that the branch then removes.",
        )
        self.git("update-ref", "refs/remotes/upstream/main", "HEAD")
        self.git("rm", "-q", ".gitattributes")
        (self.repo / "change.txt").write_text("base\n" + "line\n" * 150)
        self.commit("Drop the rule and grow the fixture file")
        # Every commit from here adds 150 lines, under 200, and counts fully
        # under its own attributes; only the sum is large.
        for total in (300, 450, 600):
            self.large(total)
        result = self.push()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Interactive review required", result.stderr)

    def test_malformed_evidence_is_a_failed_review_not_a_crash(self):
        self.large()
        head = self.git("rev-parse", "HEAD").stdout.strip()
        evidence = self.repo / ".git/codetrial-review/interactive"
        evidence.mkdir(parents=True, exist_ok=True)
        (evidence / (head + ".json")).write_text("[]\n")
        for command in ("status", "push"):
            result = self.gate(command, "--head", head)
            self.assertNotIn("Traceback", result.stderr, command)
        pushed = self.push()
        self.assertNotEqual(pushed.returncode, 0)
        self.assertNotIn("Traceback", pushed.stderr)

    def test_push_without_a_base_is_refused_not_skipped(self):
        self.large(10)
        self.git("update-ref", "-d", "refs/remotes/upstream/main")
        refused = self.push()
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("no remote-tracking main", refused.stderr)
        self.assertIn("git fetch", refused.stderr)
        self.git("update-ref", "refs/remotes/upstream/main", self.base)
        self.assertEqual(self.push().returncode, 0)

    def test_199_is_advisory_and_200_requires_interactive_evidence(self):
        self.large(199)
        self.assertEqual(self.push().returncode, 0)
        self.large(200)
        # Only one line was added to the last commit, but the aggregate is 200
        # and every commit is below 200, so this branch remains advisory.
        self.assertEqual(self.push().returncode, 0)
        (self.repo / "change.txt").write_text("base\nlast\n")
        self.commit("Replace the earlier fixture")
        result = self.push()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("commits >=200", result.stderr)

    def test_pr_501_triggers_with_all_commits_below_200(self):
        for count in [167, 334, 500]:
            self.large(count)
        self.assertEqual(self.push().returncode, 0)
        self.large(501)
        status = self.review_status()
        self.assertEqual(status["status"], "pending")
        self.assertEqual(status["pr_text_lines"], 501)
        self.assertEqual(status["large_commits"], [])
        result = self.push()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("0 commits >=200 lines; PR diff 501", result.stderr)

    def test_exactly_500_with_small_commits_needs_no_record(self):
        for count in [167, 334, 500]:
            self.large(count)
        result = self.pr()
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("under the interactive review thresholds", result.stdout)

    def test_plain_reviewed_marker_cannot_bypass_the_large_gate(self):
        self.large()
        marker = self.repo / ".git/codetrial-review/reviewed"
        marker.parent.mkdir()
        marker.write_text(self.git("rev-parse", "HEAD").stdout)
        self.assertNotEqual(self.push().returncode, 0)

    def test_binary_attributes_cannot_hide_text_from_the_review_requirement(self):
        (self.repo / ".gitattributes").write_text("change.txt -diff\n")
        self.git("add", ".gitattributes")
        self.large()
        tip = self.git("rev-parse", "HEAD").stdout.strip()
        result = self.review_status()
        self.assertTrue(result["required"])
        self.assertEqual(result["status"], "pending")
        self.assertFalse(result["coverage_complete"])
        self.assertIn(
            {"sha": tip, "paths": ["change.txt"]}, result["unmeasured_commits"]
        )
        self.assertNotEqual(self.push().returncode, 0)

    def test_removing_the_skill_cannot_disable_an_established_review_policy(self):
        skill_path = ".claude/skills/codetrial-review/SKILL.md"
        self.git("add", "-f", skill_path)
        self.git(
            "commit",
            "-qm",
            "Establish the fixture review policy",
            "-m",
            "Keep the upstream review requirement independent of the outgoing tree.",
        )
        self.base = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("update-ref", "refs/remotes/upstream/main", self.base)
        self.large()
        (self.repo / skill_path).unlink()
        self.assertNotEqual(self.push().returncode, 0)
        self.git("add", "-u", skill_path)
        self.git(
            "commit",
            "-qm",
            "Remove the fixture skill",
            "-m",
            "Verify that deleting local instructions cannot waive review.",
        )
        result = self.push()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Interactive review required", result.stderr)

    def test_replacement_objects_cannot_hide_large_commits(self):
        self.large()
        tip = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("replace", tip, self.base)
        result = self.review_status()
        self.assertEqual(result["status"], "pending")
        self.assertIn({"sha": tip, "text_lines": 200}, result["large_commits"])
        self.assertNotEqual(self.push().returncode, 0)

    def test_grafts_and_shallow_history_do_not_report_small_changes(self):
        self.large()
        tip = self.git("rev-parse", "HEAD").stdout.strip()
        grafts = self.repo / ".git/info/grafts"
        grafts.write_text(tip + "\n")
        for path in (grafts, self.repo / ".git/shallow"):
            path.write_text(tip + "\n")
            result = self.run_command(
                [sys.executable, str(GATE), "status", "--base", self.base],
                check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, "")
            path.unlink()

    def review_status(self):
        result = self.gate("status")
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def review_note(self, head="HEAD"):
        return json.loads(
            self.git("notes", "--ref=codetrial-review", "show", head).stdout
        )

    def test_required_marker_is_visible_idempotent_and_cannot_authorize_a_push(self):
        self.large()
        head = self.git("rev-parse", "HEAD").stdout.strip()
        tree = self.git("write-tree").stdout
        self.assertNotEqual(self.push().returncode, 0)
        marker = self.review_note()
        self.assertEqual(marker["head"], head)
        self.assertEqual(marker["base"], self.base)
        self.assertEqual(marker["status"], "pending")
        self.assertFalse(marker["workbench_started"])
        ref = self.git("rev-parse", "refs/notes/codetrial-review").stdout
        self.assertNotEqual(self.push().returncode, 0)
        self.assertEqual(
            self.git("rev-parse", "refs/notes/codetrial-review").stdout, ref
        )
        marker["status"] = "complete"
        self.git(
            "notes",
            "--ref=codetrial-review",
            "add",
            "-f",
            "-F",
            "-",
            head,
            data=json.dumps(marker),
        )
        self.assertNotEqual(self.push().returncode, 0)
        self.assertEqual(self.review_note()["status"], "pending")
        self.assertEqual(self.git("rev-parse", "HEAD").stdout.strip(), head)
        self.assertEqual(self.git("write-tree").stdout, tree)
        self.assertIn(
            '"status": "pending"',
            self.git("log", "-1", "--notes=codetrial-review").stdout,
        )

    def test_server_start_marks_activity_without_claiming_completion(self):
        self.large()
        bundle, head = self.prepare(approved=False)
        process = subprocess.Popen(
            [
                sys.executable,
                str(SKILL / "workbench.py"),
                "serve",
                "--port",
                "0",
                "--bundle",
                str(bundle),
                "--questions",
                str(bundle / "questions.json"),
            ],
            cwd=self.repo,
            env=self.env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            ready, _, _ = select.select([process.stdout], [], [], 10)
            self.assertTrue(ready, "Workbench did not start")
            self.assertTrue(process.stdout.readline().startswith("http://127.0.0.1:"))
            marker = self.review_note(head)
            self.assertTrue(marker["workbench_started"])
            self.assertEqual(marker["status"], "pending")
            self.assertNotEqual(self.push().returncode, 0)
            self.assertTrue(self.review_note()["workbench_started"])
        finally:
            process.terminate()
            process.communicate(timeout=5)

    def test_status_names_triggering_commits_even_with_small_final_diff(self):
        self.assertEqual(self.review_status()["status"], "not_required")
        self.large()
        large = self.git("rev-parse", "HEAD").stdout.strip()
        (self.repo / "change.txt").write_text("base\n")
        self.commit("Restore the initial fixture")
        result = self.review_status()
        self.assertEqual(result["pr_text_lines"], 0)
        self.assertTrue(result["required"])
        self.assertEqual(result["status"], "pending")
        self.assertIn({"sha": large, "text_lines": 200}, result["large_commits"])
        self.assertIn(large, self.push().stderr)

    def test_durable_lifecycle_and_explanation_allow_the_exact_head(self):
        self.large()
        bundle, head = self.prepare()
        proof = self.repo / ".git/codetrial-review/interactive" / (head + ".json")
        self.assertTrue(proof.exists())
        self.assertEqual(self.push().returncode, 0)
        self.assertEqual(self.review_status()["status"], "complete")
        marker = self.review_note()
        self.assertEqual(marker["status"], "complete")
        self.assertNotIn("answers", marker)
        self.assertNotIn("feedback", marker)
        self.assertNotIn("evidence", marker)
        code = """
import importlib.util,sys
from pathlib import Path
spec=importlib.util.spec_from_file_location('fixture_workbench',sys.argv[1]); m=importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
p=Path(sys.argv[2]); app=m.Workbench(p,p/'questions.json'); s=app.snapshot(); s['answers']['motivation']='A changed fixture answer invalidates approval.'
app.save({'identity':s['identity'],'revision':s['revision'],'answers':s['answers']})
"""
        self.run_command(
            [sys.executable, "-c", code, str(SKILL / "workbench.py"), str(bundle)]
        )
        self.assertFalse(proof.exists())
        self.assertEqual(self.review_note()["status"], "pending")
        self.assertEqual(self.review_status()["status"], "pending")
        self.assertNotEqual(self.push().returncode, 0)

    def test_pending_explanation_and_amended_heads_do_not_pass(self):
        self.large()
        bundle, head = self.prepare(approved=False)
        self.assertNotEqual(self.push().returncode, 0)
        result = self.run_command(
            [sys.executable, str(GATE), "certify", "--bundle", str(bundle)], check=False
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(
            (
                self.repo / ".git/codetrial-review/interactive" / (head + ".json")
            ).exists()
        )

    def test_amend_and_missing_database_cannot_reuse_a_certificate(self):
        self.large()
        self.prepare()
        self.assertEqual(self.push().returncode, 0)
        old_head = self.git("rev-parse", "HEAD").stdout.strip()
        database = self.root / "memory.db"
        backup = database.with_suffix(".saved")
        database.rename(backup)
        self.assertNotEqual(self.push().returncode, 0)
        backup.rename(database)
        self.assertEqual(self.push().returncode, 0)
        self.git(
            "commit",
            "--amend",
            "-qm",
            "Clarify the fixture premise",
            "-m",
            "A new commit identity needs its own matching review receipt.",
        )
        self.git("notes", "--ref=codetrial-review", "copy", old_head, "HEAD")
        self.assertNotEqual(self.push().returncode, 0)
        self.assertEqual(self.review_note()["status"], "pending")
        self.assertEqual(
            self.review_note()["head"], self.git("rev-parse", "HEAD").stdout.strip()
        )

    def test_marker_write_failure_is_reported_and_retry_preserves_the_commit(self):
        self.large()
        self.prepare()
        head = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("notes", "--ref=codetrial-review", "remove", head)
        lock = self.repo / ".git/refs/notes/codetrial-review.lock"
        lock.write_text("fixture lock\n")
        result = self.push()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Cannot record interactive-review Git note", result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD").stdout.strip(), head)
        lock.unlink()
        self.assertEqual(self.push().returncode, 0)
        self.assertEqual(self.review_note()["status"], "complete")

    def test_real_commit_hooks_and_large_review_gate_work_together(self):
        for path in (ROOT / "scripts").glob("git-*.sh"):
            shutil.copy2(path, self.repo / "scripts" / path.name)
        for name in ["install-git-hooks.sh", "indent.sh"]:
            shutil.copy2(ROOT / "scripts" / name, self.repo / "scripts" / name)
        for name in [".editorconfig", ".shellcheckrc"]:
            shutil.copy2(ROOT / name, self.repo / name)
        with (self.repo / ".git/info/exclude").open("a") as output:
            output.write("/.editorconfig\n/.shellcheckrc\n")
        (self.repo / "scripts/hashchain-base").write_text(self.base + "\n")
        self.git("add", "-f", "scripts/hashchain-base")
        self.run_command(["sh", "scripts/install-git-hooks.sh"])
        self.git(
            "commit",
            "-qm",
            "Enable the fixture chain",
            "-m",
            "Bind the next changes to this local fixture anchor.",
        )
        self.large()
        head = self.git("rev-parse", "HEAD").stdout.strip()
        change_id = next(
            line.split(": ", 1)[1]
            for line in self.git(
                "show", "--no-notes", "-s", "--format=%B", head
            ).stdout.splitlines()
            if line.startswith("Change-Id:")
        )
        self.assertEqual(self.review_note()["change_id"], change_id)
        self.assertEqual(self.review_note()["status"], "pending")
        self.assertNotEqual(self.push().returncode, 0)
        self.prepare()
        self.assertEqual(self.push().returncode, 0)
        self.assertEqual(self.review_note()["change_id"], change_id)
        self.assertEqual(self.git("rev-parse", "HEAD").stdout.strip(), head)
        # Through the installed hook: the record goes with the branch, and the
        # hook's own push of the notes ref does not loop back into the checks.
        self.git("push", "-q", "fork", "HEAD:refs/heads/topic")
        self.assertEqual(self.fork_note(head)["status"], "complete")


class QueueReviewTests(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location(
            "queue_review", ROOT / "scripts/check-queue-review.py"
        )
        self.queue = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.queue)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.env = {
            **os.environ,
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
        }
        self.git("init", "-q")
        self.git("config", "user.name", "Queue Test")
        self.git("config", "user.email", "queue@example.test")
        self.git("config", "commit.gpgsign", "false")
        self.base = self.commit("base")
        # Bind the subprocess cwd without changing the test runner's process.
        self.queue.git = self.git

    def git(self, *args):
        return subprocess.check_output(
            ["git", *args],
            cwd=self.repo,
            env=self.env,
            text=True,
            stderr=subprocess.PIPE,
        ).strip()

    def commit(self, value):
        (self.repo / "file").write_text(value)
        self.git("add", "file")
        self.git("commit", "-qm", value)
        return self.git("rev-parse", "HEAD")

    def commit_by(self, author, parent, text, *more):
        tree = self.git("rev-parse", f"{parent}^{{tree}}")
        parents = [arg for p in (parent, *more) for arg in ("-p", p)]
        env = {**self.env, "GIT_AUTHOR_NAME": "x", "GIT_AUTHOR_EMAIL": author}
        return subprocess.run(
            ["git", "commit-tree", tree, *parents, "-m", text],
            cwd=self.repo,
            env=env,
            text=True,
            capture_output=True,
            check=True,
        ).stdout.strip()

    def test_dependabot_exemption_needs_the_opener_and_every_commit(self):
        gate = self.checker()
        bot = "49699333+dependabot[bot]@users.noreply.github.com"
        mine = self.commit_by(bot, self.base, "Bump a")
        mixed = self.commit_by("person@example.test", mine, "Add a change")
        self.git("update-ref", "refs/heads/probe", mixed)
        # A merge a person resolved by hand is not Dependabot's work either.
        merged = self.commit_by("person@example.test", mine, "Merge main", self.base)
        # An address that only ends like Dependabot's is not Dependabot's.
        spoofed = self.commit_by("x" + bot, self.base, "Bump a")
        for tip, login, kind, exempt in (
            (mine, "dependabot[bot]", "Bot", True),
            (mine, "dependabot[bot]", "User", False),
            (mine, "dependabot[bot]", None, False),
            (mine, "contributor", "User", False),
            (mixed, "dependabot[bot]", "Bot", False),
            (merged, "dependabot[bot]", "Bot", False),
            (spoofed, "dependabot[bot]", "Bot", False),
            (self.base, "dependabot[bot]", "Bot", False),
        ):
            with self.subTest(tip=tip[:7], login=login, kind=kind):
                self.assertEqual(
                    gate.dependabot_exempt(self.base, tip, login, kind), exempt
                )

    def checker(self):
        gate = load_gate()
        gate.git = lambda *args, data=None: self.git(*args).encode()
        gate.module(ROOT / "scripts/hashchain.py").git = lambda *a, **k: (
            subprocess.run(
                ["git", *a],
                cwd=self.repo,
                env=self.env,
                input=k.get("data"),
                stdout=subprocess.PIPE,
                check=True,
            ).stdout
        )
        return gate

    def tree_with(self, parent, files, gate=True):
        """A commit on `parent` whose workflow directory is exactly `files`."""
        self.git("checkout", "-q", "--detach", parent)
        shutil.rmtree(self.repo / ".github", ignore_errors=True)
        folder = self.repo / ".github/workflows"
        folder.mkdir(parents=True)
        for name, text in files.items():
            (folder / name).write_text(text)
        for path in self.queue.GATE:
            target = self.repo / path
            if gate:
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text("# gate\n")
            else:
                target.unlink(missing_ok=True)
        self.git("add", "-A")
        self.git("commit", "-q", "--allow-empty", "-m", "workflows")
        return self.git("rev-parse", "HEAD")

    def test_guard_holds_the_queue_workflow_to_where_the_branch_began(self):
        guarded = "name: pr-description\njobs:\n  interactive-review:\n    steps: []\n"
        edited = guarded + "# edited\n"
        claim = "jobs:\n  interactive-review:\n    steps: []\n"
        named = "jobs:\n  x:\n    name: interactive-review # alternate\n"
        quoted = "jobs:\n  'interactive-review': {runs-on: x}\n"
        branch_point = self.tree_with(self.base, {"pr-description.yml": guarded})
        # The base moved on with an approved edit; the older branch is untouched.
        moved = self.tree_with(branch_point, {"pr-description.yml": edited})
        cases = {
            "unchanged": ({"pr-description.yml": guarded}, True),
            "disabled": (
                {"pr-description.yml": guarded.replace("steps: []", "if: false")},
                False,
            ),
            "deleted": ({}, False),
            "second job with the name": (
                {"pr-description.yml": guarded, "other.yml": claim},
                False,
            ),
            "second check with the name": (
                {"pr-description.yml": guarded, "other.yml": named},
                False,
            ),
            "quoted key": (
                {"pr-description.yml": guarded, "other.yaml": quoted},
                False,
            ),
            "unrelated workflow": (
                {"pr-description.yml": guarded, "other.yml": "jobs:\n  t: {}\n"},
                True,
            ),
        }
        for name, (files, passes) in cases.items():
            with self.subTest(name):
                head = self.tree_with(branch_point, files)
                self.assertEqual(self.queue.guard(moved, head, "") == [], passes)
        with self.subTest("approved head"):
            head = self.tree_with(
                branch_point, {"pr-description.yml": guarded + "# the pull request\n"}
            )
            self.assertTrue(self.queue.guard(moved, head, ""))
            self.assertEqual(self.queue.guard(moved, head, head), [])
        with self.subTest("an approval for some other head"):
            head = self.tree_with(
                branch_point, {"pr-description.yml": guarded + "# the pull request\n"}
            )
            self.assertTrue(self.queue.guard(moved, head, branch_point))
        with self.subTest("the name in other letters"):
            head = self.tree_with(
                branch_point,
                {
                    "pr-description.yml": guarded,
                    "other.yml": "name: Interactive-Review\n",
                },
            )
            self.assertTrue(self.queue.guard(moved, head, ""))
        with self.subTest("a gate script removed"):
            head = self.tree_with(
                branch_point, {"pr-description.yml": guarded}, gate=False
            )
            found = self.queue.guard(moved, head, "")
            self.assertEqual(len(found), len(self.queue.GATE), found)
            self.assertIn("was removed", found[0])
        with self.subTest("base moved, branch untouched"):
            head = self.tree_with(branch_point, {"pr-description.yml": guarded})
            self.assertEqual(self.queue.guard(moved, head, ""), [])

    def test_cli_guard_passes_an_unchanged_workflow_and_refuses_a_changed_one(self):
        text = "jobs:\n  interactive-review:\n    steps: []\n"
        start = self.tree_with(self.base, {"pr-description.yml": text})
        same = self.tree_with(start, {"pr-description.yml": text})
        off = self.tree_with(start, {"pr-description.yml": text + "# off\n"})
        script = str(ROOT / "scripts/check-queue-review.py")

        def run(head, *extra):
            return subprocess.run(
                [
                    sys.executable,
                    script,
                    "--base",
                    start,
                    "--head",
                    head,
                    *extra,
                ],
                cwd=self.repo,
                env=self.env,
                text=True,
                capture_output=True,
            )

        self.assertEqual(run(same).returncode, 0)
        refused = run(off)
        self.assertEqual(refused.returncode, 1)
        self.assertIn("was changed", refused.stderr)
        self.assertEqual(run(off, "--approved", off).returncode, 0)
        self.assertEqual(run(off, "--approved", same).returncode, 1)
        # A head with no common history cannot be compared, which is not a pass.
        self.git("checkout", "-q", "--orphan", "unrelated")
        self.git("commit", "-q", "--allow-empty", "-m", "unrelated")
        self.assertEqual(run(self.git("rev-parse", "HEAD")).returncode, 1)

    def test_pr_excuses_only_a_dependabot_pull_request(self):
        bot = "49699333+dependabot[bot]@users.noreply.github.com"
        theirs = self.commit_by(bot, self.base, "Bump a")
        mine = self.commit_by("person@example.test", self.base, "Change a")

        def run(head, login, kind):
            return subprocess.run(
                [sys.executable, str(GATE), "pr", "--base", self.base]
                + ["--head", head]
                + ["--author-login", login, "--author-type", kind],
                cwd=self.repo,
                env=self.env,
                text=True,
                capture_output=True,
            )

        excused = run(theirs, "dependabot[bot]", "Bot")
        self.assertEqual(excused.returncode, 0)
        self.assertIn("Dependabot's own pull request", excused.stdout)
        for head, login, kind in (
            (mine, "dependabot[bot]", "Bot"),
            (theirs, "dependabot[bot]", "User"),
            (theirs, "contributor", "User"),
        ):
            with self.subTest(login=login, kind=kind, human=head == mine):
                # A small change gets no notice either way, so only the
                # message tells whether the exemption applied.
                result = run(head, login, kind)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertNotIn("Dependabot's own", result.stdout)

    def test_the_workflow_passes_only_flags_and_variables_that_exist(self):
        text = (ROOT / ".github/workflows/pr-description.yml").read_text()
        step = support.step(text, "Measure the change")
        env, run = step.split("        run: |", 1)
        declared = set(re.findall(r"^          (\w+):", env, re.M))
        for name in set(re.findall(r"\$\{?([A-Z_]+)", run)):
            if name not in ("RUNNER_TEMP",):
                self.assertIn(name, declared, name)
        for script, source in (
            ("check-queue-review.py", ROOT / "scripts/check-queue-review.py"),
            ("check-interactive-review.py", GATE),
        ):
            calls = re.findall(script + r"((?: .*?\\\n)*[^\n]*)", run)
            self.assertTrue(calls, script)
            for flag in set(re.findall(r" (--[a-z-]+)", "".join(calls))):
                self.assertIn(f'"{flag}"', source.read_text(), (script, flag))
        # The guard runs on pull requests, with the head a maintainer approved.
        self.assertRegex(run, r"check-queue-review\.py[^\n]*\\\n[^\n]*--approved")
        # actions/checkout refuses a fork's head under pull_request_target, and
        # the head is data here anyway: only the base is checked out, and the
        # pull request's commits are fetched by identity into a bare repository.
        self.assertEqual(text.count("uses: actions/checkout"), 1)
        self.assertNotIn("head.sha", text[: text.index("- name: Measure the change")])
        self.assertIn("git init -q --bare head.git", run)
        self.assertIn('"$BASE_SHA" "$HEAD_SHA"', run)
        # The scripts and the base measured against are the workflow's own
        # commit: a reopened pull request's base.sha may predate this file.
        self.assertNotIn("pull_request.base.sha", text)
        self.assertRegex(
            text,
            r"\n +ref: \$\{\{ github\.event_name == 'merge_group'"
            r" && github\.event\.merge_group\.base_sha \|\| github\.sha \}\}\n",
        )
        self.assertRegex(
            text,
            r"\n +BASE_SHA: \$\{\{ github\.event_name =="
            r" 'pull_request_target' && github\.sha \|\| '' \}\}\n",
        )
        self.assertNotIn("labels", text.lower())
        # The review record comes from the repository the branch came from, by
        # its validated name, and only after the change itself was fetched.
        self.assertIn(
            "HEAD_REPOSITORY: ${{ github.event.pull_request.head.repo.full_name }}",
            text,
        )
        self.assertIn('"+refs/notes/codetrial-review:refs/notes/codetrial-review"', run)
        self.assertLess(run.index('"$BASE_SHA" "$HEAD_SHA"'), run.index("refs/notes/"))
        # The signature is checked against the keys GitHub lists for the
        # author, fetched by a validated login without a token.
        self.assertIn('"https://github.com/$login.keys"', run)
        self.assertIn("for login in $opener $others; do", run)
        # Signing keys too, so a dedicated key leaves an HTTPS push untouched.
        self.assertIn('"users/$login/ssh_signing_keys"', run)
        self.assertIn('--author-keys "$keys"', run)
        # A code owner's exemption needs every commit's author from GitHub.
        self.assertIn('"repos/$REPOSITORY/pulls/$PR_NUMBER/commits"', run)
        self.assertIn("listed=$authors", run)
        self.assertIn('${listed:+--commit-authors "$listed"}', run)
        # The workflow and the checker accept the same logins.
        valid = re.search(r"valid='([^']+)'", run).group(1)
        self.assertEqual(valid, load_gate().LOGIN.pattern)
        self.assertIn('"--commit-authors"', GATE.read_text())
        # A queue head has no record of its own; each pull request had to pass.
        self.assertLess(run.index('"$EVENT" = merge_group'), run.index("git init"))
