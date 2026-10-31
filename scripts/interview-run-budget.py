#!/usr/bin/env python3

"""Run budget controller for live interview validation runs.

A live run spends money the moment a candidate joins, so the limits on it have
to be enforced by a process the worker cannot talk out of them. This script is
that process. It owns four things:

* a versioned manifest naming the run, the pinned executables, the wall cap and
  the exact scripted action IDs the run may dispatch;
* a permit service on a Unix socket: the worker asks for each scripted action
  by ID and sequence before dispatching it, and the launcher records the grant
  before it replies, so an action is granted at most once and a lost reply
  still counts as spent;
* a monotonic-time sampler that stops new work before the reserved cleanup
  window, then asks for a normal end only inside the remaining budget;
* a reaper that tracks the process tree this run started and terminates only
  that, never by name and never a shared server.

Hard scopes are the ones enforced here: scripted actions, wall time and
agent-alive exposure. Everything else (model turns, tool calls, HTTP requests,
audio frames, provider invoices) is observed at best and reported as a count
or as "unknown", never as zero it did not measure.

Subcommands: validate, binary, run. Python 3.9 or newer, standard library only.
"""

import argparse
import hashlib
import json
import math
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time

SCHEMA_VERSION = 1
PROTOCOL_VERSION = 1

# The one reserved finalization permit. Canonical action lists exclude it; an
# explicit End in a displayed scenario spends this permit, never a second one.
FINAL_ACTION_ID = "end"
# Actions that put the candidate into the paid room. The executables are
# re-identified immediately before the first one is granted.
JOIN_ACTION_IDS = frozenset({"candidate_join"})

PERMIT_TIMEOUT_S = 1.0
# The join permit re-hashes every pinned executable inside the permit window.
# The default is half the timeout.
MAX_JOIN_HASH_S = 0.5
# What the rest of a join permit costs besides the hash: the request read, one
# fsynced ledger line, the reply write and scheduling under load. An explicit
# --max-join-hash-s may go up to the timeout minus this margin and no further,
# so the hash (<= 0.7 s) plus the margin (0.3 s) still fits the client's 1 s.
JOIN_HANDLING_MARGIN_S = 0.3
MAX_JOIN_HASH_CEILING_S = PERMIT_TIMEOUT_S - JOIN_HANDLING_MARGIN_S
SAMPLE_INTERVAL_MAX_S = 1.0
MIN_CLEANUP_RESERVE_S = 60
REAP_BUDGET_S = 5.0
TELEMETRY_STALE_S = 5.0

HARD_SCOPES = frozenset({"scripted_actions", "wall_time", "agent_alive_exposure"})
OBSERVED_SCOPES = frozenset(
    {"model_turns", "tool_calls", "http_requests", "audio_frames", "restarts"}
)
# Named so a manifest cannot claim a meter that does not exist.
UNMEASURED_SCOPES = frozenset({"provider_invoice"})
# `explicit_end` is the spelling that predates run kinds; it means `manual`
# and is accepted so a manifest without `run_kind` keeps validating. The long
# interview harness (scripts/long-interview-plan.cjs) refuses it.
END_MODES = frozenset(
    {
        "explicit_end",
        "manual",
        "server_deadline",
        "absence",
        "room_closed",
        "terminal_cancel",
        "native_close",
    }
)
RUN_KINDS = frozenset({"smoke", "pilot", "qualification", "experiment", "clip"})

ID_PATTERN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}\Z")
SHA_PATTERN = re.compile(r"^[0-9a-f]{64}\Z")
TOP_LEVEL_KEYS = frozenset(
    {
        "schema_version",
        "run_id",
        "binaries",
        "settings_sha256",
        "fixture_sha256",
        "selected_duration_s",
        "outer_wall_cap_s",
        "cleanup_reserve_s",
        "end_mode",
        "run_kind",
        "duration_min",
        "observation_target_s",
        "experiment_profile_min",
        "max_archive_bytes",
        "actions",
        "action_ceiling",
        "limits",
        "scopes",
        "observed_budgets",
        "named_resources",
    }
)

EXIT_CHILD_FAILED = 1
EXIT_PREFLIGHT = 2
EXIT_INCOMPLETE_BUDGET = 4
EXIT_INCOMPLETE_CLEANUP = 5
EXIT_INCOMPLETE_ARTIFACT = 6

# The owned process tree is sampled on this cadence into a file that is
# appended to as the run goes, so a run cut short at any point still has every
# sample taken before it. Both arms of a comparison run through the same
# sampler. The file is bounded: a sample count and a byte size derived from the
# wall cap and the cadence, and a run that would exceed either is an
# incomplete artifact, never one that silently dropped its oldest samples.
RESOURCE_CADENCE_S = 30.0
RESOURCE_SAMPLES_FILE = "resource-samples.jsonl"
RESOURCE_VERSION = 1
RESOURCE_ARMS = ("candidate", "baseline")
# Rows kept per sample; the totals still cover every process.
MAX_SAMPLED_PROCESSES = 16
# A sample line is trimmed to this before it is written.
MAX_SAMPLE_LINE_BYTES = 4096
# What the controller itself writes into the artifact directory.
CONTROL_FILES_ALLOWANCE_BYTES = 256 * 1024
# What a run may add to the archive beyond the sampler's own worst case: logs,
# the capture, the transcript and the report.
ARCHIVE_EVIDENCE_ALLOWANCE_BYTES = 256 * 1024 * 1024


def max_resource_samples(wall_cap_s, cadence_s=RESOURCE_CADENCE_S):
    """Most samples a run of `wall_cap_s` can take: one per cadence, plus the
    first and the last."""
    return int(wall_cap_s // cadence_s) + 2


def archive_bounds(wall_cap_s, cadence_s=RESOURCE_CADENCE_S):
    """(floor, ceiling) of `max_archive_bytes` for a run of `wall_cap_s`.

    The floor is the sampler's worst case plus the controller's own files, so a
    manifest cannot ask for an archive the required evidence does not fit in.
    The ceiling keeps the bound finite and derived, not a free number.
    """
    floor = (
        max_resource_samples(wall_cap_s, cadence_s) * MAX_SAMPLE_LINE_BYTES
        + CONTROL_FILES_ALLOWANCE_BYTES
    )
    return floor, floor + ARCHIVE_EVIDENCE_ALLOWANCE_BYTES


class ManifestError(Exception):
    """The manifest, or what it pins, is not acceptable. Carries every reason."""

    def __init__(self, errors):
        super().__init__("; ".join(errors))
        self.errors = list(errors)


def is_int(value):
    return isinstance(value, int) and not isinstance(value, bool)


def file_sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def manifest_digest(raw):
    """The identity of a manifest is the hash of the bytes the worker is given."""
    return hashlib.sha256(raw).hexdigest()


def validate_manifest(data, min_reserve=MIN_CLEANUP_RESERVE_S):
    """Every reason `data` is not an acceptable manifest, empty when it is."""
    errors = []
    if not isinstance(data, dict):
        return ["manifest must be a JSON object"]
    for key in sorted(set(data) - TOP_LEVEL_KEYS):
        errors.append(f"unknown field {key!r}")
    if data.get("schema_version") != SCHEMA_VERSION:
        errors.append(f"schema_version must be {SCHEMA_VERSION}")
    run_id = data.get("run_id")
    if not isinstance(run_id, str) or not ID_PATTERN.match(run_id):
        errors.append("run_id must match [A-Za-z0-9][A-Za-z0-9._-]{0,63}")

    for name in ("settings_sha256", "fixture_sha256"):
        value = data.get(name)
        if not isinstance(value, str) or not SHA_PATTERN.match(value):
            errors.append(f"{name} must be 64 lowercase hex digits")

    binaries = data.get("binaries")
    if not isinstance(binaries, dict) or "candidate" not in binaries:
        errors.append("binaries must be an object that pins a candidate")
    else:
        for role, entry in binaries.items():
            if not isinstance(role, str) or not ID_PATTERN.match(role):
                errors.append(f"binaries role {role!r} is not a plain name")
            if (
                not isinstance(entry, dict)
                or set(entry) != {"path", "sha256"}
                or not isinstance(entry["path"], str)
                or not os.path.isabs(entry["path"])
                or not isinstance(entry["sha256"], str)
                or not SHA_PATTERN.match(entry["sha256"])
            ):
                errors.append(f"binaries.{role} needs an absolute path and a sha256")

    cap = data.get("outer_wall_cap_s")
    reserve = data.get("cleanup_reserve_s")
    duration = data.get("selected_duration_s")
    for name, value in (
        ("outer_wall_cap_s", cap),
        ("cleanup_reserve_s", reserve),
        ("selected_duration_s", duration),
    ):
        if not is_int(value) or value <= 0:
            errors.append(f"{name} must be a positive integer")
    if is_int(reserve) and reserve < min_reserve:
        errors.append(f"cleanup_reserve_s must be at least {min_reserve}")
    if all(is_int(v) and v > 0 for v in (cap, reserve, duration)):
        if reserve >= cap:
            errors.append("cleanup_reserve_s must be below outer_wall_cap_s")
        elif duration > cap - reserve:
            errors.append("selected_duration_s must fit before the cleanup reserve")

    if "duration_min" in data and "max_archive_bytes" not in data:
        # A long interview's archive has to be bounded for incomplete_artifact
        # to mean anything; a run with no bound could never report one.
        errors.append(
            "max_archive_bytes must be a positive integer: a manifest with "
            "duration_min needs one"
        )
    if "max_archive_bytes" in data:
        limit = data["max_archive_bytes"]
        if not is_int(limit) or limit <= 0:
            errors.append("max_archive_bytes must be a positive integer")
        elif is_int(cap) and cap > 0:
            floor, ceiling = archive_bounds(cap)
            if limit < floor:
                errors.append(
                    f"max_archive_bytes {limit} is below the {floor} the "
                    f"sampler's worst case and the controller files need for "
                    f"a {cap}s wall cap"
                )
            elif limit > ceiling:
                errors.append(
                    f"max_archive_bytes {limit} is above the {ceiling} derived "
                    f"from a {cap}s wall cap"
                )

    if data.get("end_mode") not in END_MODES:
        errors.append("end_mode must be one of " + ", ".join(sorted(END_MODES)))

    # Shape only. What a run kind may do with a duration, a target or an end
    # mode is judged by scripts/long-interview-plan.cjs, which owns those rules.
    if "run_kind" in data:
        if data["run_kind"] not in RUN_KINDS:
            errors.append("run_kind must be one of " + ", ".join(sorted(RUN_KINDS)))
        elif (data["run_kind"] == "clip") != (data.get("end_mode") == "native_close"):
            errors.append("native_close is the end mode of a clip run, and only of one")
    for name in ("duration_min", "observation_target_s", "experiment_profile_min"):
        if name in data and (not is_int(data[name]) or data[name] <= 0):
            errors.append(f"{name} must be a positive integer")

    actions = data.get("actions")
    if not isinstance(actions, list) or not actions:
        errors.append("actions must be a non-empty list of nonfinal action IDs")
    else:
        seen = set()
        for action in actions:
            if not isinstance(action, str) or not ID_PATTERN.match(action):
                errors.append(f"action ID {action!r} is not a plain ID")
            elif action == FINAL_ACTION_ID:
                errors.append(
                    "the final action is the reserved permit and is not listed"
                )
            elif action in seen:
                errors.append(f"duplicate action ID {action!r}")
            seen.add(action)
        if not JOIN_ACTION_IDS & set(a for a in actions if isinstance(a, str)):
            errors.append(
                "actions must include the join action, which gates the "
                "pinned-binary re-check: " + ", ".join(sorted(JOIN_ACTION_IDS))
            )
        if is_int(data.get("action_ceiling")):
            if data["action_ceiling"] != len(actions) + 1:
                errors.append(
                    "action_ceiling must be the nonfinal action count plus one"
                )
    if not is_int(data.get("action_ceiling")):
        errors.append("action_ceiling must be an integer")

    limits = data.get("limits", {})
    if not isinstance(limits, dict) or set(limits) - {"agent_alive_s"}:
        errors.append("limits may only contain agent_alive_s")
    elif "agent_alive_s" in limits and (
        not is_int(limits["agent_alive_s"]) or limits["agent_alive_s"] <= 0
    ):
        errors.append("limits.agent_alive_s must be a positive integer")

    scopes = data.get("scopes")
    if not isinstance(scopes, dict):
        errors.append("scopes must name how each budget is scoped")
    else:
        for name, kind in scopes.items():
            if kind == "hard" and name not in HARD_SCOPES:
                errors.append(
                    f"scope {name!r} cannot be hard: this controller has no "
                    "counter that enforces it"
                )
            elif kind == "observed" and name not in OBSERVED_SCOPES:
                errors.append(f"scope {name!r} is not an observable counter")
            elif kind == "not_measured" and name not in UNMEASURED_SCOPES:
                errors.append(f"scope {name!r} cannot be declared not_measured")
            elif kind not in ("hard", "observed", "not_measured"):
                errors.append(f"scope {name!r} has unknown kind {kind!r}")
        for required in ("scripted_actions", "wall_time"):
            if scopes.get(required) != "hard":
                errors.append(f"scopes.{required} must be hard")
        if scopes.get("agent_alive_exposure") == "hard" and not (
            isinstance(limits, dict) and "agent_alive_s" in limits
        ):
            errors.append("a hard agent_alive_exposure needs limits.agent_alive_s")

    observed_budgets = data.get("observed_budgets", {})
    if not isinstance(observed_budgets, dict) or any(
        not is_int(v) or v < 0 for v in observed_budgets.values()
    ):
        errors.append("observed_budgets must map names to non-negative integers")

    resources = data.get("named_resources", [])
    if not isinstance(resources, list) or any(
        not isinstance(r, dict)
        or set(r) != {"kind", "name"}
        or not all(isinstance(r[k], str) and ID_PATTERN.match(r[k]) for k in r)
        for r in resources
    ):
        errors.append("named_resources must be a list of {kind, name} IDs")
    return errors


def load_manifest(path, min_reserve=MIN_CLEANUP_RESERVE_S):
    """(manifest, raw bytes, digest); raises ManifestError for anything unusable."""
    try:
        with open(path, "rb") as handle:
            raw = handle.read()
        data = json.loads(raw)
    except (OSError, ValueError) as error:
        raise ManifestError([f"cannot read manifest {path}: {error}"])
    errors = validate_manifest(data, min_reserve)
    if errors:
        raise ManifestError(errors)
    return data, raw, manifest_digest(raw)


def verify_binaries(manifest):
    """Hash every pinned executable; returns the seconds the hashing took."""
    started = time.monotonic()
    errors = []
    for role, entry in manifest["binaries"].items():
        path = entry["path"]
        try:
            actual = file_sha256(path)
        except OSError as error:
            errors.append(f"{role}: cannot read {path}: {error.strerror}")
            continue
        if actual != entry["sha256"]:
            errors.append(f"{role}: sha256 of {path} does not match the manifest")
        elif not os.access(path, os.X_OK):
            errors.append(f"{role}: {path} is not executable")
    if errors:
        raise ManifestError(errors)
    return time.monotonic() - started


# --------------------------------------------------------------------------
# Permit service
# --------------------------------------------------------------------------


class PermitService:
    """Grants each scripted action at most once, and records it before replying.

    Phases: open, draining (only the reserved End is granted), closed. Every
    denial path is fail-closed. A duplicate, unknown, out-of-sequence or
    malformed request is a fault: it is denied and moves the run to draining,
    so a worker that has lost track of its own count cannot spend more.
    """

    def __init__(
        self,
        manifest,
        digest,
        ledger_path,
        new_work_deadline=None,
        clock=time.monotonic,
        verify_join_binaries=False,
        on_fault=None,
        join_hash_limit_s=MAX_JOIN_HASH_S,
        hasher=None,
    ):
        self.manifest = manifest
        self.digest = digest
        self.run_id = manifest["run_id"]
        self.allowed = set(manifest["actions"]) | {FINAL_ACTION_ID}
        self.ceiling = manifest["action_ceiling"]
        self.ledger_path = ledger_path
        self.new_work_deadline = new_work_deadline
        self.clock = clock
        self.verify_join_binaries = verify_join_binaries
        self.on_fault = on_fault
        self.join_hash_limit_s = join_hash_limit_s
        self.hasher = hasher or file_sha256
        self.lock = threading.Lock()
        self.granted = {}
        self.phase = "open"
        self.phase_reason = None
        self.fault = None
        self.denials = []
        self.joined = False
        self._server = None
        self._thread = None
        self._replay()

    # -- ledger -----------------------------------------------------------

    def _replay(self):
        """Counts survive a launcher restart because the ledger is the truth."""
        if not os.path.exists(self.ledger_path):
            return
        with open(self.ledger_path) as handle:
            for line in handle:
                try:
                    event = json.loads(line)
                except ValueError:
                    continue
                if event.get("event") == "grant":
                    self.granted[event["action_id"]] = event["seq"]
                elif event.get("event") == "fault":
                    self.fault = event.get("reason")
                    self.phase = "draining"
                    self.phase_reason = self.fault

    def _record(self, event):
        line = json.dumps(event, sort_keys=True) + "\n"
        fd = os.open(self.ledger_path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
        try:
            os.write(fd, line.encode())
            os.fsync(fd)
        finally:
            os.close(fd)

    # -- phases -----------------------------------------------------------

    def drain(self, reason):
        with self.lock:
            if self.phase == "open":
                self.phase = "draining"
                self.phase_reason = reason
                self._record({"event": "drain", "reason": reason})

    def close(self, reason):
        with self.lock:
            if self.phase != "closed":
                self.phase = "closed"
                self.phase_reason = reason
                self._record({"event": "close", "reason": reason})

    def count(self):
        with self.lock:
            return len(self.granted)

    # -- decisions --------------------------------------------------------

    def _fault(self, reason):
        # Called with the lock held.
        if self.fault is None:
            self.fault = reason
        self.denials.append(reason)
        self._record({"event": "fault", "reason": reason})
        if self.phase == "open":
            self.phase = "draining"
            self.phase_reason = reason
        if self.on_fault:
            self.on_fault(reason)
        return {"v": PROTOCOL_VERSION, "granted": False, "reason": reason}

    def decide(self, request):
        """The reply to one request. The grant is durable before it is returned."""
        with self.lock:
            return self._decide(request)

    def _decide(self, request):
        if (
            not isinstance(request, dict)
            or request.get("v") != PROTOCOL_VERSION
            or request.get("op") not in ("permit", "status", "ping")
        ):
            return self._fault("malformed_request")
        if (
            request.get("run_id") != self.run_id
            or request.get("manifest_sha256") != self.digest
        ):
            return self._fault("run_identity_mismatch")
        op = request["op"]
        if op == "ping":
            return {"v": PROTOCOL_VERSION, "ok": True, "phase": self.phase}
        if op == "status":
            return {
                "v": PROTOCOL_VERSION,
                "ok": True,
                "phase": self.phase,
                "count": len(self.granted),
                "granted": sorted(self.granted, key=self.granted.get),
            }
        action_id = request.get("action_id")
        seq = request.get("seq")
        if not isinstance(action_id, str) or not is_int(seq):
            return self._fault("malformed_request")
        if action_id not in self.allowed:
            return self._fault("unknown_action")
        if action_id in self.granted:
            return self._fault("duplicate_action")
        if seq != len(self.granted) + 1 or seq > self.ceiling:
            return self._fault("sequence_mismatch")
        if (
            self.phase == "open"
            and self.new_work_deadline is not None
            and self.clock() >= self.new_work_deadline
        ):
            # Checked here as well as by the sampler, so a stalled sampler
            # cannot let new work start inside the cleanup window.
            self.phase = "draining"
            self.phase_reason = "wall_budget"
            self._record({"event": "drain", "reason": "wall_budget"})
        if self.phase == "closed" or (
            self.phase == "draining" and action_id != FINAL_ACTION_ID
        ):
            self.denials.append("not_open:" + self.phase)
            return {"v": PROTOCOL_VERSION, "granted": False, "reason": self.phase}
        if action_id in JOIN_ACTION_IDS and self.verify_join_binaries:
            # Re-hashed, not just re-stat'ed: a rewrite of the same length
            # inside one clock tick keeps the inode, size and mtime. Each
            # binary is hashed once here. The whole hash is timed: one over
            # the limit is a fault, never a grant, because a client that
            # already gave up at the permit timeout would otherwise leave a
            # grant it never saw.
            began = self.clock()
            try:
                for role, entry in self.manifest["binaries"].items():
                    if self.hasher(entry["path"]) != entry["sha256"]:
                        return self._fault("binary_changed:" + role)
                    if self.clock() - began > self.join_hash_limit_s:
                        # No point hashing the rest of a pin set already over.
                        return self._fault("join_hash_too_slow")
            except OSError:
                return self._fault("binary_missing")
        self.granted[action_id] = seq
        self._record({"event": "grant", "action_id": action_id, "seq": seq})
        return {
            "v": PROTOCOL_VERSION,
            "granted": True,
            "action_id": action_id,
            "seq": seq,
        }

    # -- socket -----------------------------------------------------------

    def start(self, socket_path):
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(socket_path)
        os.chmod(socket_path, 0o600)
        server.listen(16)
        server.settimeout(0.2)
        self._server = server
        self._thread = threading.Thread(target=self._accept, daemon=True)
        self._thread.start()

    def stop(self):
        server, self._server = self._server, None
        if server is not None:
            server.close()
        if self._thread:
            self._thread.join(2)

    def _accept(self):
        server = self._server
        while self._server is not None:
            try:
                conn, _ = server.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            threading.Thread(target=self._serve, args=(conn,), daemon=True).start()

    def _serve(self, conn):
        deadline = time.monotonic() + PERMIT_TIMEOUT_S
        data = b""
        try:
            while b"\n" not in data and len(data) < 4096:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise socket.timeout
                conn.settimeout(remaining)
                block = conn.recv(4096)
                if not block:
                    break
                data += block
            if b"\n" not in data:
                # Silence is dropped; a started but unfinished request is not.
                if data:
                    with self.lock:
                        self._fault("malformed_request")
                return
            try:
                request = json.loads(data.split(b"\n", 1)[0])
            except ValueError:
                request = None
            reply = self.decide(request)
            conn.settimeout(PERMIT_TIMEOUT_S)
            conn.sendall(json.dumps(reply).encode() + b"\n")
        except socket.timeout:
            if data:
                with self.lock:
                    self._fault("malformed_request")
        except OSError:
            pass
        finally:
            conn.close()


# --------------------------------------------------------------------------
# Worker side
# --------------------------------------------------------------------------


class PermitDenied(Exception):
    """No permit: the worker must not dispatch this action."""


class LauncherLost(PermitDenied):
    """The launcher cannot be reached: stop new work and clean up."""


def exchange(socket_path, request, timeout=PERMIT_TIMEOUT_S):
    """One request, one reply, one connection, one deadline. Never retried."""
    deadline = time.monotonic() + timeout
    try:
        conn = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            conn.settimeout(timeout)
            conn.connect(socket_path)
            conn.sendall(json.dumps(request).encode() + b"\n")
            data = b""
            while b"\n" not in data and len(data) < 4096:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise socket.timeout
                conn.settimeout(remaining)
                block = conn.recv(4096)
                if not block:
                    raise LauncherLost("connection closed before a reply")
                data += block
        finally:
            conn.close()
    except socket.timeout:
        raise LauncherLost("no reply within the permit timeout")
    except OSError as error:
        raise LauncherLost(f"launcher unreachable: {error.strerror}")
    try:
        reply = json.loads(data.split(b"\n", 1)[0])
    except ValueError:
        raise PermitDenied("malformed reply")
    if not isinstance(reply, dict) or reply.get("v") != PROTOCOL_VERSION:
        raise PermitDenied("malformed reply")
    return reply


class WorkerGuard:
    """What a worker holds: the only way it is allowed to start an action.

    Any failure leaves the guard dead, because a request whose reply was lost
    may have been granted, and dispatching on a guess is how a budget is
    overrun. The next process to join the run asks the launcher for its count.
    """

    def __init__(self, socket_path, run_id, manifest_sha256, timeout=PERMIT_TIMEOUT_S):
        self.socket_path = socket_path
        self.run_id = run_id
        self.manifest_sha256 = manifest_sha256
        self.timeout = timeout
        self.dead = None
        self._lock = threading.Lock()
        status = self._call({"op": "status"})
        if not status.get("ok") or not is_int(status.get("count")):
            self.dead = "unusable status reply"
            raise PermitDenied(self.dead)
        self.next_seq = status["count"] + 1

    @classmethod
    def from_env(cls, env=None):
        env = os.environ if env is None else env
        path = env.get("BROWSER_CHECK_RUN_MANIFEST")
        with open(path, "rb") as handle:
            raw = handle.read()
        if manifest_digest(raw) != env.get("BROWSER_CHECK_RUN_MANIFEST_SHA256"):
            raise PermitDenied("manifest hash does not match the launcher's")
        run_id = json.loads(raw)["run_id"]
        if run_id != env.get("BROWSER_CHECK_RUN_ID"):
            raise PermitDenied("run ID does not match the manifest")
        return cls(env["BROWSER_CHECK_BUDGET_SOCKET"], run_id, manifest_digest(raw))

    def _call(self, request):
        request = dict(request, v=PROTOCOL_VERSION, run_id=self.run_id)
        request["manifest_sha256"] = self.manifest_sha256
        return exchange(self.socket_path, request, self.timeout)

    def ping(self):
        return self._call({"op": "ping"}).get("ok") is True

    def permit(self, action_id):
        with self._lock:
            if self.dead:
                raise PermitDenied("guard is dead: " + self.dead)
            seq = self.next_seq
            try:
                reply = self._call({"op": "permit", "action_id": action_id, "seq": seq})
                if reply.get("granted") is not True:
                    raise PermitDenied(str(reply.get("reason", "denied")))
                if reply.get("action_id") != action_id or reply.get("seq") != seq:
                    raise PermitDenied("reply does not match the request")
            except PermitDenied as error:
                self.dead = str(error)
                raise
            self.next_seq = seq + 1

    def run_action(self, action_id, dispatch):
        """The permit comes first; nothing is dispatched on a denial."""
        self.permit(action_id)
        return dispatch()

    def watch_launcher(self, on_lost, interval=0.25):
        """Call `on_lost` once if the launcher goes away; returns a stop event."""
        stop = threading.Event()
        parent = os.getppid()

        def watch():
            while not stop.wait(interval):
                lost = os.getppid() != parent
                if not lost:
                    try:
                        lost = not self.ping()
                    except PermitDenied:
                        lost = True
                if lost:
                    self.dead = self.dead or "launcher lost"
                    on_lost()
                    return

        threading.Thread(target=watch, daemon=True).start()
        return stop


def bounded_cleanup(cleanup, bound_s):
    """Run the worker's own cleanup for at most `bound_s`; True if it finished."""
    thread = threading.Thread(target=cleanup, daemon=True)
    thread.start()
    thread.join(bound_s)
    return not thread.is_alive()


# --------------------------------------------------------------------------
# Process tree
# --------------------------------------------------------------------------


def parse_proc_stat(text):
    """(pid, state, ppid, pgid, start) from /proc/<pid>/stat."""
    head, _, tail = text.rpartition(")")
    pid = int(head.split(" ", 1)[0])
    fields = tail.split()
    return pid, fields[0], int(fields[1]), int(fields[2]), int(fields[19])


def parse_ps(text):
    """Rows of `ps -A -o pid= -o ppid= -o pgid= -o stat= -o lstart=` in the same shape.

    The start key is the printed start time, whole seconds, so a recycled PID
    compares unequal unless it was reused within the same second. A row
    without one keys as 0, which matches nothing it should not.
    """
    table = {}
    for line in text.splitlines():
        parts = line.split()
        if len(parts) >= 4:
            start = " ".join(parts[4:9]) if len(parts) >= 9 else 0
            table[int(parts[0])] = (parts[3][0], int(parts[1]), int(parts[2]), start)
    return table


def process_table():
    """{pid: (state, ppid, pgid, start)} for every process visible to us."""
    table = {}
    if os.path.isdir("/proc/self"):
        for name in os.listdir("/proc"):
            if not name.isdigit():
                continue
            try:
                with open(f"/proc/{name}/stat") as handle:
                    pid, state, ppid, pgid, start = parse_proc_stat(handle.read())
            except (OSError, ValueError, IndexError):
                continue
            table[pid] = (state, ppid, pgid, start)
        return table
    out = subprocess.run(
        [
            "ps",
            "-A",
            "-o",
            "pid=",
            "-o",
            "ppid=",
            "-o",
            "pgid=",
            "-o",
            "stat=",
            "-o",
            "lstart=",
        ],
        capture_output=True,
        text=True,
        check=False,
    ).stdout
    return parse_ps(out)


def process_table_available():
    if os.path.isdir("/proc/self"):
        return True
    return shutil.which("ps") is not None


class OwnedTree:
    """Every process this run started, accumulated at each sample.

    Membership: the root, anything below an owned process, and anything in the
    root's process group. It is accumulated because an orphan is reparented and
    no longer descends from anything we hold; on Linux the launcher also marks
    itself a child subreaper so an orphan reparents to it and stays in the tree.
    The start time guards against signalling a recycled PID.
    """

    def __init__(self, root_pid, root_pgid, adopt_orphans=False):
        self.adopt_orphans = adopt_orphans
        self.root_pid = root_pid
        self.root_pgid = root_pgid
        self.owned = {}
        self.root_enrolled = False

    def refresh(self, table=None):
        table = process_table() if table is None else table
        self.owned = {
            pid: start
            for pid, start in self.owned.items()
            if pid in table and table[pid][3] == start
        }
        # Enrolled once: after the worker is reaped its PID can be recycled,
        # and a second enrollment would take the stranger's start time as ours.
        if self.root_pid in table and not self.root_enrolled:
            self.owned[self.root_pid] = table[self.root_pid][3]
            self.root_enrolled = True
        changed = True
        while changed:
            changed = False
            for pid, (_, ppid, pgid, start) in table.items():
                if pid in self.owned or pid == os.getpid():
                    continue
                if (
                    (ppid in self.owned and table[ppid][3] == self.owned[ppid])
                    or pgid == self.root_pgid
                    # An orphan reparented to us by the subreaper mark.
                    or (self.adopt_orphans and ppid == os.getpid())
                ):
                    self.owned[pid] = start
                    changed = True
        return table

    def alive(self, table):
        """Owned processes that are still running; a zombie is already dead."""
        return sorted(
            pid
            for pid, start in self.owned.items()
            if pid in table and table[pid][3] == start and table[pid][0] != "Z"
        )


def reap_owned(tree, budget_s, send=os.kill, clock=time.monotonic):
    """TERM, then KILL, only the processes `tree` owns; never a name or a global.

    Returns (elapsed, survivors). The budget splits 40 percent for TERM to be
    honored and the rest for KILL to land.
    """
    started = clock()
    own_group = os.getpgrp()

    def signal_all(sig, pids):
        for pid in pids:
            try:
                send(pid, sig)
            except (ProcessLookupError, PermissionError):
                pass

    def wait(until):
        alive = []
        while clock() < until:
            table = process_table()
            for pid in list(tree.owned):
                if pid != tree.root_pid and pid in table and table[pid][0] == "Z":
                    try:
                        os.waitpid(pid, os.WNOHANG)
                    except OSError:
                        pass
            alive = tree.alive(table)
            if not alive:
                return []
            time.sleep(0.02)
        return tree.alive(process_table())

    def group_is_ours(table):
        # The group ID is only ours while an owned process still sits in it;
        # once the last one is gone the number may belong to someone else.
        return (
            tree.root_pgid != own_group
            and tree.root_pgid > 1
            and any(table[p][2] == tree.root_pgid for p in tree.alive(table))
        )

    tree.refresh()
    table = process_table()
    targets = [p for p in tree.alive(table) if p != os.getpid()]
    # Group-wide first, so a child holding no shell keeps nothing alive. The
    # root's group is ours by construction. Individual PIDs cover the rest.
    if group_is_ours(table):
        try:
            send(-tree.root_pgid, signal.SIGTERM)
        except (ProcessLookupError, PermissionError):
            pass
    signal_all(signal.SIGTERM, targets)
    survivors = wait(started + budget_s * 0.4)
    if survivors:
        tree.refresh()
        if group_is_ours(process_table()):
            try:
                send(-tree.root_pgid, signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                pass
        signal_all(
            signal.SIGKILL, [p for p in tree.alive(process_table()) if p != os.getpid()]
        )
        survivors = wait(started + budget_s)
    return clock() - started, survivors


def become_subreaper():
    """Best effort: orphans of our children reparent to us instead of init."""
    if not sys.platform.startswith("linux"):
        return False
    try:
        import ctypes

        libc = ctypes.CDLL(None, use_errno=True)
        return libc.prctl(36, 1, 0, 0, 0) == 0  # PR_SET_CHILD_SUBREAPER
    except (OSError, AttributeError):
        return False


# --------------------------------------------------------------------------
# Launcher
# --------------------------------------------------------------------------


class Telemetry:
    """Tails the worker's JSONL telemetry; absence is reported, never zeroed."""

    def __init__(self, path):
        self.path = path
        self.offset = 0
        self.partial = b""
        self.last_heartbeat = None
        self.agent_alive = False
        self.observed = {}
        self.dispatched = []
        self.malformed = 0
        self.events = 0

    def poll(self, now):
        try:
            with open(self.path, "rb") as handle:
                handle.seek(self.offset)
                block = handle.read()
        except OSError:
            return []
        self.offset += len(block)
        data = self.partial + block
        *lines, self.partial = data.split(b"\n")
        events = []
        for line in lines:
            if not line.strip():
                continue
            try:
                event = json.loads(line)
                kind = event["event"]
            except (ValueError, KeyError, TypeError):
                self.malformed += 1
                continue
            self.events += 1
            if kind == "hb":
                self.last_heartbeat = now
            elif kind == "agent_alive" and isinstance(event.get("value"), bool):
                self.last_heartbeat = now
                events.append(("agent_alive", event["value"]))
                self.agent_alive = event["value"]
            elif kind == "observed" and is_int(event.get("value")):
                self.observed[str(event.get("name"))] = event["value"]
            elif kind == "dispatched" and isinstance(event.get("action_id"), str):
                self.dispatched.append(event["action_id"])
            else:
                self.malformed += 1
        return events


def read_process_resources(pid, start, proc_root="/proc"):
    """{rss_kb, fds, threads, reused} for one process; a value is None when it
    could not be read, and a PID whose start time no longer matches `start` is
    another process, so every value of it is None and `reused` is true.

    The start time is checked after the reads as well as before, so a process
    that exited and whose number was reused between two reads is not counted
    as the one this run started.
    """
    unknown = {"rss_kb": None, "fds": None, "threads": None, "reused": False}
    base = f"{proc_root}/{pid}"

    def started():
        try:
            with open(f"{base}/stat") as handle:
                return parse_proc_stat(handle.read())[4]
        except (OSError, ValueError, IndexError):
            return None

    before = started()
    if before is None:
        return dict(unknown)
    if before != start:
        return {**unknown, "reused": True}
    out = dict(unknown)
    try:
        with open(f"{base}/status") as handle:
            for line in handle:
                if line.startswith("VmRSS:"):
                    out["rss_kb"] = int(line.split()[1])
                elif line.startswith("Threads:"):
                    out["threads"] = int(line.split()[1])
    except (OSError, ValueError, IndexError):
        out["rss_kb"] = out["threads"] = None
    try:
        out["fds"] = len(os.listdir(f"{base}/fd"))
    except OSError:
        out["fds"] = None
    if started() != start:
        return {**unknown, "reused": True}
    return out


class ResourceSampler:
    """Appends one JSON line per sample of the owned process tree.

    Every value a sample could not read is `null`: unknown, never zero. A tree
    total is the sum of the values that were read, with `*_unknown` counting the
    processes that did not give one, and is `null` when none did. Processes are
    keyed by PID and start time, so a reused PID is another process.
    """

    def __init__(
        self,
        path,
        arm,
        cadence_s=RESOURCE_CADENCE_S,
        max_samples=None,
        max_bytes=None,
        proc_root="/proc",
        wall_cap_s=None,
    ):
        self.path = path
        self.arm = arm
        self.cadence_s = cadence_s
        if max_samples is None:
            max_samples = max_resource_samples(wall_cap_s or 0, cadence_s)
        self.max_samples = max_samples
        self.max_bytes = (
            max_bytes if max_bytes is not None else max_samples * MAX_SAMPLE_LINE_BYTES
        )
        self.proc_root = proc_root
        self.samples = 0
        self.bytes = 0
        self.overflow = None
        self.next_due = 0.0
        self.last_at = None

    def due(self, elapsed):
        return self.overflow is None and elapsed >= self.next_due

    def sample(self, elapsed, tree, table, unix_ms=None):
        """Takes one sample; returns the record, or None once the bound is hit."""
        if self.overflow is not None:
            return None
        if self.samples >= self.max_samples:
            self.overflow = "max_samples"
            return None
        alive = tree.alive(table)
        rows = []
        totals = {"rss_kb": 0, "fds": 0, "threads": 0}
        known = {name: 0 for name in totals}
        for pid in alive:
            start = tree.owned[pid]
            values = read_process_resources(pid, start, self.proc_root)
            rows.append({"pid": pid, "start": start, **values})
            for name in totals:
                if values[name] is not None:
                    totals[name] += values[name]
                    known[name] += 1
        record = {
            "event": "resource",
            "version": RESOURCE_VERSION,
            "arm": self.arm,
            "seq": self.samples,
            "elapsed_s": round(elapsed, 3),
            "gap_s": (
                None if self.last_at is None else round(elapsed - self.last_at, 3)
            ),
            "unix_ms": unix_ms if unix_ms is not None else int(time.time() * 1000),
            "processes": len(alive),
            "tree": {name: (totals[name] if known[name] else None) for name in totals},
            "tree_unknown": {name: len(alive) - known[name] for name in totals},
            "rows": rows[:MAX_SAMPLED_PROCESSES],
            "rows_omitted": max(0, len(rows) - MAX_SAMPLED_PROCESSES),
        }
        line = json.dumps(record, sort_keys=True, separators=(",", ":"))
        while len(line) + 1 > MAX_SAMPLE_LINE_BYTES and record["rows"]:
            record["rows_omitted"] += 1
            record["rows"].pop()
            line = json.dumps(record, sort_keys=True, separators=(",", ":"))
        if self.bytes + len(line) + 1 > self.max_bytes:
            self.overflow = "max_bytes"
            return None
        with open(self.path, "a") as handle:
            handle.write(line + "\n")
            handle.flush()
            os.fsync(handle.fileno())
        self.samples += 1
        self.bytes += len(line) + 1
        self.last_at = elapsed
        # Late ticks do not accumulate a backlog: the next is one cadence on.
        self.next_due = elapsed + self.cadence_s
        return record


def archive_bytes(directory):
    """Total size of the files under `directory`; unreadable ones count 0."""
    total = 0
    for root, _, names in os.walk(directory):
        for name in names:
            try:
                total += os.lstat(os.path.join(root, name)).st_size
            except OSError:
                pass
    return total


def write_json(path, value):
    tmp = path + ".tmp"
    with open(tmp, "w") as handle:
        json.dump(value, handle, indent=2, sort_keys=True)
        handle.write("\n")
    os.replace(tmp, path)


def join_hash_limit_error(value):
    """Why `value` is not an acceptable --max-join-hash-s, or None."""
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        return "--max-join-hash-s must be a number"
    if not math.isfinite(value) or value <= 0:
        return "--max-join-hash-s must be a positive, finite number of seconds"
    if value > MAX_JOIN_HASH_CEILING_S:
        return (
            f"--max-join-hash-s {value} is over the {MAX_JOIN_HASH_CEILING_S:.2f}s "
            f"ceiling ({PERMIT_TIMEOUT_S}s permit timeout minus the "
            f"{JOIN_HANDLING_MARGIN_S}s the rest of the join permit needs)"
        )
    return None


def preflight(manifest, args, resource_cleaner, max_hash_s=None, verify=None):
    """Every reason this run cannot be enforced; empty means it may start."""
    errors = []
    if max_hash_s is None:
        max_hash_s = getattr(args, "max_join_hash_s", MAX_JOIN_HASH_S)
        limit_error = join_hash_limit_error(max_hash_s)
        if limit_error:
            errors.append(limit_error)
            max_hash_s = MAX_JOIN_HASH_S
    try:
        hash_seconds = (verify or verify_binaries)(manifest)
        if hash_seconds > max_hash_s:
            errors.append(
                f"pinned binaries take {hash_seconds:.2f}s to hash, over the "
                f"{max_hash_s}s the join permit can spend re-checking them"
            )
    except ManifestError as error:
        errors.extend(error.errors)
    if not process_table_available():
        errors.append(
            "no process table (/proc or ps): the owned tree cannot be tracked"
        )
    if not hasattr(os, "setsid") or not hasattr(socket, "AF_UNIX"):
        errors.append("this platform has no process groups or Unix sockets")
    if manifest.get("named_resources") and not resource_cleaner:
        errors.append(
            "named_resources are declared and no --resource-cleaner closes them"
        )
    if args.sample_interval > SAMPLE_INTERVAL_MAX_S or args.sample_interval <= 0:
        errors.append("--sample-interval must be above 0 and at most 1 second")
    if args.resource_cadence <= 0:
        errors.append("--resource-cadence must be positive")
    if args.reap_budget <= 0:
        errors.append("--reap-budget must be positive")
    passes = 2 if runs_cleaner(manifest, resource_cleaner) else 1
    if args.reap_budget * passes >= manifest["cleanup_reserve_s"]:
        errors.append(
            "--reap-budget must fit inside cleanup_reserve_s"
            + (
                ", once for the reap and once for the resource cleaner"
                if passes == 2
                else ""
            )
        )
    return errors


def runs_cleaner(manifest, cleaner):
    """Whether the run will close named resources, so the wall cap holds back
    time for it and the cleanup pass is made."""
    return bool(cleaner and manifest.get("named_resources"))


def run_cleaner(cleaner, resources, timeout):
    """Close only the manifest's named resources; returns the failures.

    `timeout` bounds the whole pass, not each resource, so the hard wall cap
    holds however many resources the manifest names.
    """
    failures = []
    deadline = time.monotonic() + timeout
    for resource in resources:
        left = deadline - time.monotonic()
        if left <= 0:
            failures.append({**resource, "reason": "timeout"})
            continue
        try:
            result = subprocess.run(
                [cleaner, resource["kind"], resource["name"]],
                stdin=subprocess.DEVNULL,
                capture_output=True,
                timeout=left,
                check=False,
            )
            if result.returncode != 0:
                failures.append({**resource, "reason": f"exit {result.returncode}"})
        except subprocess.TimeoutExpired:
            failures.append({**resource, "reason": "timeout"})
        except OSError as error:
            failures.append({**resource, "reason": error.strerror})
    return failures


def launch(args, command):
    """Run `command` under the budget; returns the process exit status."""
    artifacts = os.path.abspath(args.artifact_dir)
    os.makedirs(artifacts, exist_ok=True)
    stale_refusal = os.path.join(artifacts, "preflight_failed.json")
    if os.path.exists(stale_refusal):
        os.unlink(stale_refusal)
    min_reserve = (
        args.allow_short_reserve if args.allow_short_reserve else MIN_CLEANUP_RESERVE_S
    )
    try:
        manifest, raw, digest = load_manifest(args.manifest, min_reserve)
    except ManifestError as error:
        write_json(
            os.path.join(artifacts, "preflight_failed.json"),
            {"status": "preflight_failed", "errors": error.errors},
        )
        print("preflight failed: " + "; ".join(error.errors), file=sys.stderr)
        return EXIT_PREFLIGHT
    errors = preflight(manifest, args, args.resource_cleaner)
    if errors:
        write_json(
            os.path.join(artifacts, "preflight_failed.json"),
            {
                "status": "preflight_failed",
                "run_id": manifest["run_id"],
                "errors": errors,
            },
        )
        print("preflight failed: " + "; ".join(errors), file=sys.stderr)
        return EXIT_PREFLIGHT

    existing = [
        name
        for name in (
            "budget-ledger.jsonl",
            "budget-result.json",
            "incomplete_budget.json",
            "incomplete_cleanup.json",
            "incomplete_artifact.json",
            RESOURCE_SAMPLES_FILE,
        )
        if os.path.exists(os.path.join(artifacts, name))
    ]
    if existing:
        # A rerun here would start from a fresh count and mix two runs'
        # verdicts, so each run gets an empty directory.
        message = "artifact directory already holds a run: " + ", ".join(existing)
        write_json(
            os.path.join(artifacts, "preflight_failed.json"),
            {
                "status": "preflight_failed",
                "run_id": manifest["run_id"],
                "errors": [message],
            },
        )
        print("preflight failed: " + message, file=sys.stderr)
        return EXIT_PREFLIGHT

    manifest_copy = os.path.join(artifacts, "run-manifest.json")
    with open(manifest_copy, "wb") as handle:
        handle.write(raw)
    ledger = os.path.join(artifacts, "budget-ledger.jsonl")
    telemetry_path = os.path.join(artifacts, "budget-telemetry.jsonl")
    stop_file = os.path.join(artifacts, "budget-stop")
    open(telemetry_path, "a").close()

    cap = manifest["outer_wall_cap_s"]
    reserve = manifest["cleanup_reserve_s"]
    stop_new_work = cap - reserve
    # The resource cleaner runs after the reap, so its budget is held back
    # too: the cap covers both passes.
    cleaner_budget = (
        args.reap_budget if runs_cleaner(manifest, args.resource_cleaner) else 0
    )
    reap_start = cap - args.reap_budget - cleaner_budget
    scopes = manifest["scopes"]
    exposure_hard = scopes.get("agent_alive_exposure") == "hard"
    exposure_limit = manifest.get("limits", {}).get("agent_alive_s")

    subreaper = become_subreaper()
    socket_dir = tempfile.mkdtemp(prefix="irb-")
    os.chmod(socket_dir, 0o700)
    socket_path = os.path.join(socket_dir, "permit.sock")
    t0 = time.monotonic()
    faults = []
    service = PermitService(
        manifest,
        digest,
        ledger,
        new_work_deadline=t0 + stop_new_work,
        verify_join_binaries=True,
        on_fault=faults.append,
        join_hash_limit_s=args.max_join_hash_s,
    )
    service.start(socket_path)

    env = dict(os.environ)
    env.update(
        BROWSER_CHECK_RUN_MANIFEST=manifest_copy,
        BROWSER_CHECK_RUN_ID=manifest["run_id"],
        BROWSER_CHECK_RUN_MANIFEST_SHA256=digest,
        BROWSER_CHECK_BUDGET_SOCKET=socket_path,
        BROWSER_CHECK_BUDGET_TELEMETRY=telemetry_path,
        BROWSER_CHECK_BUDGET_STOP=stop_file,
    )
    # A launcher that is told to stop must stop the worker with it: the worker
    # sits in its own session and would otherwise keep spending.
    interrupted = []
    previous_handlers = {}
    for name in ("SIGTERM", "SIGINT", "SIGHUP"):
        signum = getattr(signal, name, None)
        if signum is not None:
            previous_handlers[signum] = signal.signal(
                signum, lambda received, frame: interrupted.append(received)
            )

    try:
        child = subprocess.Popen(
            command, env=env, stdin=subprocess.DEVNULL, start_new_session=True
        )
    except OSError as error:
        for signum, handler in previous_handlers.items():
            signal.signal(signum, handler)
        service.stop()
        shutil.rmtree(socket_dir, ignore_errors=True)
        message = f"cannot start the worker: {error.strerror}"
        write_json(
            os.path.join(artifacts, "preflight_failed.json"),
            {
                "status": "preflight_failed",
                "run_id": manifest["run_id"],
                "errors": [message],
            },
        )
        print("preflight failed: " + message, file=sys.stderr)
        return EXIT_PREFLIGHT
    tree = OwnedTree(child.pid, child.pid, adopt_orphans=subreaper)
    telemetry = Telemetry(telemetry_path)
    max_archive = manifest.get("max_archive_bytes")
    sampler = ResourceSampler(
        os.path.join(artifacts, RESOURCE_SAMPLES_FILE),
        args.arm,
        cadence_s=args.resource_cadence,
        wall_cap_s=cap,
    )
    artifact_failure = None

    failure = None  # first budget failure reason
    exposure = 0.0
    alive_since = None
    previous_tick = t0
    max_gap = 0.0
    reap_elapsed = 0.0
    survivors = []
    leaked = 0
    reaped = False
    child_status = None

    stopped = []

    def stop_run(reason):
        if not stopped:
            stopped.append(reason)
            service.drain(reason)
            try:
                open(stop_file, "w").close()
            except OSError:
                pass

    def trigger(reason):
        nonlocal failure
        if failure is None:
            failure = reason
            stop_run(reason)

    def artifact_trigger(reason):
        # The evidence cannot be kept whole, so the run stops spending on it.
        # It keeps its own record, so a budget failure that follows is still
        # the first budget failure and not lost behind this one.
        nonlocal artifact_failure
        if artifact_failure is None:
            artifact_failure = reason
            stop_run("artifact:" + reason)

    try:
        while True:
            now = time.monotonic()
            elapsed = now - t0
            max_gap = max(max_gap, now - previous_tick)
            table = tree.refresh()
            if sampler.due(elapsed):
                sampler.sample(elapsed, tree, table)
            if sampler.overflow:
                artifact_trigger("resource_samples_" + sampler.overflow)
            if max_archive is not None and archive_bytes(artifacts) > max_archive:
                artifact_trigger("archive_bytes")
            for kind, value in telemetry.poll(now):
                if kind == "agent_alive":
                    if value and alive_since is None:
                        # From the previous tick: over-counts rather than misses.
                        alive_since = previous_tick
                    elif not value and alive_since is not None:
                        exposure += now - alive_since
                        alive_since = None
            live_exposure = exposure + (
                now - alive_since if alive_since is not None else 0.0
            )
            previous_tick = now

            for dispatched in telemetry.dispatched:
                if dispatched not in service.granted:
                    trigger("dispatch_without_permit")
            if interrupted:
                trigger("launcher_signal:" + signal.Signals(interrupted[0]).name)
                break
            if faults:
                trigger("permit_fault:" + faults[0])
            child_status = child.poll()
            if child_status is not None:
                break
            if elapsed >= stop_new_work:
                trigger("wall_budget")
            if exposure_hard and live_exposure >= exposure_limit:
                trigger("agent_exposure")
            if exposure_hard:
                silent_since = telemetry.last_heartbeat
                if (
                    now - (silent_since if silent_since is not None else t0)
                ) > args.telemetry_stale:
                    trigger("telemetry_lost")
            if elapsed >= reap_start:
                trigger("wall_budget")
                break
            time.sleep(min(args.sample_interval, max(0.0, reap_start - elapsed)))

        # Worker gone or out of time: close the permit service, then reap what
        # this run still owns, including anything the worker leaked.
        telemetry.poll(time.monotonic())
        # What arrived since the last tick still counts: a worker can log an
        # ungranted dispatch or send a bad request and exit between two ticks.
        for dispatched in telemetry.dispatched:
            if dispatched not in service.granted:
                trigger("dispatch_without_permit")
        if faults:
            trigger("permit_fault:" + faults[0])
        if alive_since is not None:
            exposure += time.monotonic() - alive_since
        service.close("reaping")
        tree.refresh()
        leaked = len([p for p in tree.alive(process_table()) if p != child.pid])
        reap_elapsed, survivors = reap_owned(tree, args.reap_budget)
        reaped = True
        child_status = child.poll()
        if child_status is None:
            try:
                child_status = child.wait(timeout=0.5)
            except subprocess.TimeoutExpired:
                child_status = None
    finally:
        if not reaped:
            # An exception escaped the loop: the worker still must not outlive
            # the launcher that was bounding it.
            tree.refresh()
            reap_owned(tree, args.reap_budget)
        # The recording handlers stay until the result is written: a second
        # signal during the resource cleaner would otherwise kill the launcher
        # with no result and the named resources still open.
        service.stop()
        shutil.rmtree(socket_dir, ignore_errors=True)

    cleanup_failures = []
    if survivors:
        cleanup_failures.append({"survivors": survivors})
    if reap_elapsed > args.reap_budget:
        cleanup_failures.append({"reap_overshoot_s": round(reap_elapsed, 3)})
    if runs_cleaner(manifest, args.resource_cleaner):
        failed = run_cleaner(
            args.resource_cleaner, manifest["named_resources"], args.reap_budget
        )
        if failed:
            cleanup_failures.append({"resources": failed})

    observed = {}
    for name in sorted(OBSERVED_SCOPES):
        if scopes.get(name) == "observed":
            observed[name] = {
                "count": telemetry.observed.get(name, "unknown"),
                "configured": manifest.get("observed_budgets", {}).get(name),
            }
    granted = sorted(service.granted, key=service.granted.get)
    result = {
        "run_id": manifest["run_id"],
        "manifest_sha256": digest,
        "child_exit": child_status,
        "scopes": scopes,
        "hard": {
            "scripted_actions": {
                "granted": len(granted),
                "ceiling": manifest["action_ceiling"],
                "granted_ids": granted,
                "end_permit_used": FINAL_ACTION_ID in service.granted,
                "denials": service.denials,
                "dispatched_without_permit": [
                    a for a in telemetry.dispatched if a not in service.granted
                ],
            },
            "wall_time": {
                "elapsed_s": round(time.monotonic() - t0, 3),
                "cap_s": cap,
                "new_work_stopped_at_s": stop_new_work,
                "max_sample_gap_s": round(max_gap, 3),
            },
            "agent_alive_exposure": (
                {"seconds": round(exposure, 3), "limit_s": exposure_limit}
                if exposure_hard
                else "not_enforced"
            ),
        },
        "observed": observed,
        "budget_failure": failure,
        "cleanup": {
            "reaped": reaped,
            "leaked_children": leaked,
            "reap_elapsed_s": round(reap_elapsed, 3),
            "failures": cleanup_failures,
        },
        "telemetry": {
            "events": telemetry.events,
            "malformed": telemetry.malformed,
            "heartbeat_seen": telemetry.last_heartbeat is not None,
        },
    }
    if args.allow_short_reserve:
        result["test_overrides"] = {"min_cleanup_reserve_s": args.allow_short_reserve}
    if args.max_join_hash_s != MAX_JOIN_HASH_S:
        # A result produced under a loosened join-hash limit says so.
        result["overrides"] = {"max_join_hash_s": args.max_join_hash_s}

    final_bytes = archive_bytes(artifacts)
    if max_archive is not None and final_bytes > max_archive:
        artifact_failure = artifact_failure or "archive_bytes"
    result["resources"] = {
        "arm": args.arm,
        "cadence_s": args.resource_cadence,
        "samples": sampler.samples,
        "max_samples": sampler.max_samples,
        "bytes": sampler.bytes,
        "max_bytes": sampler.max_bytes,
        "overflow": sampler.overflow,
    }
    result["archive"] = {
        "bytes": final_bytes,
        "max_archive_bytes": max_archive if max_archive is not None else "unbounded",
        "artifact_failure": artifact_failure,
    }
    if failure:
        write_json(os.path.join(artifacts, "incomplete_budget.json"), result)
    if artifact_failure:
        write_json(os.path.join(artifacts, "incomplete_artifact.json"), result)
    if cleanup_failures:
        write_json(os.path.join(artifacts, "incomplete_cleanup.json"), result)
    if cleanup_failures:
        status, code = "incomplete_cleanup", EXIT_INCOMPLETE_CLEANUP
    elif artifact_failure:
        status, code = "incomplete_artifact", EXIT_INCOMPLETE_ARTIFACT
    elif failure:
        status, code = "incomplete_budget", EXIT_INCOMPLETE_BUDGET
    elif child_status != 0:
        status, code = "child_failed", EXIT_CHILD_FAILED
    else:
        status, code = "complete", 0
    result["status"] = status
    write_json(os.path.join(artifacts, "budget-result.json"), result)
    print(f"run budget: {status} ({manifest['run_id']})", file=sys.stderr)
    for signum, handler in previous_handlers.items():
        signal.signal(signum, handler)
    return code


# --------------------------------------------------------------------------
# Command line
# --------------------------------------------------------------------------


def command_validate(args):
    try:
        manifest, _, digest = load_manifest(args.manifest, args.min_cleanup_reserve_s)
        if args.verify_binaries:
            verify_binaries(manifest)
    except ManifestError as error:
        for reason in error.errors:
            print(f"manifest: {reason}", file=sys.stderr)
        return EXIT_PREFLIGHT
    if args.print_field == "sha":
        print(digest)
    elif args.print_field == "run_id":
        print(manifest["run_id"])
    else:
        print(f"ok {manifest['run_id']} {digest}")
    return 0


def command_binary(args):
    try:
        manifest, _, _ = load_manifest(args.manifest, args.min_cleanup_reserve_s)
        verify_binaries(manifest)
        entry = manifest["binaries"].get(args.role)
        if entry is None:
            raise ManifestError([f"manifest pins no {args.role!r} binary"])
    except ManifestError as error:
        for reason in error.errors:
            print(f"manifest: {reason}", file=sys.stderr)
        return EXIT_PREFLIGHT
    print(entry["path"])
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)

    validate = commands.add_parser("validate", help="check a manifest")
    validate.add_argument("--manifest", required=True)
    validate.add_argument("--verify-binaries", action="store_true")
    validate.add_argument("--print", dest="print_field", choices=("sha", "run_id"))
    validate.add_argument(
        "--min-cleanup-reserve-s", type=int, default=MIN_CLEANUP_RESERVE_S
    )
    validate.set_defaults(func=command_validate)

    binary = commands.add_parser("binary", help="print a verified pinned executable")
    binary.add_argument("--manifest", required=True)
    binary.add_argument(
        "--min-cleanup-reserve-s", type=int, default=MIN_CLEANUP_RESERVE_S
    )
    binary.add_argument("role")
    binary.set_defaults(func=command_binary)

    run = commands.add_parser("run", help="run a command under the budget")
    run.add_argument("--manifest", required=True)
    run.add_argument("--artifact-dir", required=True)
    run.add_argument("--resource-cleaner", help="executable run as: KIND NAME")
    run.add_argument("--sample-interval", type=float, default=SAMPLE_INTERVAL_MAX_S)
    run.add_argument("--reap-budget", type=float, default=REAP_BUDGET_S)
    run.add_argument("--telemetry-stale", type=float, default=TELEMETRY_STALE_S)
    run.add_argument(
        "--arm",
        choices=RESOURCE_ARMS,
        default="candidate",
        help="which arm of a comparison this run is, recorded on each sample",
    )
    run.add_argument(
        "--resource-cadence",
        type=float,
        default=RESOURCE_CADENCE_S,
        help="seconds between process resource samples (30 unless a fixture)",
    )
    run.add_argument(
        "--max-join-hash-s",
        type=float,
        default=MAX_JOIN_HASH_S,
        help=(
            "seconds the pinned binaries may take to re-hash at the join "
            f"permit (default {MAX_JOIN_HASH_S}, at most "
            f"{MAX_JOIN_HASH_CEILING_S:.2f}); recorded in budget-result.json "
            "when it differs from the default"
        ),
    )
    run.add_argument(
        "--allow-short-reserve",
        type=int,
        default=0,
        help="fixture runs only: lower the 60 second cleanup reserve minimum",
    )

    raw = list(sys.argv[1:] if argv is None else argv)
    command = []
    if "--" in raw:
        split = raw.index("--")
        raw, command = raw[:split], raw[split + 1 :]
    args = parser.parse_args(raw)
    if args.command == "run":
        if not command:
            parser.error("run needs -- followed by the worker command")
        return launch(args, command)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
