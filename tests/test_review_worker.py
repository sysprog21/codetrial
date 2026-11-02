"""Persistent reviewer delivery, cancellation and explicit retries over real processes."""

import importlib.util
import json
import os
import select
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch
from urllib.parse import urlsplit
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
SKILL = ROOT / ".claude/skills/codetrial-review"


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


workbench = module("worker_workbench", SKILL / "workbench.py")
worker = module("worker_runtime", SKILL / "review-worker.py")


class ReviewerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bundle = self.root / "bundle"
        self.bundle.mkdir()
        (self.bundle / "manifest.json").write_text(
            json.dumps(
                {
                    "head": "a" * 40,
                    "files": [{"path": "example.py", "additions": 1, "deletions": 0}],
                    "committed": False,
                }
            )
        )
        (self.bundle / "pr.diff").write_text(
            "diff --git a/example.py b/example.py\n+new\n"
        )
        self.questions = self.bundle / "questions.json"
        self.questions.write_text("[]")
        self.app = workbench.Workbench(self.bundle, self.questions)
        # Git receipt revocation is covered by the separate hook integration suite.
        self.app.revoke = lambda: None
        self.control = self.root / "control.json"
        self.mode("valid")
        self.executable = self.root / "codex"
        self.executable.write_text(
            f"#!{sys.executable}\n"
            + r"""
import json,os,sys,time
from pathlib import Path
root=Path(__file__).parent
if sys.argv[1:]==['login','status']:sys.exit(0)
with (root/'calls').open('a') as f:f.write(str(os.getpid())+'\n')
mode=json.loads((root/'control.json').read_text())['mode']
while mode=='hold' and not (root/'release').exists():time.sleep(.02)
prompt=sys.stdin.read()
(root/('prompt-'+str(os.getpid()))).write_text(prompt)
if mode=='fail':sys.exit(1)
value={'notes':[{'field':'motivation','text':'Fixture-only source consistency feedback.'}]}
if mode=='bad':value['notes']=[]
if mode=='max-note':
 schema=json.loads(Path(sys.argv[sys.argv.index('--output-schema')+1]).read_text())
 value['notes'][0]['text']='x'*schema['properties']['notes']['items']['properties']['text']['maxLength']
Path(sys.argv[sys.argv.index('--output-last-message')+1]).write_text(json.dumps(value))
"""
        )
        self.executable.chmod(0o700)

    def mode(self, mode):
        self.control.write_text(json.dumps({"mode": mode}))

    def submit(self, text="This fixture makes the missing submission handoff visible."):
        s = self.app.snapshot()
        self.app.save(
            {
                "identity": s["identity"],
                "revision": s["revision"],
                "answers": {"motivation": text},
            }
        )
        s = self.app.snapshot()
        return self.app.request_review(
            {"identity": s["identity"], "revision": s["revision"]}
        )

    def start(self, timeout=5):
        instance = worker.Reviewer(
            self.app, executable=str(self.executable), timeout=timeout
        )
        self.addCleanup(instance.close)
        return instance

    def test_truncation_notice_fits_with_a_maximum_length_reference(self):
        (self.bundle / "pr.diff").write_text(
            (self.bundle / "pr.diff").read_text() + "+x\n" * 70000
        )
        self.app = workbench.Workbench(self.bundle, self.questions)
        self.app.revoke = lambda: None
        self.mode("max-note")
        self.submit()
        self.start()
        self.until(lambda: self.app.snapshot()["status"] == "reflection_needed")
        note = self.app.snapshot()["feedback"]["notes"][0]["text"]
        self.assertLessEqual(len(note), 8000)
        self.assertIn("condensed or left out", note)

    def test_the_question_file_is_shown_first_when_the_diff_must_be_cut(self):
        big = lambda name: (  # noqa: E731
            f"diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n@@ -0,0 +1,9000 @@\n"
            + "+filler line\n" * 9000
        )
        asked = (
            "diff --git a/asked.py b/asked.py\n--- a/asked.py\n+++ b/asked.py\n"
            "@@ -0,0 +1 @@\n+the_passage_the_question_is_about()\n"
        )
        (self.bundle / "pr.diff").write_text(
            big("one.py") + big("two.py") + big("three.py") + asked
        )
        manifest = json.loads((self.bundle / "manifest.json").read_text())
        manifest["files"] += [
            {"path": name, "additions": 1, "deletions": 0}
            for name in ("one.py", "two.py", "three.py", "asked.py")
        ]
        (self.bundle / "manifest.json").write_text(json.dumps(manifest))
        self.questions.write_text(
            json.dumps(
                [
                    {
                        "id": "core",
                        "path": "asked.py",
                        "prompt": "What does the asked passage do?",
                        "quote": "the_passage_the_question_is_about()",
                    }
                ]
            )
        )
        self.app = workbench.Workbench(self.bundle, self.questions)
        self.app.revoke = lambda: None
        self.submit(
            "It calls the_passage_the_question_is_about() before replying to the caller."
        )
        self.start()
        self.until(lambda: self.app.snapshot()["status"] == "reflection_needed")
        prompt = next(self.root.glob("prompt-*")).read_text()
        evidence = json.loads(prompt[prompt.index('{"head"') :])
        self.assertIn("the_passage_the_question_is_about()", evidence["diff"])
        self.assertTrue(evidence["diff_truncated"])
        # Changed lines alone do not shrink a file that is only additions, so a
        # large one that does not fit is shown by its opening lines.
        self.assertTrue(evidence["files_first_lines_only"])
        self.assertNotIn("asked.py", evidence["files_first_lines_only"])
        self.assertEqual(evidence["files_counts_only"], [])
        self.assertIn("more changed lines not shown", evidence["diff"])
        self.assertLessEqual(len(evidence["diff"].encode()), 200000)

    def test_a_large_diff_keeps_every_file_by_dropping_context_first(self):
        def padded(name):
            return (
                f"diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n"
                "@@ -1,6001 +1,6001 @@\n"
                + " unchanged context line\n" * 6000
                + f"-old_{name}\n+new_{name}\n"
            )

        asked = (
            "diff --git a/asked.py b/asked.py\n--- a/asked.py\n+++ b/asked.py\n"
            "@@ -0,0 +1 @@\n+the_passage_the_question_is_about()\n"
        )
        names = ["one.py", "two.py", "three.py", "four.py", "five.py"]
        (self.bundle / "pr.diff").write_text("".join(padded(n) for n in names) + asked)
        manifest = json.loads((self.bundle / "manifest.json").read_text())
        manifest["files"] += [
            {"path": n, "additions": 1, "deletions": 1} for n in [*names, "asked.py"]
        ]
        (self.bundle / "manifest.json").write_text(json.dumps(manifest))
        self.questions.write_text(
            json.dumps(
                [
                    {
                        "id": "core",
                        "path": "asked.py",
                        "prompt": "What does the asked passage do?",
                        "quote": "the_passage_the_question_is_about()",
                    }
                ]
            )
        )
        self.app = workbench.Workbench(self.bundle, self.questions)
        self.app.revoke = lambda: None
        self.submit(
            "It calls the_passage_the_question_is_about() before replying to the caller."
        )
        self.start()
        self.until(lambda: self.app.snapshot()["status"] == "reflection_needed")
        prompt = next(self.root.glob("prompt-*")).read_text()
        evidence = json.loads(prompt[prompt.index('{"head"') :])
        self.assertIn("the_passage_the_question_is_about()", evidence["diff"])
        self.assertTrue(evidence["files_without_context"])
        self.assertEqual(evidence["files_counts_only"], [])
        for name in names:
            self.assertIn(f"new_{name}", evidence["diff"], name)
        self.assertLessEqual(len(evidence["diff"].encode()), 200000)

    def test_moved_lines_and_whitespace_edits_are_folded_before_anything_is_cut(self):
        func = [
            "def compute(total, counted):",
            "    result = max(total, counted)",
            "    return result + len(str(total))",
            "    # keep the larger reading",
        ]
        filler = "+filler line\n" * 18000

        def block(name, hunk):
            return f"diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n{hunk}"

        diff = (
            block("big.py", "@@ -0,0 +1,18000 @@\n" + filler)
            + block(
                "a.py",
                "@@ -1,6 +1,2 @@\n head\n"
                + "".join(f"-{x}\n" for x in func)
                + "-    x  = 1\n+    x = 1\n",
            )
            + block(
                "b.py",
                "@@ -1,1 +1,5 @@\n head\n" + "".join(f"+{x}\n" for x in func),
            )
            + block(
                "asked.py",
                "@@ -0,0 +1 @@\n+the_passage_the_question_is_about()\n",
            )
        )
        (self.bundle / "pr.diff").write_text(diff)
        manifest = json.loads((self.bundle / "manifest.json").read_text())
        manifest["files"] = [
            {"path": "big.py", "additions": 18000, "deletions": 0},
            {"path": "a.py", "additions": 1, "deletions": 5},
            {"path": "b.py", "additions": 4, "deletions": 0},
            {"path": "asked.py", "additions": 1, "deletions": 0},
        ]
        (self.bundle / "manifest.json").write_text(json.dumps(manifest))
        self.questions.write_text(
            json.dumps(
                [
                    {
                        "id": "core",
                        "path": "asked.py",
                        "prompt": "What does the asked passage do?",
                        "quote": "the_passage_the_question_is_about()",
                    }
                ]
            )
        )
        self.app = workbench.Workbench(self.bundle, self.questions)
        self.app.revoke = lambda: None
        self.submit(
            "It calls the_passage_the_question_is_about() before replying to the caller."
        )
        self.start()
        self.until(lambda: self.app.snapshot()["status"] == "reflection_needed")
        prompt = next(self.root.glob("prompt-*")).read_text()
        evidence = json.loads(prompt[prompt.index('{"head"') :])
        shown = evidence["diff"]
        self.assertIn(
            "lines moved here from a.py:2 (unchanged text not repeated)", shown
        )
        self.assertNotIn("def compute", shown)
        self.assertIn("# whitespace-only change", shown)
        self.assertEqual(evidence["folded"]["moved_lines"], 8)
        self.assertEqual(evidence["folded"]["whitespace_lines"], 2)

    def test_transient_session_read_failure_does_not_repeat_the_cli_call(self):
        self.mode("hold")
        self.submit()
        original = worker.Reviewer.current
        failures = []

        def unavailable(instance, snapshot):
            if instance.app is self.app and len(failures) < 2:
                self.until(lambda: (self.root / "calls").exists())
                failures.append(True)
                raise ValueError("Temporary session read failure")
            return original(instance, snapshot)

        with patch.object(worker.Reviewer, "current", unavailable):
            self.start()
            self.until(
                lambda: (
                    (self.bundle / "reviewer-job.json").exists()
                    and json.loads((self.bundle / "reviewer-job.json").read_text())[
                        "state"
                    ]
                    == "failed"
                )
            )
            time.sleep(0.5)
            self.assertEqual(len((self.root / "calls").read_text().splitlines()), 1)
            self.assertEqual(self.app.snapshot()["reviewer"], "error")

    def until(self, condition):
        deadline = time.monotonic() + 8
        while not condition():
            if time.monotonic() >= deadline:
                self.fail("Worker condition did not settle")
            time.sleep(0.03)

    def calls(self):
        path = self.root / "calls"
        return path.read_text().splitlines() if path.exists() else []

    def test_one_submission_produces_feedback_once_without_a_chat_waiter(self):
        self.submit()
        self.start()
        self.until(lambda: self.app.snapshot()["status"] == "reflection_needed")
        time.sleep(0.4)
        self.assertEqual(len(self.calls()), 1)
        self.assertEqual(
            self.app.snapshot()["feedback"]["notes"][0]["field"], "motivation"
        )

    def test_failure_is_visible_and_only_explicit_resubmission_retries(self):
        self.mode("fail")
        initial = self.submit()
        self.start()
        self.until(lambda: self.app.snapshot()["reviewer"] == "error")
        time.sleep(0.4)
        self.assertEqual(len(self.calls()), 1)
        self.mode("valid")
        s = self.app.snapshot()
        retry = self.app.request_review(
            {"identity": s["identity"], "revision": s["revision"]}
        )
        self.assertNotEqual(retry["request_id"], initial["request_id"])
        duplicate = self.app.request_review(
            {"identity": retry["identity"], "revision": retry["revision"]}
        )
        self.assertEqual(duplicate["request_id"], retry["request_id"])
        self.until(lambda: self.app.snapshot()["status"] == "reflection_needed")
        self.assertEqual(len(self.calls()), 2)

    def test_changed_answers_cancel_the_old_process_and_its_result(self):
        self.mode("hold")
        self.submit()
        instance = self.start()
        self.until(lambda: len(self.calls()) == 1)
        s = self.app.snapshot()
        self.app.save(
            {
                "identity": s["identity"],
                "revision": s["revision"],
                "answers": {"motivation": "A different reason needs its own feedback."},
            }
        )
        self.until(lambda: instance.job().get("state") == "cancelled")
        self.assertIsNone(self.app.snapshot()["feedback"])
        self.mode("valid")
        self.submit("A different reason needs its own feedback.")
        self.until(lambda: self.app.snapshot()["status"] == "reflection_needed")
        self.assertEqual(len(self.calls()), 2)
        self.assertIn("different reason", next(self.root.glob("prompt-*")).read_text())

    def test_saving_a_previous_answer_does_not_silently_resubmit_it(self):
        self.mode("hold")
        original = "This exact answer was submitted before editing."
        self.submit(original)
        instance = self.start()
        self.until(lambda: len(self.calls()) == 1)
        for text in ["An unsent changed draft.", original]:
            s = self.app.snapshot()
            self.app.save(
                {
                    "identity": s["identity"],
                    "revision": s["revision"],
                    "answers": {"motivation": text},
                }
            )
        self.until(lambda: instance.job().get("state") == "cancelled")
        time.sleep(0.4)
        self.assertEqual(self.app.snapshot()["status"], "ready_to_send")
        self.assertEqual(len(self.calls()), 1)

    def test_human_feedback_cannot_be_overwritten_by_an_inflight_worker(self):
        self.mode("hold")
        s = self.submit()
        instance = self.start()
        self.until(lambda: len(self.calls()) == 1)
        self.app.feedback(
            {
                "identity": s["identity"],
                "answer_digest": s["answer_digest"],
                "notes": [
                    {
                        "field": "motivation",
                        "text": "Human feedback owns this submission.",
                    }
                ],
            }
        )
        self.until(lambda: instance.job().get("state") == "cancelled")
        self.assertEqual(
            self.app.snapshot()["feedback"]["notes"][0]["text"],
            "Human feedback owns this submission.",
        )

    def test_timeout_is_bounded_even_when_the_cli_does_not_read_its_input(self):
        (self.bundle / "pr.diff").write_text("+" + "x" * 210000)
        self.app = workbench.Workbench(self.bundle, self.questions)
        self.app.revoke = lambda: None
        self.mode("hold")
        self.submit()
        instance = self.start(timeout=0.3)
        self.until(lambda: instance.job().get("state") == "failed")
        self.assertEqual(instance.job()["error"], "reviewer_timeout")
        self.assertIsNone(self.app.snapshot()["feedback"])

    def test_restart_does_not_blindly_replay_an_interrupted_provider_call(self):
        s = self.submit()
        self.app.write(
            {
                "identity": s["identity"],
                "request_id": s["request_id"],
                "answer_digest": s["answer_digest"],
                "state": "running",
            },
            self.bundle / "reviewer-job.json",
        )
        instance = self.start()
        self.until(lambda: self.app.snapshot()["reviewer"] == "error")
        self.assertEqual(instance.job()["error"], "reviewer_interrupted")
        self.assertEqual(self.calls(), [])

    def test_invalid_structured_output_does_not_approve_anything(self):
        self.mode("bad")
        self.submit()
        self.start()
        self.until(lambda: self.app.snapshot()["reviewer"] == "error")
        self.assertIsNone(self.app.snapshot()["feedback"])
        self.assertEqual(self.app.snapshot()["status"], "awaiting_review")

    def http_server(self):
        repo = self.root / "repo"
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        token = self.root / "token"
        token.write_text("a" * 43)
        token.chmod(0o600)
        process = subprocess.Popen(
            [
                sys.executable,
                str(SKILL / "workbench.py"),
                "serve",
                "--port",
                "0",
                "--reviewer",
                "codex",
                "--token-file",
                str(token),
                "--bundle",
                str(self.bundle),
                "--questions",
                str(self.questions),
            ],
            cwd=repo,
            env={
                **os.environ,
                "PATH": str(self.root) + os.pathsep + os.environ["PATH"],
            },
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

        def stop():
            if process.poll() is None:
                process.terminate()
            process.communicate(timeout=10)

        self.addCleanup(stop)
        self.assertTrue(select.select([process.stdout], [], [], 8)[0])
        url = urlsplit(process.stdout.readline().strip())
        self.assertEqual(url.fragment, "token=" + "a" * 43)

        def request(path, value=None):
            body = json.dumps(value).encode() if value is not None else None
            req = urllib.request.Request(
                f"http://127.0.0.1:{url.port}" + path,
                data=body,
                method="PUT" if body else "GET",
                headers={
                    "Authorization": "Bearer " + "a" * 43,
                    "Content-Type": "application/json",
                },
            )
            with urllib.request.urlopen(req, timeout=5) as response:
                return json.load(response)

        return process, request

    def test_http_service_drives_feedback_without_a_chat_wait_process(self):
        _, request = self.http_server()
        s = request("/api/session")
        s = request(
            "/api/answers",
            {
                "identity": s["identity"],
                "revision": s["revision"],
                "answers": {
                    "motivation": "The submission handoff must keep working after the chat ends, because the managed worker owns the request."
                },
            },
        )
        s = request(
            "/api/review", {"identity": s["identity"], "revision": s["revision"]}
        )
        self.until(lambda: request("/api/session")["status"] == "reflection_needed")
        self.assertEqual(len(self.calls()), 1)
        self.assertEqual(
            request("/api/session")["feedback"]["answer_digest"], s["answer_digest"]
        )

    def test_server_shutdown_terminates_the_inflight_reviewer(self):
        self.mode("hold")
        process, request = self.http_server()
        s = request("/api/session")
        s = request(
            "/api/answers",
            {
                "identity": s["identity"],
                "revision": s["revision"],
                "answers": {
                    "motivation": "This request must be cancellable, so shutting the server down has to stop the reviewer process too."
                },
            },
        )
        request("/api/review", {"identity": s["identity"], "revision": s["revision"]})
        self.until(lambda: len(self.calls()) == 1)
        pid = int(self.calls()[0])
        process.terminate()
        process.wait(timeout=8)
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)

    def test_insecure_or_symlinked_token_files_cannot_start_a_server(self):
        token = self.root / "insecure-token"
        token.write_text("a" * 43)
        token.chmod(0o644)
        link = self.root / "token-link"
        link.symlink_to(token)
        for path in [token, link]:
            if path == link:
                token.chmod(0o600)
            result = subprocess.run(
                [
                    sys.executable,
                    str(SKILL / "workbench.py"),
                    "serve",
                    "--port",
                    "0",
                    "--bundle",
                    str(self.bundle),
                    "--questions",
                    str(self.questions),
                    "--token-file",
                    str(path),
                ],
                capture_output=True,
                text=True,
                timeout=5,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, "")

    def test_a_bundle_has_only_one_worker_owner(self):
        self.start()
        with self.assertRaises(OSError):
            worker.Reviewer(self.app, executable=str(self.executable))


if __name__ == "__main__":
    unittest.main()
