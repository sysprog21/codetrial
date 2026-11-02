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
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
GATE = ROOT / "scripts/check-interactive-review.py"
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
        (self.repo / "change.txt").write_text("base\n")
        self.commit("Create the review fixture")
        self.base = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("remote", "add", "upstream", "https://github.com/sysprog21/codetrial")
        self.git("update-ref", "refs/remotes/upstream/main", self.base)
        (self.repo / "scripts").mkdir()
        for name in [
            "git-pre-push.sh",
            "git-commit-msg.sh",
            "git-commit-msg.awk",
            "check-commit-log.sh",
            "check-secrets.py",
            "check-interactive-review.py",
            "hashchain.py",
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
            ["sh", "scripts/git-pre-push.sh", "fork", "local-fixture"],
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

    def gate(self, command, *args):
        return self.run_command(
            [
                sys.executable,
                "scripts/check-interactive-review.py",
                command,
                "--base",
                self.base,
                *args,
            ],
            check=False,
        )

    def labelled(self, *names):
        return self.gate("pr", "--labels", json.dumps(list(names)))

    def test_a_small_change_needs_no_label(self):
        self.large(10)
        result = self.labelled()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("under the interactive review thresholds", result.stdout)

    def test_a_large_change_passes_only_with_the_maintainers_label(self):
        self.large()
        for names in ((), ("bug",), ("not-explained",)):
            refused = self.labelled(*names)
            self.assertNotEqual(refused.returncode, 0, names)
            self.assertIn("[interactive-review/required]", refused.stderr)
        self.assertEqual(self.labelled("bug", "explained").returncode, 0)
        malformed = self.gate("pr", "--labels", '{"explained": true}')
        self.assertNotEqual(malformed.returncode, 0)
        self.assertNotIn("Traceback", malformed.stderr)

    def test_the_label_does_not_replace_the_local_review_at_push(self):
        self.large()
        self.assertEqual(self.labelled("explained").returncode, 0)
        refused = self.push()
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("Interactive review required", refused.stderr)
        self.prepare()
        self.assertEqual(self.push().returncode, 0)

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
        return json.loads(
            self.run_command(
                [sys.executable, str(GATE), "status", "--base", self.base]
            ).stdout
        )

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
        remote = self.root / "remote.git"
        self.git("init", "--bare", "-q", str(remote))
        self.git("remote", "add", "fork", str(remote))
        self.git("push", "-q", "fork", "HEAD:refs/heads/topic")


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
        self.first = self.commit("first")
        self.second = self.commit("second")
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

    def pull(self, number, tip, labels=("explained",)):
        return {
            "number": number,
            "head": {"sha": tip},
            "user": {"login": "contributor", "type": "User"},
            "labels": [{"name": name} for name in labels],
        }

    def checker(self):
        calls = []

        def check_labels(base, tip, labels):
            calls.append((base, tip, labels))
            if "explained" not in labels:
                raise ValueError("unlabeled")

        return (
            SimpleNamespace(
                check_labels=check_labels,
                dependabot_exempt=lambda *a: a[2] == "dependabot[bot]",
            ),
            calls,
        )

    def test_every_included_head_is_checked_separately(self):
        checker, calls = self.checker()
        result = self.queue.check(
            self.base,
            self.second,
            [
                self.pull(1, self.first),
                self.pull(2, self.second),
                self.pull(3, self.base),
            ],
            checker,
        )
        self.assertEqual(result, [1, 2])
        self.assertEqual(
            calls,
            [
                (self.base, self.first, ["explained"]),
                (self.base, self.second, ["explained"]),
            ],
        )

    def test_a_missing_label_in_any_pr_fails_the_group(self):
        checker, _ = self.checker()
        with self.assertRaisesRegex(ValueError, "unlabeled"):
            self.queue.check(
                self.base,
                self.second,
                [
                    self.pull(1, self.first, ()),
                    self.pull(2, self.second),
                ],
                checker,
            )

    def test_missing_or_moved_pr_head_fails_closed(self):
        checker, _ = self.checker()
        for pulls in ([], [self.pull(1, self.first)], [self.pull(1, "0" * 40)]):
            with self.subTest(pulls=pulls):
                with self.assertRaisesRegex(ValueError, "matching"):
                    self.queue.check(self.base, self.second, pulls, checker)

    def test_a_group_that_does_not_descend_from_its_base_is_refused(self):
        checker, _ = self.checker()
        self.git("checkout", "-q", "--orphan", "unrelated")
        self.git("commit", "-q", "--allow-empty", "-m", "unrelated")
        stray = self.git("rev-parse", "HEAD")
        with self.assertRaises(subprocess.CalledProcessError):
            self.queue.check(self.base, stray, [self.pull(1, stray)], checker)

    def test_merge_commit_is_not_treated_as_a_pr_commit(self):
        checker, calls = self.checker()
        self.git("checkout", "-qb", "queue", self.base)
        self.git("merge", "--no-ff", "--no-edit", self.second)
        merged = self.git("rev-parse", "HEAD")
        self.assertEqual(
            self.queue.check(
                self.base,
                merged,
                [
                    self.pull(2, self.second),
                ],
                checker,
            ),
            [2],
        )
        self.assertEqual(len(calls), 1)

    def run_queue(self, labels=("explained",), head=None, api_failure=False):
        head = head or self.second
        binary = self.repo / "bin"
        binary.mkdir(exist_ok=True)
        stub = binary / "gh"
        stub.write_text(
            '#!/bin/sh\nif [ "$QUEUE_API_FAILURE" = 1 ]; then exit 1; fi\n'
            'printf "%s" "$QUEUE_API_PAGES"\n'
        )
        stub.chmod(0o755)
        return subprocess.run(
            [
                sys.executable,
                str(ROOT / "scripts/check-queue-review.py"),
                "--base",
                self.base,
                "--head",
                head,
                "--branch",
                "main",
                "--repo",
                "owner/repo",
            ],
            cwd=self.repo,
            text=True,
            capture_output=True,
            env={
                **self.env,
                "PATH": str(binary) + os.pathsep + self.env["PATH"],
                "QUEUE_API_FAILURE": "1" if api_failure else "0",
                "QUEUE_API_PAGES": json.dumps([[], [self.pull(2, head, labels)]]),
            },
        )

    def big(self):
        (self.repo / "file").write_text("line\n" * 600)
        self.git("add", "file")
        self.git("commit", "-qm", "A change over the threshold")
        return self.git("rev-parse", "HEAD")

    def test_cli_checks_the_real_labels_from_later_api_page(self):
        result = self.run_queue()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("[2]", result.stdout)

    def test_cli_rejects_a_large_change_without_the_label(self):
        head = self.big()
        refused = self.run_queue(labels=(), head=head)
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("interactive-review/required", refused.stderr)
        accepted = self.run_queue(labels=("other", "explained"), head=head)
        self.assertEqual(accepted.returncode, 0, accepted.stderr)

    def test_cli_does_not_pass_when_github_cannot_be_read(self):
        result = self.run_queue(api_failure=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("Verified", result.stdout)

    def test_group_with_independent_pr_branches_checks_both(self):
        checker, calls = self.checker()
        self.git("checkout", "-qb", "other", self.base)
        (self.repo / "other").write_text("independent change")
        self.git("add", "other")
        self.git("commit", "-qm", "other branch")
        other = self.git("rev-parse", "HEAD")
        self.git("checkout", "-qb", "queue", self.base)
        self.git("merge", "--no-ff", "--no-edit", self.first)
        self.git("merge", "--no-ff", "--no-edit", other)
        merged = self.git("rev-parse", "HEAD")
        self.assertEqual(
            self.queue.check(
                self.base,
                merged,
                [
                    self.pull(1, self.first),
                    self.pull(2, other),
                ],
                checker,
            ),
            [1, 2],
        )
        self.assertEqual([call[1] for call in calls], [self.first, other])

    def test_dependabot_pull_request_skips_only_its_own_label_check(self):
        checker, calls = self.checker()
        bot = self.pull(1, self.first, ())
        bot["user"] = {"login": "dependabot[bot]", "type": "Bot"}
        result = self.queue.check(
            self.base, self.second, [bot, self.pull(2, self.second)], checker
        )
        self.assertEqual(result, [1, 2])
        self.assertEqual([call[1] for call in calls], [self.second])

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
        gate = self.queue_checker()
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

    def queue_checker(self):
        spec = importlib.util.spec_from_file_location("gate_under_test", GATE)
        gate = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(gate)
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
                    "--guard",
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
                + ["--head", head, "--labels", "[]"]
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
                # A small change needs no label, so only the message tells
                # whether the exemption applied.
                result = run(head, login, kind)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertNotIn("Dependabot's own", result.stdout)

    def test_the_workflow_passes_only_flags_and_variables_that_exist(self):
        text = (ROOT / ".github/workflows/pr-description.yml").read_text()
        step = text[text.index("- name: Measure the change") :]
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
        # The guard runs on pull requests, and the branch name loses refs/heads/.
        self.assertRegex(run, r"check-queue-review\.py[^\n]*\\\n[^\n]*--guard")
        self.assertIn('--branch "${BASE_REF#refs/heads/}"', run)
