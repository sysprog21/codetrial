#!/usr/bin/env python3
"""Local explanation workbench over an existing, immutable review bundle."""

import argparse
import copy
from contextlib import contextmanager
import errno
import fcntl
import functools
import importlib.util
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import secrets
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time
from urllib.parse import urlsplit


FIELDS = ("motivation",)
CONFIDENCE = ("sure", "probably", "guessing", "unknown")
ASSETS = {
    "/": ("index.html", "text/html"),
    "/app.js": ("app.js", "text/javascript"),
    "/diffview.js": ("diffview.js", "text/javascript"),
    "/explainer.js": ("explainer.js", "text/javascript"),
    "/style.css": ("style.css", "text/css"),
    "/styles.css": ("shared", "text/css"),
}
LIMIT = 1024 * 1024
DEFAULT_PORT = 7777


def digest(value):
    # The gate verifies what this writes, so there is one definition of both.
    return load_gate().digest(value)


def load(path):
    return json.loads(path.read_text())


def file_patch(diff, path):
    def quoted(value):
        escape = {9: r"\t", 10: r"\n", 13: r"\r", 34: r"\"", 92: r"\\"}
        return (
            '"'
            + "".join(
                escape.get(b, chr(b) if 32 <= b < 127 else "\\%03o" % b)
                for b in value.encode()
            )
            + '"'
        )

    names = ["a/" + path, "b/" + path]
    headers = {
        "diff --git " + " ".join(names),
        "diff --git " + " ".join(map(quoted, names)),
    }
    # The path's current content wins over a block that only renames it away,
    # which can sort first when a file is renamed and then recreated.
    source = None
    for block in re.split(r"(?=^diff --git )", diff, flags=re.MULTILINE):
        header = block.split("\n", 1)[0]
        metadata = block.split("\n@@", 1)[0].splitlines()[1:]
        destination = {
            "+++ " + names[1],
            "+++ " + quoted(names[1]),
            "rename to " + path,
            "rename to " + quoted(path),
        }
        origin = {
            "--- " + names[0],
            "--- " + quoted(names[0]),
            "rename from " + path,
            "rename from " + quoted(path),
        }
        if header in headers or any(line in destination for line in metadata):
            return block
        if source is None and any(line in origin for line in metadata):
            source = block
    return source or ""


@functools.lru_cache(maxsize=None)
def load_sibling(name):
    """A module that sits beside this file, loaded by path and kept."""
    path = Path(__file__).with_name(f"{name}.py")
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def load_diffmodel():
    return load_sibling("diffmodel")


def load_understanding():
    return load_sibling("understanding")


def load_explainer():
    return load_sibling("explainer")


def reflected(value, identity, answer_digest, feedback):
    return load_gate().reflected(value, identity, answer_digest, feedback)


class Workbench:
    def __init__(self, bundle, questions):
        self.bundle = Path(bundle).resolve()
        self.manifest = load(self.bundle / "manifest.json")
        self.diff = (self.bundle / "pr.diff").read_text()
        self.questions_path = Path(questions).resolve()
        self.questions = load(self.questions_path)
        paths = [f["path"] for f in self.manifest["files"]]
        if len(paths) != len(set(paths)):
            raise ValueError("Duplicate manifest paths")
        if not isinstance(self.questions, list) or len(self.questions) > 3:
            raise ValueError(
                "Use a fresh bundle with at most three questions: one core and two optional reading prompts"
            )
        ids = []
        question_paths = []
        for question in self.questions:
            if (
                not isinstance(question, dict)
                or set(question)
                not in ({"id", "path", "prompt"}, {"id", "path", "prompt", "quote"})
                or not all(isinstance(v, str) and v.strip() for v in question.values())
            ):
                raise ValueError("Each question needs nonempty id, path and prompt")
            if len(question["id"]) > 128 or len(question["prompt"]) > 4000:
                raise ValueError(
                    "Keep question ids within 128 characters and prompts within 4000"
                )
            if question["path"] not in paths:
                raise ValueError("Question path is outside the manifest")
            if "quote" in question and (
                len(question["quote"]) > 2000
                or question["quote"] not in file_patch(self.diff, question["path"])
            ):
                raise ValueError("Question quote is not in the pinned diff")
            ids.append(question["id"])
            question_paths.append(question["path"])
        if len(ids) != len(set(ids)):
            raise ValueError("Question ids must be unique")
        if len(question_paths) != len(set(question_paths)):
            raise ValueError("Use only one question per file")
        self.keys = [*FIELDS, *("reading:" + q["id"] for q in self.questions[1:])]
        # What the core answer is held against: the question's own file, or the
        # whole change when the question is the generic one.
        self.core_patch = (
            file_patch(self.diff, self.questions[0]["path"])
            if self.questions
            else self.diff
        )
        self.identity = digest([self.manifest, self.diff, self.questions])
        self.state_path = self.bundle / "explanation.json"
        self.presence_path = self.bundle / "explanation-agent.json"
        self._reading = None
        self._notes = {}
        # Handler threads share these caches.
        self._cache_lock = threading.Lock()

    @contextmanager
    def locked(self):
        fd = os.open(self.bundle / "explanation.lock", os.O_CREAT | os.O_RDWR, 0o600)
        with os.fdopen(fd, "w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            yield

    def state(self):
        state = (
            load(self.state_path)
            if self.state_path.exists()
            else {
                "identity": self.identity,
                "revision": 0,
                "answers": {},
                "feedback": None,
                "requested_digest": None,
                "request_id": None,
                "submission": None,
                "reflection": None,
                "history": [],
            }
        )
        if state["identity"] != self.identity:
            raise ValueError(
                "Bundle or questions changed; keep the old answers and use a new bundle directory"
            )
        if any(key not in [*self.keys, "confidence"] for key in state["answers"]):
            raise ValueError(
                "This draft uses the older questionnaire; keep it and open a fresh motivation bundle"
            )
        if (
            digest(
                [
                    load(self.bundle / "manifest.json"),
                    (self.bundle / "pr.diff").read_text(),
                    load(self.questions_path),
                ]
            )
            != self.identity
        ):
            raise ValueError(
                "Bundle changed while the workbench was open; prepare a new session"
            )
        return state

    def write(self, state, target=None):
        serialized = json.dumps(state, indent=2, ensure_ascii=False) + "\n"
        if len(serialized.encode()) > LIMIT:
            raise ValueError(
                "Session record is full; download it and start a fresh bundle"
            )
        fd, name = tempfile.mkstemp(prefix=".explanation-", dir=self.bundle)
        try:
            with os.fdopen(fd, "w") as stream:
                stream.write(serialized)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(name, target or self.state_path)
            directory = os.open(self.bundle, os.O_RDONLY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
        finally:
            if os.path.exists(name):
                os.unlink(name)

    def revoke(self):
        load_gate().revoke(self.manifest["head"], self.identity)

    def diff_reading(self):
        """The diff as files, hunks and labelled lines, with what does not add up.

        Read once: the diff is part of the session's identity and never changes.
        """
        if self._reading is None:
            self._reading = load_diffmodel().reading(
                self.diff,
                self.manifest["files"],
                self.manifest.get("files_complete") is not False,
            )
        return self._reading

    def read_note(self, text):
        """A reference note as blocks, every citation held against the pinned diff."""
        # The same note is read by every snapshot, and the diff never changes.
        read = self._notes.get(text)
        if read is None:
            reading = self.diff_reading()
            read = load_explainer().read(
                text, reading["files"], trusted=not reading["problems"]
            )
            # A second thread may have read it too: the inputs are pinned, so
            # either result is the same, and the lock only guards the dict.
            with self._cache_lock:
                if len(self._notes) >= 32:
                    self._notes.pop(next(iter(self._notes)))
                read = self._notes.setdefault(text, read)
        return read

    def reference_view(self, feedback):
        """What the page draws for each note, or None for a note it cannot read.

        A stored note that no longer parses is shown as the text it is.
        """
        if not feedback:
            return []
        views = []
        for note in feedback["notes"]:
            read = self.read_note(note["text"])
            views.append(None if read["problems"] else read["blocks"])
        return views

    def checks(self, answers):
        """The deterministic checks on the core answer; see understanding.py."""
        return load_understanding().check(
            answers.get("motivation", ""),
            self.questions[0] if self.questions else None,
            self.core_patch,
        )

    def require_own_words(self):
        """Refuse a send whose core answer is too short or copied.

        Saving a draft is never refused; only the explicit send is, and only
        for what is cheap to see. Whether the answer names the changed code
        is reported, not enforced.
        """
        with self.locked():
            answers = self.state()["answers"]
        problem = load_understanding().problem(self.checks(answers))
        if problem:
            raise ValueError(problem)

    def snapshot(self):
        with self.locked():
            state = self.state()
        missing = [k for k in FIELDS if not state["answers"].get(k, "").strip()]
        fingerprint = digest(state["answers"])
        feedback = state["feedback"]
        current = (
            feedback
            and feedback["answer_digest"] == fingerprint
            and state.get("requested_digest") == fingerprint
            and (state.get("submission") or {}).get("request_id")
            == state.get("request_id")
            and state.get("request_id")
        )
        status = "draft" if missing else "ready_to_send"
        if not missing and state.get("requested_digest") == fingerprint:
            status = "awaiting_review"
        if current:
            status = (
                "explanation_complete"
                if not missing
                and reflected(
                    state.get("reflection"), self.identity, fingerprint, feedback
                )
                else "reflection_needed"
            )
        files = self.manifest["files"]
        known = all(
            type(f.get(k)) is int and f[k] >= 0
            for f in files
            for k in ("additions", "deletions")
        )
        complete = self.manifest.get("files_complete", True) and not self.manifest.get(
            "untracked"
        )
        sizes = self.manifest.get("commit_sizes", {})
        commit_coverage = (
            sizes.get("head") == self.manifest["head"] and sizes.get("complete") is True
        )
        large_commits = [
            c
            for c in sizes.get("commits", [])
            if sizes.get("head") == self.manifest["head"]
            and c.get("known_lines", 0) >= load_gate().LARGE_COMMIT
        ]
        presence = load(self.presence_path) if self.presence_path.exists() else {}
        reviewer = "offline"
        if (
            presence.get("identity") == self.identity
            and presence.get("until", 0) > time.time()
        ):
            reviewer = presence["state"]
        return {
            **state,
            "manifest": self.manifest,
            "diff": self.diff,
            "questions": self.questions,
            "missing": missing,
            "answer_digest": fingerprint,
            "feedback_digest": digest(feedback) if feedback else None,
            "status": status,
            "total": sum(f["additions"] + f["deletions"] for f in files)
            if known
            else None,
            "coverage_complete": bool(known and complete),
            "committed": self.manifest.get("committed") is True
            or self.manifest.get("pr") is not None,
            "reviewer": reviewer,
            "reviewer_error": presence.get("error") if reviewer == "error" else None,
            "large_commits": large_commits,
            "commit_coverage_complete": commit_coverage,
        }

    def page_snapshot(self):
        """The snapshot with what is drawn for the contributor alone.

        The agent CLI and the managed worker read snapshot(), which never holds
        the drawn note, so the worker does not compute what it would discard.
        """
        snapshot = self.snapshot()
        return {
            **snapshot,
            "reference_view": self.reference_view(snapshot["feedback"]),
        }

    def finish(self):
        """Allow the session to end; only a confirmed explanation can."""
        if self.snapshot()["status"] != "explanation_complete":
            raise ValueError("Confirm your reflection before finishing")
        return {"finished": True}

    def reported_checks(self, answers):
        understanding = load_understanding()
        checks = self.checks(answers)
        return {
            "words": checks["words"],
            "minimum": understanding.MIN_WORDS,
            "grounded": checks["grounded"],
            "anchors": list(checks["anchors"]),
            "state": understanding.state(checks),
            "message": understanding.message(checks),
        }

    def check_draft(self, payload):
        """The checks on text being typed; nothing is saved or recorded."""
        if not isinstance(payload, dict) or set(payload) != {"identity", "text"}:
            raise ValueError("Expected identity and text")
        if payload["identity"] != self.identity:
            raise ValueError("Session changed; reload before continuing")
        if not isinstance(payload["text"], str) or len(payload["text"]) > 40000:
            raise ValueError("Answer exceeds 40000 characters")
        return self.reported_checks({"motivation": payload["text"]})

    def save(self, payload):
        if not isinstance(payload, dict) or set(payload) != {
            "identity",
            "revision",
            "answers",
        }:
            raise ValueError("Expected identity, revision and answers")
        answers = payload["answers"]
        if not isinstance(answers, dict) or any(
            k not in [*self.keys, "confidence"]
            or not isinstance(v, str)
            or len(v) > 40000
            for k, v in answers.items()
        ):
            raise ValueError("Unknown answer field or answer exceeds 40000 characters")
        if "confidence" in answers and answers["confidence"] not in CONFIDENCE:
            raise ValueError("Unknown confidence choice")
        with self.locked():
            state = self.state()
            if (
                payload["identity"] != self.identity
                or payload["revision"] != state["revision"]
            ):
                raise ValueError(
                    "Session changed; reload before saving (your text is still in the form)"
                )
            # Nothing changed: a new revision would make `wait --after` hand
            # the same pending request to the reviewer again.
            if answers != state["answers"]:
                self.admit_submission(state, answers)
                self.revoke()
                state["requested_digest"] = None
                state["request_id"] = None
                state["reflection"] = None
                state["answers"] = answers
                state["revision"] += 1
                self.write(state)
        return self.snapshot()

    def submission(self, answers, fingerprint, request_id):
        return {
            "head": self.manifest["head"],
            "against": self.manifest.get("against"),
            "identity": self.identity,
            "answer_digest": fingerprint,
            "request_id": request_id,
            "focus": self.questions,
            "answers": dict(answers),
            # Facts for the reviewer, not a verdict: whether the answer names
            # anything from the changed lines, and which names were available.
            "checks": {
                k: v
                for k, v in self.reported_checks(answers).items()
                if k in ("words", "grounded", "anchors")
            },
        }

    def admit_submission(self, state, answers):
        preview = copy.deepcopy(state)
        preview["answers"] = answers
        preview["revision"] += 2
        preview["requested_digest"] = digest(answers)
        preview["request_id"] = "0" * 32
        if preview["feedback"]:
            self.event(preview, "previous_reference", preview["feedback"])
        preview["feedback"] = preview["reflection"] = None
        preview["submission"] = self.submission(
            answers, digest(answers), preview["request_id"]
        )
        self.event(preview, "author", preview["submission"])
        # Reserve space for the bounded reference and the author's correction.
        if len(preview.get("history", [])) + 2 > 64:
            raise ValueError(
                "Session history is full; export it and start a fresh session"
            )
        if (
            len(json.dumps(preview, indent=2, ensure_ascii=False).encode()) + 192000
            > LIMIT
        ):
            raise ValueError(
                "Explanations and history exceed the submission budget; shorten the text or export it and start a fresh session"
            )

    def request_review(self, payload):
        if not isinstance(payload, dict) or set(payload) != {"identity", "revision"}:
            raise ValueError("Expected identity and revision")
        with self.locked():
            state = self.state()
            if (
                payload["identity"] != self.identity
                or payload["revision"] != state["revision"]
            ):
                raise ValueError("Session changed; check for updates before sending")
            if any(not state["answers"].get(k, "").strip() for k in FIELDS):
                raise ValueError(
                    "Explain your motivation before sending; answer the follow-up if present"
                )
            if (
                state.get("requested_digest") == digest(state["answers"])
                and state.get("submission")
                and state["feedback"]
                and state["feedback"]["answer_digest"] == digest(state["answers"])
            ):
                raise ValueError(
                    "These answers already have feedback; revise them before sending again"
                )
            fingerprint = digest(state["answers"])
            job_path = self.bundle / "reviewer-job.json"
            job = load(job_path) if job_path.exists() else {}
            retry = (
                job.get("identity") == self.identity
                and job.get("request_id")
                == (state.get("request_id") or "legacy:" + fingerprint)
                and job.get("state") == "failed"
            )
            if (
                state.get("requested_digest") != fingerprint
                or retry
                or not state.get("request_id")
                or not state.get("submission")
                or state["submission"].get("request_id") != state["request_id"]
            ):
                self.admit_submission(state, state["answers"])
                if state["feedback"]:
                    self.event(state, "previous_reference", state["feedback"])
                state["feedback"] = None
                state["reflection"] = None
                state["requested_digest"] = fingerprint
                state["request_id"] = secrets.token_hex(16)
                state["submission"] = self.submission(
                    state["answers"], fingerprint, state["request_id"]
                )
                self.event(state, "author", state["submission"])
                state["revision"] += 1
                self.write(state)
        return self.snapshot()

    def presence(self, state, seconds):
        self.write(
            {"identity": self.identity, "state": state, "until": time.time() + seconds},
            self.presence_path,
        )

    def wait(self, after, timeout, event="submission"):
        """Block until the page needs the agent, or `timeout` seconds pass.

        `submission` is the contributor asking for a reference; the agent is
        shown as waiting for it. `confirmation` is the contributor confirming
        their self-assessment, which leaves the agent the closeout to do
        (certify, then report); it is a watch rather than a task, so it does not
        claim the agent is waiting on the page.
        """
        if event not in ("submission", "confirmation"):
            raise ValueError("Wait for a submission or a confirmation")
        deadline = time.monotonic() + timeout
        task = event == "submission"
        if task:
            self.presence("waiting", timeout + 2)
        try:
            while True:
                snapshot = self.snapshot()
                if snapshot["revision"] > after and snapshot["status"] == (
                    "awaiting_review" if task else "explanation_complete"
                ):
                    if task:
                        self.presence("reading", 120)
                    return snapshot
                if time.monotonic() >= deadline:
                    if task:
                        self.presence("offline", 0)
                    return None
                time.sleep(min(0.25, max(0, deadline - time.monotonic())))
        except BaseException:
            if task:
                self.presence("offline", 0)
            raise

    def check_note(self, feedback):
        """Refuse a note whose drawn parts the diff does not hold.

        For the agent that wrote it and can fix it. The managed worker cannot be
        asked again, so it is not checked: it stores what it wrote, and a note
        that does not read is shown as the text it is.
        """
        notes = feedback.get("notes") if isinstance(feedback, dict) else None
        for note in notes if isinstance(notes, list) else []:
            text = note.get("text") if isinstance(note, dict) else None
            problems = self.read_note(text)["problems"] if isinstance(text, str) else []
            if problems:
                raise ValueError("Reference note: " + "; ".join(problems[:3]))

    def feedback(self, feedback, *, request=None, replace=False):
        if not isinstance(feedback, dict) or set(feedback) != {
            "identity",
            "answer_digest",
            "notes",
        }:
            raise ValueError(
                "Feedback needs identity, answer_digest and reference notes, not a score"
            )
        if not isinstance(feedback["notes"], list) or len(feedback["notes"]) != 1:
            raise ValueError(
                "Feedback must explain the source checks or remaining gaps"
            )
        for note in feedback["notes"]:
            if (
                not isinstance(note, dict)
                or set(note) != {"field", "text"}
                or note["field"] not in self.keys
                or not isinstance(note["text"], str)
                or not note["text"].strip()
                or len(note["text"]) > 8000
            ):
                raise ValueError(
                    "Each feedback note needs a known field and nonempty text"
                )
        with self.locked():
            state = self.state()
            if state.get("requested_digest") != feedback[
                "answer_digest"
            ] or not state.get("request_id"):
                raise ValueError(
                    "Answers have not been sent for review; choose Send to reviewer first"
                )
            if request is not None and (
                state.get("request_id") != request.get("request_id")
                or state.get("requested_digest") != feedback["answer_digest"]
                or (
                    not replace
                    and state["feedback"]
                    and state["feedback"]["answer_digest"] == feedback["answer_digest"]
                )
            ):
                raise ValueError("Submission changed or already has feedback")
            if feedback["identity"] != self.identity or feedback[
                "answer_digest"
            ] != digest(state["answers"]):
                raise ValueError("Feedback is for different questions, code or answers")
            if feedback == state["feedback"]:
                return
            preview = copy.deepcopy(state)
            preview["feedback"] = feedback
            preview["reflection"] = None
            self.event(preview, "reference", feedback)
            preview["revision"] += 1
            if (
                len(preview.get("history", [])) + 1 > 64
                or len(json.dumps(preview, indent=2, ensure_ascii=False).encode())
                + 64000
                > LIMIT
            ):
                raise ValueError(
                    "Reference exceeds the reflection budget; shorten it or start a fresh session"
                )
            self.revoke()
            self.write(preview)
        self.presence("offline", 0)

    @staticmethod
    def event(state, kind, value):
        history = state.setdefault("history", [])
        if len(history) >= 64:
            raise ValueError(
                "Session history is full; download it and start a fresh bundle"
            )
        history.append({"kind": kind, "value": value})

    def reflect(self, payload):
        if not isinstance(payload, dict) or set(payload) != {
            "identity",
            "revision",
            "feedback_digest",
            "grade",
            "correction",
        }:
            raise ValueError("Expected a current reference and your self-assessment")
        with self.locked():
            state = self.state()
            feedback = state["feedback"]
            answer = digest(state["answers"])
            value = {
                "identity": self.identity,
                "answer_digest": answer,
                "feedback_digest": payload["feedback_digest"],
                "grade": payload["grade"],
                "correction": payload["correction"],
            }
            if (
                payload["identity"] != self.identity
                or payload["revision"] != state["revision"]
                or state.get("requested_digest") != answer
                or not state.get("request_id")
                or not state.get("submission")
                or state["submission"].get("request_id") != state["request_id"]
                or state["submission"]["answer_digest"] != answer
                or not feedback
                or feedback["answer_digest"] != answer
                or not reflected(value, self.identity, answer, feedback)
            ):
                raise ValueError(
                    "Reference changed, or your self-assessment needs a correction/uncertainty note"
                )
            self.revoke()
            state["reflection"] = value
            self.event(state, "reflection", value)
            state["revision"] += 1
            self.write(state)
        return self.snapshot()

    def basis(self):
        snapshot = self.snapshot()
        submission = snapshot.get("submission")
        if not submission:
            raise ValueError("No explicitly submitted contributor explanation")
        return {
            "authority": "context_only",
            "submission": submission,
            "current": (
                submission["answer_digest"] == snapshot["answer_digest"]
                and snapshot.get("requested_digest") == snapshot["answer_digest"]
                and submission.get("request_id") == snapshot.get("request_id")
            ),
            "reference": snapshot["feedback"],
            "reflection": snapshot.get("reflection"),
            "history": snapshot.get("history", []),
        }


@functools.lru_cache(maxsize=None)
def load_gate():
    path = Path(__file__).resolve().parents[3] / "scripts/check-interactive-review.py"
    spec = importlib.util.spec_from_file_location("interactive_evidence", path)
    gate = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(gate)
    return gate


def server(workbench, port, *, reviewer="manual", token=None):
    token = token or secrets.token_urlsafe(32)
    assets = Path(__file__).with_name("workbench")

    class Handler(BaseHTTPRequestHandler):
        # An idle or half-open connection must not hold the server.
        timeout = 5

        def log_message(self, *_args):
            pass

        def reply(self, code, data, content_type="application/json"):
            body = (
                json.dumps(data).encode()
                if content_type == "application/json"
                else data
            )
            self.send_response(code)
            self.send_header("Content-Type", content_type + "; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header(
                "Content-Security-Policy",
                "default-src 'self'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'",
            )
            self.end_headers()
            self.wfile.write(body)

        def authorized(self, token):
            return secrets.compare_digest(
                self.headers.get("Authorization", "").encode(),
                ("Bearer " + token).encode(),
            )

        def allowed(self):
            host = f"127.0.0.1:{self.server.server_port}"
            return (
                self.headers.get("Host") == host
                and self.headers.get("Origin", "http://" + host) == "http://" + host
            )

        def do_GET(self):
            if not self.allowed():
                return self.reply(403, {"error": "Use the local workbench URL"})
            route = urlsplit(self.path).path
            if route in ASSETS:
                name, kind = ASSETS[route]
                path = (
                    Path(__file__).resolve().parents[3] / "web/styles.css"
                    if name == "shared"
                    else assets / name
                )
                return self.reply(200, path.read_bytes(), kind)
            if route not in ("/api/session", "/api/diff"):
                return self.reply(404, {"error": "Not found"})
            if not self.authorized(token):
                return self.reply(
                    403, {"error": "Reopen the URL printed by the workbench"}
                )
            try:
                if route == "/api/diff":
                    reading = workbench.diff_reading()
                    # The raw text only travels when the reading cannot be trusted.
                    return self.reply(
                        200,
                        {
                            **reading,
                            "raw": workbench.diff if reading["problems"] else None,
                        },
                    )
                # The diff is fetched once from /api/diff, not on every poll.
                self.reply(
                    200,
                    {k: v for k, v in workbench.page_snapshot().items() if k != "diff"},
                )
            except (ValueError, OSError, subprocess.SubprocessError) as error:
                self.reply(409, {"error": str(error)})

        def do_PUT(self):
            if not self.allowed() or not self.authorized(token):
                return self.reply(
                    403, {"error": "Reopen the URL printed by the workbench"}
                )
            if self.path not in (
                "/api/answers",
                "/api/review",
                "/api/reflection",
                "/api/check",
                "/api/finish",
            ):
                return self.reply(404, {"error": "Not found"})
            try:
                length = int(self.headers.get("Content-Length", "0"))
                if not 0 < length <= LIMIT:
                    return self.reply(
                        413,
                        {"error": "Answer payload must be between 1 byte and 1 MiB"},
                    )
                payload = json.loads(self.rfile.read(length))
                if self.path == "/api/review":
                    workbench.require_own_words()
                result = (
                    workbench.finish()
                    if self.path == "/api/finish"
                    else workbench.check_draft(payload)
                    if self.path == "/api/check"
                    else workbench.save(payload)
                    if self.path == "/api/answers"
                    else workbench.reflect(payload)
                    if self.path == "/api/reflection"
                    else workbench.request_review(payload)
                )
                self.reply(200, result)
                if self.path == "/api/finish":
                    # From another thread: shutdown() waits for the serving
                    # loop, which is waiting on nothing here but the next request.
                    print("Interactive review finished; stopping.", file=sys.stderr)
                    threading.Thread(target=httpd.shutdown, daemon=True).start()
            except (
                ValueError,
                TypeError,
                OSError,
                subprocess.SubprocessError,
            ) as error:
                self.reply(409, {"error": str(error)})

    requested_port = port
    while True:
        try:
            httpd = ThreadingHTTPServer(("127.0.0.1", port), Handler)
            httpd.daemon_threads = True
            break
        except OSError as error:
            if error.errno == errno.EADDRINUSE and 0 < port < 65535:
                port += 1
                continue
            raise ValueError(
                f"Cannot bind review server at 127.0.0.1:{port}"
            ) from error
    if port != requested_port:
        print(
            f"Review port {requested_port} was busy; using 127.0.0.1:{port}.",
            file=sys.stderr,
        )
    httpd.timeout = 1
    worker = None
    try:
        gate = load_gate()
        gate.record_workbench(workbench.bundle)
        if reviewer == "codex":
            spec = importlib.util.spec_from_file_location(
                "review_worker", Path(__file__).with_name("review-worker.py")
            )
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            worker = module.Reviewer(workbench)
        print(f"http://127.0.0.1:{httpd.server_port}/#token={token}", flush=True)
        httpd.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        try:
            if worker is not None:
                worker.close()
        finally:
            httpd.server_close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "command", choices=["serve", "inspect", "feedback", "wait", "basis"]
    )
    parser.add_argument("--bundle", required=True)
    parser.add_argument(
        "--questions",
        required=True,
        help="JSON array: {id, path, prompt}, prepared from the diff",
    )
    parser.add_argument("--file", help="Feedback JSON checked against this session")
    parser.add_argument(
        "--request-id",
        help="request_id from the wait result being answered; stale requests are refused",
    )
    parser.add_argument("--reviewer", choices=["manual", "codex"], default="manual")
    parser.add_argument(
        "--token-file",
        type=Path,
        help="Private access-token file to preserve an existing local URL",
    )
    parser.add_argument(
        "--after",
        type=int,
        default=-1,
        help="Wait for an event newer than this revision",
    )
    parser.add_argument(
        "--for",
        dest="event",
        choices=["submission", "confirmation"],
        default="submission",
        help="wait: what to wake for; a confirmation means the contributor confirmed",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=45,
        help="Wait budget in seconds (0 through 60)",
    )
    parser.add_argument(
        "--port",
        type=int,
        default=DEFAULT_PORT,
        help=f"Loopback port; default {DEFAULT_PORT}. Use 0 only for isolated automated tests.",
    )
    args = parser.parse_args()
    if not 0 <= args.port <= 65535:
        parser.error("--port must be between 0 and 65535")
    try:
        # Evidence and git notes belong to the repository this skill is part
        # of, wherever the command was started from.
        if subprocess.run(
            ["git", "rev-parse", "--git-dir"], capture_output=True
        ).returncode:
            for name in ("bundle", "questions", "file", "token_file"):
                if getattr(args, name):
                    setattr(args, name, str(Path(getattr(args, name)).resolve()))
            os.chdir(Path(__file__).resolve().parents[3])
        workbench = Workbench(args.bundle, args.questions)
        if args.command == "serve":
            workbench.snapshot()
            token = None
            if args.token_file:
                fd = os.open(
                    args.token_file, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
                )
                with os.fdopen(fd, "rb") as source:
                    info = os.fstat(source.fileno())
                    if (
                        not stat.S_ISREG(info.st_mode)
                        or info.st_mode & 0o077
                        or info.st_uid != os.geteuid()
                    ):
                        raise ValueError(
                            "Token file must be a private, owned regular file (mode 0600)"
                        )
                    raw = source.read(257)
                if len(raw) > 256 or not re.fullmatch(
                    rb"[A-Za-z0-9_-]{32,128}", raw.strip()
                ):
                    raise ValueError("Invalid private access token")
                token = raw.strip().decode("ascii")
            server(workbench, args.port, reviewer=args.reviewer, token=token)
        elif args.command == "basis":
            print(json.dumps(workbench.basis(), indent=2, ensure_ascii=False))
        elif args.command == "inspect":
            print(json.dumps(workbench.snapshot(), indent=2))
        elif args.command == "wait":
            if not 0 <= args.timeout <= 60:
                parser.error("--timeout must be between 0 and 60")
            result = workbench.wait(args.after, args.timeout, args.event)
            if result is None:
                print(
                    json.dumps(
                        {
                            "outcome": "pending",
                            "revision": workbench.snapshot()["revision"],
                        }
                    )
                )
                return 2
            print(json.dumps(result, indent=2))
        else:
            if not args.file:
                parser.error("feedback requires --file")
            if not args.request_id:
                parser.error("feedback requires --request-id from the wait result")
            # The agent may refine its reference for the request it answers;
            # the workers never replace one.
            note = load(Path(args.file))
            workbench.check_note(note)
            workbench.feedback(
                note,
                request={"request_id": args.request_id},
                replace=True,
            )
            print(json.dumps(workbench.snapshot(), indent=2))
    except (ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, f"workbench: {error}\n")


def interrupted(_signal, _frame):
    raise KeyboardInterrupt


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, interrupted)
    raise SystemExit(main())
