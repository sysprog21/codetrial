"""Large-push rejection and durable interactive evidence using real Git objects."""

import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import sys
import tempfile
import unittest

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

    def description(self, text):
        path = self.root / "description.md"
        path.write_text(text)
        return self.gate("pr-body", "--body-file", str(path))

    def test_description_of_a_small_change_carries_a_matching_block(self):
        self.large(10)
        block = self.gate("note").stdout
        self.assertIn("Interactive review: not required", block)
        self.assertEqual(self.description("Intro\n\n" + block).returncode, 0)
        missing = self.description("Only prose, no review block.\n")
        self.assertNotEqual(missing.returncode, 0)
        self.assertIn("[interactive-review/missing-section]", missing.stderr)
        placeholder = self.description(
            "<!-- interactive-review -->\n"
            "Interactive review: replace this block with the hook's output\n"
            "<!-- /interactive-review -->\n"
        )
        self.assertNotEqual(placeholder.returncode, 0)
        self.assertIn("[interactive-review/undeclared]", placeholder.stderr)

    def test_description_of_a_large_change_must_declare_its_own_reviewed_diff(self):
        self.large()
        pending = self.gate("note").stdout
        self.assertIn("Interactive review: pending", pending)
        self.assertIn(
            "[interactive-review/undeclared]", self.description(pending).stderr
        )
        claimed = pending.replace("pending", "not required")
        refused = self.description(claimed)
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("[interactive-review/required]", refused.stderr)
        self.prepare()
        block = self.gate("note").stdout
        self.assertIn("Interactive review: complete", block)
        self.assertEqual(self.description(block).returncode, 0)
        self.assertEqual(self.push().returncode, 0)
        written = self.repo / ".git/codetrial-review/pr-description.md"
        self.assertEqual(written.read_text(), block)
        (self.repo / "change.txt").write_text("base\n" + "line\n" * 201)
        self.commit("Change the fixture after its review")
        stale = self.description(block)
        self.assertNotEqual(stale.returncode, 0)
        self.assertIn("[interactive-review/stale]", stale.stderr)

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

    def test_pushing_a_tag_does_not_discard_the_description_block(self):
        self.large(10)
        self.assertEqual(self.push().returncode, 0)
        written = self.repo / ".git/codetrial-review/pr-description.md"
        block = written.read_text()
        self.assertIn("Interactive review: not required", block)
        head = self.git("rev-parse", "HEAD").stdout.strip()
        tag = self.run_command(
            ["sh", "scripts/git-pre-push.sh", "fork", "local-fixture"],
            data=f"refs/tags/v1 {head} refs/tags/v1 {'0' * 40}\n",
        )
        self.assertEqual(tag.returncode, 0)
        self.assertEqual(written.read_text(), block)
        self.assertNotIn("Pull request description", tag.stderr)

    def test_a_block_copied_from_another_diff_or_doubled_is_refused(self):
        self.large(10)
        block = self.gate("note").stdout
        other = block.replace("Diff: sha256:", "Diff: sha256:00")
        stale = self.description(other)
        self.assertNotEqual(stale.returncode, 0)
        self.assertIn("[interactive-review/stale]", stale.stderr)
        twice = self.description(block + "\n" + block)
        self.assertNotEqual(twice.returncode, 0)
        self.assertIn("more than one", twice.stderr)

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
