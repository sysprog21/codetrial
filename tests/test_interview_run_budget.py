#!/usr/bin/env python3
"""Contract tests for scripts/interview-run-budget.py and its worker guards.

Every worker here is a fake: a Python stub speaking the real permit protocol,
a stub server answering wrongly, or a `sleep`. Nothing calls a paid API, opens
a browser or touches the network beyond a Unix socket in a scratch directory.
The launcher runs as its own process so its subreaper mark, its signals and its
death are real and cannot leak into the other cases the runner runs at once.
"""

import hashlib
import importlib.util
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TOOL = ROOT / "scripts" / "interview-run-budget.py"
STUBS = ROOT / "tests" / "stubs" / "interview-run-budget"
WORKER = STUBS / "fake_worker.py"
DRIVER = STUBS / "guard-driver.cjs"

SUPPORT_SPEC = importlib.util.spec_from_file_location(
    "support", Path(__file__).with_name("support.py")
)
support = importlib.util.module_from_spec(SUPPORT_SPEC)
SUPPORT_SPEC.loader.exec_module(support)

SPEC = importlib.util.spec_from_file_location("interview_run_budget", TOOL)
irb = importlib.util.module_from_spec(SPEC)
sys.modules["interview_run_budget"] = irb
SPEC.loader.exec_module(irb)

ACTIONS = ["candidate_join", "mic_1", "edit_1", "board_1"]
HEX = "a" * 64


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    try:
        # A zombie still answers signal 0; it is not running.
        with open(f"/proc/{pid}/stat") as handle:
            return handle.read().rpartition(")")[2].split()[0] != "Z"
    except OSError:
        return True


def wait_until(predicate, seconds=10):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.02)
    return predicate()


class Scratch(unittest.TestCase):
    """A scratch tree, a pinned fake candidate and a manifest builder."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="irbt-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.out = self.root / "out"
        self.fake = self.root / "fake"
        self.fake.mkdir()
        self.candidate = self.root / "candidate"
        support.write_executable(self.candidate, "#!/bin/sh\nexit 0\n")
        self.spawned = []
        self.addCleanup(self.kill_spawned)

    def kill_spawned(self):
        for name in ("pids", "grandchild.pid", "sleeper.pid"):
            path = self.fake / name
            if path.exists():
                for token in path.read_text().split():
                    self.kill(int(token))
        for pid in self.spawned:
            self.kill(pid)

    @staticmethod
    def kill(pid):
        try:
            os.kill(pid, signal.SIGKILL)
        except OSError:
            pass

    def decoy(self):
        """A process that is none of this run's: same program, other session."""
        process = subprocess.Popen(["sleep", "300"], start_new_session=True)
        self.spawned.append(process.pid)
        return process

    def manifest(self, **overrides):
        data = {
            "schema_version": 1,
            "run_id": "run-test-1",
            "binaries": {
                "candidate": {
                    "path": str(self.candidate),
                    "sha256": sha256(self.candidate),
                }
            },
            "settings_sha256": HEX,
            "fixture_sha256": HEX,
            "selected_duration_s": 10,
            "outer_wall_cap_s": 30,
            "cleanup_reserve_s": 4,
            "end_mode": "explicit_end",
            "actions": list(ACTIONS),
            "action_ceiling": len(ACTIONS) + 1,
            "limits": {},
            "scopes": {"scripted_actions": "hard", "wall_time": "hard"},
        }
        data.update(overrides)
        return data

    def write_manifest(self, **overrides):
        path = self.root / "manifest.json"
        path.write_text(json.dumps(self.manifest(**overrides)))
        return path

    def launch(self, mode, *, extra=(), env=None, manifest=None, **overrides):
        """The launcher over a fake worker; returns (process, seconds)."""
        path = manifest or self.write_manifest(**overrides)
        environment = dict(os.environ)
        environment.update(
            FAKE_MODE=mode, FAKE_DIR=str(self.fake), IRB_MODULE=str(TOOL), **(env or {})
        )
        started = time.monotonic()
        command = [
            sys.executable,
            str(TOOL),
            "run",
            "--manifest",
            str(path),
            "--artifact-dir",
            str(self.out),
            "--allow-short-reserve",
            "1",
            "--sample-interval",
            "0.05",
            "--reap-budget",
            "1.5",
            *extra,
            "--",
            sys.executable,
            str(WORKER),
        ]
        result = subprocess.run(
            command,
            env=environment,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=90,
            check=False,
        )
        return result, time.monotonic() - started

    def result(self):
        return json.loads((self.out / "budget-result.json").read_text())

    def dispatched(self):
        path = self.fake / "dispatch.log"
        return path.read_text().split() if path.exists() else []

    def ledger(self):
        path = self.out / "budget-ledger.jsonl"
        return [json.loads(line) for line in path.read_text().splitlines()]

    def grants(self):
        return [e["action_id"] for e in self.ledger() if e["event"] == "grant"]

    def assert_no_overshoot(self):
        """Nothing was dispatched that the ledger did not grant first."""
        granted = self.grants()
        dispatched = self.dispatched()
        self.assertEqual([a for a in dispatched if a not in granted], [])
        self.assertLessEqual(len(granted), len(ACTIONS) + 1)
        self.assertEqual(len(set(dispatched)), len(dispatched))


class ManifestTests(Scratch):
    def errors(self, **overrides):
        return irb.validate_manifest(self.manifest(**overrides), min_reserve=1)

    def test_a_complete_manifest_is_accepted(self):
        self.assertEqual(self.errors(), [])

    def test_each_malformed_manifest_names_its_reason(self):
        scopes = {"scripted_actions": "hard", "wall_time": "hard"}
        rows = [
            (
                {"actions": ["a", "a", "b", "c"], "action_ceiling": 5},
                "duplicate action ID",
            ),
            ({"actions": ["a", "end"], "action_ceiling": 3}, "reserved permit"),
            ({"action_ceiling": 4}, "action_ceiling must be the nonfinal"),
            ({"action_ceiling": "5"}, "action_ceiling must be an integer"),
            ({"actions": [], "action_ceiling": 1}, "non-empty list"),
            ({"schema_version": 2}, "schema_version"),
            ({"run_id": "../x"}, "run_id"),
            ({"run_id": "run-test-1\n"}, "run_id"),
            ({"settings_sha256": "abc"}, "settings_sha256"),
            ({"fixture_sha256": None}, "fixture_sha256"),
            ({"binaries": {}}, "pins a candidate"),
            (
                {"binaries": {"candidate": {"path": "rel", "sha256": HEX}}},
                "absolute path",
            ),
            ({"selected_duration_s": 0}, "selected_duration_s"),
            ({"outer_wall_cap_s": 4, "cleanup_reserve_s": 4}, "below outer_wall_cap_s"),
            ({"selected_duration_s": 27}, "fit before the cleanup reserve"),
            ({"end_mode": "whenever"}, "end_mode"),
            ({"limits": {"turns": 3}}, "limits may only contain"),
            ({"scopes": {**scopes, "model_turns": "hard"}}, "cannot be hard"),
            ({"scopes": {**scopes, "provider_invoice": "hard"}}, "cannot be hard"),
            ({"scopes": {"wall_time": "hard"}}, "scopes.scripted_actions must be hard"),
            (
                {"scopes": {**scopes, "agent_alive_exposure": "hard"}},
                "needs limits.agent_alive_s",
            ),
            ({"scopes": {**scopes, "model_turns": "sometimes"}}, "unknown kind"),
            ({"observed_budgets": {"restarts": -1}}, "observed_budgets"),
            ({"named_resources": [{"kind": "room"}]}, "named_resources"),
            ({"surprise": 1}, "unknown field"),
            (
                {"actions": ["mic_1", "edit_1"], "action_ceiling": 3},
                "must include the join action",
            ),
        ]
        for overrides, expected in rows:
            with self.subTest(expected=expected):
                joined = "; ".join(self.errors(**overrides))
                self.assertIn(expected, joined)

    def test_run_kind_and_end_mode_vocabulary_is_shape_checked(self):
        accepted = [
            {},
            {
                "run_kind": "qualification",
                "end_mode": "manual",
                "duration_min": 90,
                "max_archive_bytes": irb.archive_bounds(30)[0],
            },
            {"run_kind": "smoke", "end_mode": "server_deadline"},
            {"run_kind": "smoke", "end_mode": "absence"},
            {"run_kind": "smoke", "end_mode": "room_closed"},
            {"run_kind": "smoke", "end_mode": "terminal_cancel"},
            {"run_kind": "clip", "end_mode": "native_close"},
            {
                "run_kind": "experiment",
                "end_mode": "manual",
                "observation_target_s": 1200,
                "experiment_profile_min": 20,
            },
        ]
        for overrides in accepted:
            with self.subTest(overrides=overrides):
                self.assertEqual(self.errors(**overrides), [])
        rows = [
            ({"run_kind": "marathon"}, "run_kind must be one of"),
            (
                {"run_kind": "clip", "end_mode": "manual"},
                "native_close is the end mode",
            ),
            (
                {"run_kind": "smoke", "end_mode": "native_close"},
                "native_close is the end mode",
            ),
            ({"duration_min": 0}, "duration_min must be a positive integer"),
            ({"duration_min": True}, "duration_min must be a positive integer"),
            ({"observation_target_s": -1}, "observation_target_s must be a positive"),
            (
                {"experiment_profile_min": "20"},
                "experiment_profile_min must be a positive",
            ),
        ]
        for overrides, expected in rows:
            with self.subTest(expected=expected):
                self.assertIn(expected, "; ".join(self.errors(**overrides)))

    def test_the_sixty_second_cleanup_reserve_is_the_default_minimum(self):
        errors = irb.validate_manifest(self.manifest())
        self.assertIn("cleanup_reserve_s must be at least 60", "; ".join(errors))

    def test_binary_hashes_are_checked_against_the_manifest(self):
        manifest = self.manifest()
        irb.verify_binaries(manifest)

        manifest["binaries"]["candidate"]["sha256"] = HEX
        with self.assertRaises(irb.ManifestError) as raised:
            irb.verify_binaries(manifest)
        self.assertIn("does not match the manifest", str(raised.exception))

        manifest["binaries"]["candidate"]["path"] = str(self.root / "absent")
        with self.assertRaises(irb.ManifestError) as raised:
            irb.verify_binaries(manifest)
        self.assertIn("cannot read", str(raised.exception))

    def test_validate_subcommand_prints_the_hash_the_worker_must_match(self):
        path = self.write_manifest(cleanup_reserve_s=60, outer_wall_cap_s=600)
        result = subprocess.run(
            [
                sys.executable,
                str(TOOL),
                "validate",
                "--manifest",
                str(path),
                "--print",
                "sha",
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), sha256(path))

    def test_process_table_parsers_read_proc_and_ps_rows(self):
        stat = "4242 (a (weird) name) S 7 4242 4242 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 987654 0 0"
        self.assertEqual(irb.parse_proc_stat(stat), (4242, "S", 7, 4242, 987654))
        table = irb.parse_ps("  10     1    10 Ss\n  11    10    10 Z+\n")
        self.assertEqual(table, {10: ("S", 1, 10, 0), 11: ("Z", 10, 10, 0)})
        table = irb.parse_ps(
            "  10     1    10 Ss   Mon Oct 30 12:34:56 2026\n"
            "  11    10    10 Z+   Mon Oct  5 01:02:03 2026\n"
        )
        self.assertEqual(table[10][3], "Mon Oct 30 12:34:56 2026")
        self.assertEqual(table[11][3], "Mon Oct 5 01:02:03 2026")


class PermitServiceTests(Scratch):
    def service(self, **kwargs):
        manifest = self.manifest()
        digest = HEX
        service = irb.PermitService(
            manifest, digest, str(self.root / "ledger"), **kwargs
        )
        self.digest = digest
        return service

    def ask(self, service, action_id, seq, **fields):
        request = {
            "v": 1,
            "op": "permit",
            "run_id": "run-test-1",
            "manifest_sha256": self.digest,
            "action_id": action_id,
            "seq": seq,
        }
        request.update(fields)
        return service.decide(request)

    def test_each_action_is_granted_once_in_sequence(self):
        service = self.service()
        for seq, action in enumerate(ACTIONS, start=1):
            self.assertTrue(self.ask(service, action, seq)["granted"])
        self.assertTrue(self.ask(service, "end", 5)["granted"])
        self.assertEqual(service.count(), 5)

    def test_a_duplicate_is_denied_and_stops_new_work(self):
        service = self.service()
        self.assertTrue(self.ask(service, "candidate_join", 1)["granted"])
        reply = self.ask(service, "candidate_join", 2)
        self.assertEqual(
            reply, {"v": 1, "granted": False, "reason": "duplicate_action"}
        )
        self.assertFalse(self.ask(service, "mic_1", 2)["granted"])
        self.assertEqual(service.count(), 1)

    def test_every_refused_request_leaves_the_count_untouched(self):
        rows = [
            ("unknown_action", dict(action_id="board_9", seq=1)),
            ("sequence_mismatch", dict(action_id="mic_1", seq=2)),
            ("sequence_mismatch", dict(action_id="mic_1", seq=0)),
            ("malformed_request", dict(action_id=7, seq=1)),
            ("malformed_request", dict(action_id="mic_1", seq="1")),
            ("malformed_request", dict(action_id="mic_1", seq=True)),
            ("malformed_request", dict(v=2)),
            ("malformed_request", dict(op="grant")),
            ("run_identity_mismatch", dict(run_id="someone-else")),
            ("run_identity_mismatch", dict(manifest_sha256="0" * 64)),
        ]
        for reason, fields in rows:
            with self.subTest(reason=reason, fields=fields):
                service = self.service()
                fields = {"action_id": "mic_1", "seq": 1, **fields}
                reply = self.ask(
                    service, fields.pop("action_id"), fields.pop("seq"), **fields
                )
                self.assertEqual((reply["granted"], reply["reason"]), (False, reason))
                self.assertEqual(service.count(), 0)
        self.assertFalse(self.service().decide("not an object")["granted"])

    def test_only_the_reserved_end_is_granted_while_draining(self):
        service = self.service()
        self.assertTrue(self.ask(service, "candidate_join", 1)["granted"])
        service.drain("wall_budget")
        self.assertFalse(self.ask(service, "mic_1", 2)["granted"])
        self.assertTrue(self.ask(service, "end", 2)["granted"])
        service.close("reaping")
        self.assertFalse(self.ask(service, "end", 3)["granted"])

    def test_new_work_is_refused_at_the_deadline_even_if_the_sampler_stalls(self):
        now = [100.0]
        service = self.service(new_work_deadline=110.0, clock=lambda: now[0])
        self.assertTrue(self.ask(service, "candidate_join", 1)["granted"])
        now[0] = 110.0
        reply = self.ask(service, "mic_1", 2)
        self.assertEqual((reply["granted"], reply["reason"]), (False, "draining"))
        self.assertTrue(self.ask(service, "end", 2)["granted"])

    def test_counts_survive_a_service_restart_from_the_ledger(self):
        first = self.service()
        self.assertTrue(self.ask(first, "candidate_join", 1)["granted"])
        self.assertTrue(self.ask(first, "mic_1", 2)["granted"])
        restarted = self.service()
        self.assertEqual(restarted.count(), 2)
        self.assertFalse(self.ask(restarted, "mic_1", 3)["granted"])
        self.assertFalse(self.ask(restarted, "edit_1", 3)["granted"])
        # The duplicate above was a fault, so only the reserved End is left.
        self.assertTrue(self.ask(restarted, "end", 3)["granted"])

    def test_a_pinned_binary_that_changed_blocks_the_join(self):
        service = self.service(verify_join_binaries=True)
        self.candidate.write_bytes(b"#!/bin/sh\nexit 1\n")
        reply = self.ask(service, "candidate_join", 1)
        self.assertEqual(
            (reply["granted"], reply["reason"]), (False, "binary_changed:candidate")
        )
        self.assertEqual(service.count(), 0)


class ServedPermitTests(Scratch):
    """The same service over a real Unix socket, with real worker guards."""

    def serve(self, **overrides):
        manifest = self.manifest(**overrides)
        raw = json.dumps(manifest).encode()
        digest = irb.manifest_digest(raw)
        self.service = irb.PermitService(manifest, digest, str(self.root / "ledger"))
        self.socket_path = str(self.root / "p.sock")
        self.service.start(self.socket_path)
        self.addCleanup(self.service.stop)
        return manifest["run_id"], digest

    def guard(self):
        run_id, digest = self.run_id, self.digest
        return irb.WorkerGuard(self.socket_path, run_id, digest)

    def setUp(self):
        super().setUp()
        self.run_id, self.digest = self.serve()

    def test_a_reconnecting_guard_continues_the_count_instead_of_restarting_it(self):
        first = self.guard()
        first.permit("candidate_join")
        first.permit("mic_1")
        reloaded = self.guard()
        self.assertEqual(reloaded.next_seq, 3)
        reloaded.permit("edit_1")
        self.assertEqual(self.service.count(), 3)

    def test_a_guard_that_restarts_its_own_count_is_refused_and_dies(self):
        first = self.guard()
        first.permit("candidate_join")
        stale = self.guard()
        stale.next_seq = 1
        with self.assertRaises(irb.PermitDenied):
            stale.permit("mic_1")
        self.assertEqual(self.service.count(), 1)
        with self.assertRaises(irb.PermitDenied) as raised:
            stale.permit("mic_1")
        self.assertIn("guard is dead", str(raised.exception))

    def test_denied_work_is_not_dispatched(self):
        guard = self.guard()
        dispatched = []
        guard.run_action("candidate_join", lambda: dispatched.append("candidate_join"))
        with self.assertRaises(irb.PermitDenied):
            guard.run_action("surplus", lambda: dispatched.append("surplus"))
        self.assertEqual(dispatched, ["candidate_join"])

    def test_a_request_never_finished_is_a_fault_not_a_grant(self):
        conn = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        conn.connect(self.socket_path)
        conn.sendall(b'{"v": 1, "op": "perm')
        conn.close()
        self.assertTrue(wait_until(lambda: self.service.fault is not None))
        self.assertEqual(self.service.fault, "malformed_request")
        self.assertEqual(self.service.count(), 0)

    def test_a_silent_connection_is_dropped_without_a_fault(self):
        conn = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        conn.connect(self.socket_path)
        conn.settimeout(3)
        self.assertEqual(conn.recv(10), b"")
        conn.close()
        self.assertIsNone(self.service.fault)

    def test_worker_environment_must_match_the_launchers_manifest(self):
        path = self.root / "manifest.json"
        path.write_text(json.dumps(self.manifest()))
        env = {
            "BROWSER_CHECK_RUN_MANIFEST": str(path),
            "BROWSER_CHECK_RUN_ID": "run-test-1",
            "BROWSER_CHECK_RUN_MANIFEST_SHA256": sha256(path),
            "BROWSER_CHECK_BUDGET_SOCKET": self.socket_path,
        }
        for broken in (
            {"BROWSER_CHECK_RUN_MANIFEST_SHA256": HEX},
            {"BROWSER_CHECK_RUN_ID": "run-test-2"},
        ):
            with self.subTest(broken=broken):
                with self.assertRaises(irb.PermitDenied):
                    irb.WorkerGuard.from_env({**env, **broken})


class StubServer:
    """A permit server that answers wrongly, once per connection, and counts."""

    def __init__(self, path, behavior):
        self.connections = 0
        self.behavior = behavior
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.server.bind(path)
        self.server.listen(8)
        self.server.settimeout(0.1)
        self.stopped = threading.Event()
        self.thread = threading.Thread(target=self.loop, daemon=True)
        self.thread.start()

    def loop(self):
        held = []
        while not self.stopped.is_set():
            try:
                conn, _ = self.server.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            self.connections += 1
            held.append(conn)
            data = b""
            conn.settimeout(2)
            try:
                while b"\n" not in data:
                    data += conn.recv(4096)
            except OSError:
                continue
            request = json.loads(data)
            if request["op"] == "status":
                conn.sendall(b'{"v": 1, "ok": true, "count": 0}\n')
                conn.close()
                self.connections -= 1
                continue
            self.behavior(conn, request)
        for conn in held:
            conn.close()

    def stop(self):
        self.stopped.set()
        self.thread.join(3)
        self.server.close()


class PermitClientFailureTests(Scratch):
    def stub(self, behavior):
        path = str(self.root / "stub.sock")
        server = StubServer(path, behavior)
        self.addCleanup(server.stop)
        guard = irb.WorkerGuard(path, "run-test-1", HEX)
        return server, guard

    def test_a_reply_that_never_comes_fails_closed_within_the_timeout_without_retry(
        self,
    ):
        server, guard = self.stub(lambda conn, request: None)
        started = time.monotonic()
        with self.assertRaises(irb.LauncherLost):
            guard.permit("candidate_join")
        self.assertLess(time.monotonic() - started, 1.6)
        self.assertEqual(server.connections, 1)
        # The action may have been granted, so the guard refuses the next ask
        # rather than offering the same action again.
        with self.assertRaises(irb.PermitDenied):
            guard.permit("candidate_join")
        self.assertEqual(server.connections, 1)

    def test_a_lost_connection_fails_closed_without_retry(self):
        server, guard = self.stub(lambda conn, request: conn.close())
        with self.assertRaises(irb.LauncherLost):
            guard.permit("candidate_join")
        self.assertEqual(server.connections, 1)

    def test_replies_that_are_not_an_exact_grant_fail_closed(self):
        replies = {
            "not json": b"{oops\n",
            "wrong shape": b"[1, 2]\n",
            "wrong version": b'{"v": 9, "granted": true}\n',
            "granted as text": b'{"v": 1, "granted": "yes", "action_id": "candidate_join", "seq": 1}\n',
            "other action": b'{"v": 1, "granted": true, "action_id": "mic_1", "seq": 1}\n',
            "other sequence": b'{"v": 1, "granted": true, "action_id": "candidate_join", "seq": 2}\n',
            "refusal": b'{"v": 1, "granted": false, "reason": "draining"}\n',
        }
        for name, reply in replies.items():
            with self.subTest(reply=name):
                path = str(self.root / f"{abs(hash(name))}.sock")
                server = StubServer(
                    path, lambda conn, request, r=reply: conn.sendall(r)
                )
                self.addCleanup(server.stop)
                guard = irb.WorkerGuard(path, "run-test-1", HEX)
                with self.assertRaises(irb.PermitDenied):
                    guard.permit("candidate_join")
                self.assertIsNotNone(guard.dead)

    def test_an_unreachable_launcher_is_reported_as_lost(self):
        with self.assertRaises(irb.LauncherLost):
            irb.WorkerGuard(str(self.root / "nobody.sock"), "run-test-1", HEX)

    def test_bounded_cleanup_gives_up_on_cleanup_that_hangs(self):
        release = threading.Event()
        self.addCleanup(release.set)
        started = time.monotonic()
        self.assertFalse(irb.bounded_cleanup(lambda: release.wait(30), 0.3))
        self.assertLess(time.monotonic() - started, 2)
        self.assertTrue(irb.bounded_cleanup(lambda: None, 1))


class LauncherRunTests(Scratch):
    def test_a_clean_run_completes_with_every_action_granted_and_dispatched_once(self):
        started = time.time()
        result, _ = self.launch("normal", env={"FAKE_OBSERVED": "1"})
        self.assertEqual(result.returncode, 0, result.stderr)
        report = self.result()
        self.assertEqual(report["status"], "complete")
        # The worker is told when the reap begins: before the wall cap.
        reap_at = int((self.fake / "reap_at").read_text())
        self.assertGreater(reap_at, started - 1)
        self.assertLess(reap_at, started + self.manifest()["outer_wall_cap_s"])
        self.assertEqual(self.dispatched(), ACTIONS + ["end"])
        self.assertEqual(self.grants(), ACTIONS + ["end"])
        hard = report["hard"]["scripted_actions"]
        self.assertEqual((hard["granted"], hard["ceiling"]), (5, 5))
        self.assertTrue(hard["end_permit_used"])
        self.assertEqual(hard["dispatched_without_permit"], [])
        self.assertEqual(report["hard"]["agent_alive_exposure"], "not_enforced")
        self.assertFalse((self.out / "incomplete_budget.json").exists())
        self.assertEqual(report["cleanup"]["failures"], [])

    def test_observed_counts_are_reported_or_unknown_never_invented(self):
        scopes = {
            "scripted_actions": "hard",
            "wall_time": "hard",
            "model_turns": "observed",
            "http_requests": "observed",
            "provider_invoice": "not_measured",
        }
        result, _ = self.launch(
            "normal",
            env={"FAKE_OBSERVED": "1"},
            scopes=scopes,
            observed_budgets={"model_turns": 20},
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        observed = self.result()["observed"]
        self.assertEqual(observed["model_turns"], {"count": 7, "configured": 20})
        self.assertEqual(
            observed["http_requests"], {"count": "unknown", "configured": None}
        )
        self.assertNotIn("provider_invoice", observed)

    # Expected: the 5 listed actions are dispatched, the sixth request is
    # refused, nothing is dispatched for it, and the run is incomplete_budget.
    # This is the case a check moved after dispatch cannot pass: the surplus
    # line would reach dispatch.log and the telemetry.
    def test_an_overrun_dispatches_nothing_past_the_permits(self):
        result, _ = self.launch("overrun")
        self.assertEqual(result.returncode, irb.EXIT_INCOMPLETE_BUDGET, result.stderr)
        self.assertEqual(self.dispatched(), ACTIONS)
        self.assertNotIn("surplus_action", self.dispatched())
        self.assert_no_overshoot()
        report = self.result()
        self.assertEqual(report["status"], "incomplete_budget")
        self.assertEqual(
            report["hard"]["scripted_actions"]["dispatched_without_permit"], []
        )
        self.assertIn("permit_fault:unknown_action", report["budget_failure"])
        self.assertTrue((self.out / "incomplete_budget.json").exists())

    def test_a_repeated_action_is_not_dispatched_twice(self):
        result, _ = self.launch("duplicate")
        self.assertEqual(result.returncode, irb.EXIT_INCOMPLETE_BUDGET, result.stderr)
        self.assertEqual(self.dispatched(), ["candidate_join"])
        self.assertEqual(
            self.result()["budget_failure"], "permit_fault:duplicate_action"
        )

    def test_a_reload_keeps_the_run_wide_count(self):
        result, _ = self.launch("reload")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.dispatched(), ACTIONS + ["end"])
        self.assertEqual(self.result()["hard"]["scripted_actions"]["granted"], 5)

    def test_a_reload_that_resets_its_sequence_gains_no_allowance(self):
        result, _ = self.launch("bad_reload")
        self.assertEqual(result.returncode, irb.EXIT_INCOMPLETE_BUDGET, result.stderr)
        self.assertEqual(self.dispatched(), ACTIONS[:2])
        self.assertIn("sequence_mismatch", self.result()["budget_failure"])
        self.assert_no_overshoot()

    def test_a_worker_that_dies_between_permit_and_dispatch_still_spends_the_permit(
        self,
    ):
        result, _ = self.launch("die_mid_permit")
        self.assertEqual(result.returncode, irb.EXIT_CHILD_FAILED, result.stderr)
        self.assertEqual(self.dispatched(), [])
        self.assertEqual(self.grants(), ["candidate_join"])
        self.assertEqual(self.result()["child_exit"], 9)

    def test_a_lost_reply_is_never_retried_into_a_second_dispatch(self):
        result, _ = self.launch("lose_reply")
        self.assertEqual(result.returncode, irb.EXIT_INCOMPLETE_BUDGET, result.stderr)
        self.assertEqual(self.dispatched(), [])
        self.assertEqual(self.grants(), ["candidate_join"])

    def test_a_hung_worker_that_ignores_term_is_reaped_inside_the_cap(self):
        decoy = self.decoy()
        result, seconds = self.launch(
            "hang",
            env={"FAKE_IGNORE_TERM": "1"},
            outer_wall_cap_s=6,
            cleanup_reserve_s=2,
            selected_duration_s=3,
        )
        self.assertEqual(result.returncode, irb.EXIT_INCOMPLETE_BUDGET, result.stderr)
        report = self.result()
        self.assertEqual(report["status"], "incomplete_budget")
        self.assertEqual(report["budget_failure"], "wall_budget")
        self.assertLess(seconds, 6 + 2)
        self.assertLessEqual(report["cleanup"]["reap_elapsed_s"], 1.5 + 0.3)
        self.assertEqual(report["cleanup"]["failures"], [])
        # Both the worker, which ignored TERM, and a grandchild that left the
        # process group are gone.
        pids = [int(p) for p in (self.fake / "pids").read_text().split()]
        grandchild = int((self.fake / "grandchild.pid").read_text())
        self.assertFalse(alive(pids[0]))
        self.assertTrue(wait_until(lambda: not alive(grandchild), 3))
        # Nothing that was not started by this run was touched.
        self.assertTrue(alive(decoy.pid))
        self.assertTrue((self.out / "incomplete_budget.json").exists())

    def test_the_end_is_requested_only_within_the_remaining_budget(self):
        result, _ = self.launch(
            "polite",
            outer_wall_cap_s=8,
            cleanup_reserve_s=5,
            selected_duration_s=2,
            limits={"agent_alive_s": 100},
            scopes={
                "scripted_actions": "hard",
                "wall_time": "hard",
                "agent_alive_exposure": "hard",
            },
            extra=("--telemetry-stale", "1.0"),
        )
        self.assertEqual(result.returncode, irb.EXIT_INCOMPLETE_BUDGET, result.stderr)
        report = self.result()
        self.assertEqual(report["budget_failure"], "telemetry_lost")
        # The worker ended normally on the stop file, spending the reserved
        # permit after the drain began and not before.
        self.assertEqual(report["child_exit"], 0)
        self.assertEqual(self.grants(), ["candidate_join", "end"])
        self.assertEqual(self.dispatched(), ["candidate_join", "end"])

    def test_agent_exposure_over_its_limit_stops_new_work(self):
        result, _ = self.launch(
            "exposure",
            limits={"agent_alive_s": 1},
            scopes={
                "scripted_actions": "hard",
                "wall_time": "hard",
                "agent_alive_exposure": "hard",
            },
        )
        self.assertEqual(result.returncode, irb.EXIT_INCOMPLETE_BUDGET, result.stderr)
        report = self.result()
        self.assertEqual(report["budget_failure"], "agent_exposure")
        exposure = report["hard"]["agent_alive_exposure"]
        self.assertGreaterEqual(exposure["seconds"], 1)
        self.assertLess(exposure["seconds"], 3)
        self.assertEqual(self.grants(), ["candidate_join", "end"])

    def start_waiting_launcher(self, extra=(), **overrides):
        """A launcher over a worker that waits for it to die; returns the process."""
        path = self.write_manifest(outer_wall_cap_s=60, **overrides)
        environment = dict(os.environ)
        environment.update(
            FAKE_MODE="lose_launcher", FAKE_DIR=str(self.fake), IRB_MODULE=str(TOOL)
        )
        launcher = subprocess.Popen(
            [
                sys.executable,
                str(TOOL),
                "run",
                "--manifest",
                str(path),
                "--artifact-dir",
                str(self.out),
                "--allow-short-reserve",
                "1",
                "--sample-interval",
                "0.05",
                "--reap-budget",
                "1.5",
                *extra,
                "--",
                sys.executable,
                str(WORKER),
            ],
            env=environment,
            stdin=subprocess.DEVNULL,
            start_new_session=True,
        )
        self.spawned.append(launcher.pid)
        self.assertTrue(wait_until(lambda: (self.fake / "ready").exists()))
        # A killed launcher cannot remove its own socket directory.
        socket_dir = Path((self.fake / "socket").read_text().strip()).parent
        self.addCleanup(shutil.rmtree, socket_dir, True)
        return launcher

    def test_a_terminated_launcher_reaps_its_worker_instead_of_leaving_it_spending(
        self,
    ):
        launcher = self.start_waiting_launcher()
        sleeper = int((self.fake / "sleeper.pid").read_text())
        worker = int((self.fake / "pids").read_text().split()[0])
        launcher.terminate()
        self.assertEqual(launcher.wait(timeout=20), irb.EXIT_INCOMPLETE_BUDGET)
        self.assertFalse(alive(worker))
        self.assertTrue(wait_until(lambda: not alive(sleeper), 3))
        self.assertEqual(self.result()["budget_failure"], "launcher_signal:SIGTERM")
        self.assertEqual(self.result()["cleanup"]["failures"], [])

    def test_a_second_signal_during_the_resource_cleaner_still_writes_the_result(self):
        started = self.root / "cleaner-started"
        cleaner = self.root / "slow-cleaner.sh"
        cleaner.write_text(f"#!/bin/sh\ntouch {started}\nsleep 1\n")
        cleaner.chmod(0o755)
        launcher = self.start_waiting_launcher(
            extra=("--resource-cleaner", str(cleaner)),
            named_resources=[{"kind": "room", "name": "interview-test-1"}],
        )
        launcher.terminate()
        self.assertTrue(wait_until(started.exists, 20))
        # Past the reap and inside the cleaner: the default handler would
        # kill the launcher here with no result and the room still open.
        launcher.terminate()
        self.assertEqual(launcher.wait(timeout=20), irb.EXIT_INCOMPLETE_BUDGET)
        self.assertEqual(self.result()["budget_failure"], "launcher_signal:SIGTERM")

    def test_losing_the_launcher_stops_new_work_and_runs_the_workers_own_cleanup(self):
        launcher = self.start_waiting_launcher()
        sleeper = int((self.fake / "sleeper.pid").read_text())
        worker = int((self.fake / "pids").read_text().split()[0])
        self.assertTrue(alive(sleeper))
        launcher.kill()
        launcher.wait()
        self.assertTrue(wait_until(lambda: (self.fake / "cleanup_done").exists(), 8))
        self.assertTrue(wait_until(lambda: not alive(sleeper), 3))
        self.assertTrue(wait_until(lambda: not alive(worker), 5))
        self.assertIn(
            (self.fake / "denied_after_loss").read_text().strip(),
            ("LauncherLost", "PermitDenied"),
        )
        self.assertEqual(self.dispatched(), [])

    def test_a_telemetry_file_that_cannot_prove_exposure_does_not_pass_a_hard_limit(
        self,
    ):
        # No heartbeat ever arrives, so the hard exposure counter is missing.
        result, _ = self.launch(
            "hang",
            outer_wall_cap_s=6,
            cleanup_reserve_s=2,
            selected_duration_s=3,
            limits={"agent_alive_s": 100},
            scopes={
                "scripted_actions": "hard",
                "wall_time": "hard",
                "agent_alive_exposure": "hard",
            },
            extra=("--telemetry-stale", "1.0"),
        )
        self.assertEqual(result.returncode, irb.EXIT_INCOMPLETE_BUDGET, result.stderr)
        self.assertEqual(self.result()["budget_failure"], "telemetry_lost")

    def test_failed_resource_cleanup_is_incomplete_cleanup_not_success(self):
        cleaner = self.root / "cleaner"
        support.write_executable(
            cleaner, f'#!/bin/sh\necho "$1 $2" >> {self.root}/cleaned\nexit 1\n'
        )
        result, _ = self.launch(
            "normal",
            extra=("--resource-cleaner", str(cleaner)),
            named_resources=[{"kind": "room", "name": "interview-test-1"}],
        )
        self.assertEqual(result.returncode, irb.EXIT_INCOMPLETE_CLEANUP, result.stderr)
        report = self.result()
        self.assertEqual(report["status"], "incomplete_cleanup")
        self.assertEqual(
            report["cleanup"]["failures"][0]["resources"][0]["name"], "interview-test-1"
        )
        self.assertTrue((self.out / "incomplete_cleanup.json").exists())
        self.assertEqual((self.root / "cleaned").read_text(), "room interview-test-1\n")

    def test_only_the_named_resources_are_passed_to_the_cleaner(self):
        cleaner = self.root / "cleaner"
        support.write_executable(
            cleaner, f'#!/bin/sh\necho "$1 $2" >> {self.root}/cleaned\n'
        )
        result, _ = self.launch(
            "normal",
            extra=("--resource-cleaner", str(cleaner)),
            named_resources=[{"kind": "room", "name": "interview-test-1"}],
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / "cleaned").read_text(), "room interview-test-1\n")

    def test_preflight_refuses_what_it_cannot_enforce_before_starting_anything(self):
        rows = {
            "declared resources without a cleaner": (
                dict(named_resources=[{"kind": "room", "name": "r1"}]),
                (),
                "no --resource-cleaner",
            ),
            "a hard limit with no counter": (
                dict(
                    scopes={
                        "scripted_actions": "hard",
                        "wall_time": "hard",
                        "model_turns": "hard",
                    }
                ),
                (),
                "cannot be hard",
            ),
            "a sampler slower than one second": (
                {},
                ("--sample-interval", "2"),
                "at most 1 second",
            ),
            "a reap that does not fit the reserve": (
                {},
                ("--reap-budget", "9"),
                "inside cleanup_reserve_s",
            ),
            "the 60 second reserve": (
                {"cleanup_reserve_s": 5},
                ("--allow-short-reserve", "0"),
                "at least 60",
            ),
        }
        for name, (overrides, extra, expected) in rows.items():
            with self.subTest(name):
                self.out = self.root / ("out-" + str(abs(hash(name))))
                result, _ = self.launch("normal", extra=extra, **overrides)
                self.assertEqual(result.returncode, irb.EXIT_PREFLIGHT, result.stderr)
                self.assertIn(expected, result.stderr)
                self.assertFalse((self.out / "budget-ledger.jsonl").exists())
                self.assertFalse((self.fake / "pids").exists())

    def test_a_second_run_in_the_same_artifact_directory_is_refused(self):
        result, _ = self.launch("normal")
        self.assertEqual(result.returncode, 0, result.stderr)
        before = (self.out / "budget-ledger.jsonl").read_text()
        again, _ = self.launch("normal")
        self.assertEqual(again.returncode, irb.EXIT_PREFLIGHT, again.stderr)
        self.assertIn("already holds a run", again.stderr)
        self.assertEqual((self.out / "budget-ledger.jsonl").read_text(), before)
        self.assertEqual(self.result()["status"], "complete")

    def test_a_pinned_binary_that_does_not_match_blocks_the_start(self):
        manifest = self.manifest()
        manifest["binaries"]["candidate"]["sha256"] = HEX
        path = self.root / "tampered.json"
        path.write_text(json.dumps(manifest))
        result, _ = self.launch("normal", manifest=path)
        self.assertEqual(result.returncode, irb.EXIT_PREFLIGHT, result.stderr)
        self.assertIn("does not match the manifest", result.stderr)
        self.assertFalse((self.fake / "pids").exists())
        self.assertTrue((self.out / "preflight_failed.json").exists())


class PreflightHashTests(Scratch):
    def test_a_pin_too_large_to_rehash_at_join_fails_preflight(self):
        class Args:
            sample_interval = 0.5
            reap_budget = 1.5
            resource_cadence = 30.0

        manifest = self.manifest()
        self.assertEqual(irb.preflight(manifest, Args, None), [])
        errors = irb.preflight(manifest, Args, None, max_hash_s=-1)
        self.assertIn("to hash", "; ".join(errors))


class ReapTests(Scratch):
    def test_a_process_that_cannot_be_killed_is_reported_as_a_survivor(self):
        child = subprocess.Popen(["sleep", "300"], start_new_session=True)
        self.spawned.append(child.pid)
        tree = irb.OwnedTree(child.pid, child.pid)
        elapsed, survivors = irb.reap_owned(tree, 0.5, send=lambda pid, sig: None)
        self.assertEqual(survivors, [child.pid])
        self.assertGreaterEqual(elapsed, 0.5)
        child.kill()
        child.wait()

    def test_reaping_signals_only_the_owned_tree(self):
        owned = subprocess.Popen(
            ["sh", "-c", "sleep 300 & wait"], start_new_session=True
        )
        other = self.decoy()
        self.spawned.append(owned.pid)
        self.assertTrue(
            wait_until(
                lambda: (
                    len(
                        [
                            p
                            for p, row in irb.process_table().items()
                            if row[2] == owned.pid
                        ]
                    )
                    >= 2
                )
            )
        )
        tree = irb.OwnedTree(owned.pid, owned.pid)
        signalled = []

        def send(pid, sig):
            signalled.append(pid)
            os.kill(pid, sig)

        _, survivors = irb.reap_owned(tree, 2.0, send=send)
        owned.wait(timeout=5)
        self.assertEqual(survivors, [])
        self.assertNotIn(other.pid, signalled)
        self.assertTrue(alive(other.pid))


class RootEnrollmentTests(Scratch):
    def test_a_recycled_root_pid_is_not_enrolled_after_the_worker_is_gone(self):
        tree = irb.OwnedTree(500, 500)
        tree.refresh({500: ("S", 1, 500, "t1")})
        self.assertEqual(tree.owned, {500: "t1"})
        tree.refresh({})
        self.assertEqual(tree.owned, {})
        tree.refresh({500: ("S", 1, 900, "t2")})
        self.assertEqual(tree.owned, {})


class CleanerBudgetTests(Scratch):
    def test_the_budget_bounds_the_whole_pass_not_each_resource(self):
        cleaner = os.path.join(self.root, "slow-cleaner.sh")
        with open(cleaner, "w") as handle:
            handle.write("#!/bin/sh\nsleep 5\n")
        os.chmod(cleaner, 0o755)
        resources = [{"kind": "room", "name": f"r{n}"} for n in range(3)]
        started = time.monotonic()
        failed = irb.run_cleaner(cleaner, resources, 0.6)
        self.assertLess(time.monotonic() - started, 1.3)
        self.assertEqual([item["reason"] for item in failed], ["timeout"] * 3)


class BrowserCheckValidationTests(Scratch):
    """scripts/browser-check.sh in validate-only mode: no browser, no build."""

    def setUp(self):
        super().setUp()
        self.manifest_path = self.write_manifest(
            outer_wall_cap_s=3600, cleanup_reserve_s=60, selected_duration_s=600
        )

    def check(self, **env):
        environment = {
            k: v
            for k, v in os.environ.items()
            if not k.startswith(("BROWSER_CHECK_", "CODETRIAL_"))
        }
        environment.update(
            BROWSER_CHECK_AGENT="home",
            BROWSER_CHECK_VALIDATE_ENV_ONLY="1",
            BROWSER_CHECK_RUN_MANIFEST=str(self.manifest_path),
            BROWSER_CHECK_RUN_ID="run-test-1",
            BROWSER_CHECK_RUN_MANIFEST_SHA256=sha256(self.manifest_path),
            BROWSER_CHECK_BUDGET_SOCKET=str(self.root / "unused.sock"),
            PYTHON=sys.executable,
        )
        environment.update(env)
        environment = {k: v for k, v in environment.items() if v is not None}
        return subprocess.run(
            ["sh", str(ROOT / "scripts" / "browser-check.sh")],
            env=environment,
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )

    def test_a_matching_manifest_and_pinned_binary_pass(self):
        result = self.check()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_a_run_without_a_manifest_is_unchanged(self):
        result = self.check(
            BROWSER_CHECK_RUN_MANIFEST=None,
            BROWSER_CHECK_RUN_ID=None,
            BROWSER_CHECK_RUN_MANIFEST_SHA256=None,
            BROWSER_CHECK_BUDGET_SOCKET=None,
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_each_mismatch_is_refused_before_anything_is_built(self):
        rows = [
            (
                {"BROWSER_CHECK_RUN_MANIFEST_SHA256": HEX},
                2,
                "RUN_MANIFEST_SHA256 does not match",
            ),
            ({"BROWSER_CHECK_RUN_ID": "other-run"}, 2, "RUN_ID does not match"),
            ({"BROWSER_CHECK_RUN_ID": None}, 2, "needs BROWSER_CHECK_RUN_MANIFEST"),
            ({"CODETRIAL_WEB_URL": "http://127.0.0.1:1"}, 2, "unset CODETRIAL_WEB_URL"),
        ]
        for env, code, expected in rows:
            with self.subTest(env=env):
                result = self.check(**env)
                self.assertEqual(result.returncode, code, result.stderr)
                self.assertIn(expected, result.stderr)

    def test_a_changed_pinned_binary_is_refused(self):
        self.candidate.write_bytes(b"#!/bin/sh\nexit 3\n")
        result = self.check()
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn("does not match the manifest", result.stderr)

    def test_a_live_run_needs_the_launchers_socket(self):
        result = self.check(
            BROWSER_CHECK_VALIDATE_ENV_ONLY=None, BROWSER_CHECK_FLOW="avatar"
        )
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn("live BROWSER_CHECK_BUDGET_SOCKET", result.stderr)

    def dummy_config(self):
        """The credentials a dispatch run asks for, from a throwaway config
        rather than whatever this machine keeps."""
        config = self.root / "dummy.env.local"
        config.write_text(
            "LIVEKIT_URL=wss://primary.example\nLIVEKIT_API_KEY=k\n"
            "LIVEKIT_API_SECRET=s\nGOOGLE_API_KEY=g\n"
        )
        return config

    def test_the_serving_fixture_needs_a_budgeted_dispatch_run_and_a_fresh_report(self):
        config = self.dummy_config()
        report = self.root / "fixture-report.json"
        # A reserve that outlasts the 150 s default stop wait with a minute over.
        manifest = self.write_manifest(
            outer_wall_cap_s=3600, cleanup_reserve_s=300, selected_duration_s=600
        )
        base = {
            "CODETRIAL_CONFIG_ENV": str(config),
            "BROWSER_CHECK_RUN_MANIFEST": str(manifest),
            "BROWSER_CHECK_RUN_MANIFEST_SHA256": sha256(manifest),
            "BROWSER_CHECK_FIXTURE_TEST": "module::serve",
            "BROWSER_CHECK_AGENT": "dispatch",
            "BROWSER_CHECK_FIXTURE_OUT": str(report),
            "BROWSER_CHECK_BUDGET_REAP_AT": str(int(time.time()) + 3600),
        }
        result = self.check(**base)
        self.assertEqual(result.returncode, 0, result.stderr)
        # A capture path that is free is fine.
        free = self.check(**base, BROWSER_CHECK_CAPTURE=str(self.root / "capture.json"))
        self.assertEqual(free.returncode, 0, free.stderr)
        no_run = {
            "BROWSER_CHECK_RUN_MANIFEST": None,
            "BROWSER_CHECK_RUN_ID": None,
            "BROWSER_CHECK_RUN_MANIFEST_SHA256": None,
            "BROWSER_CHECK_BUDGET_SOCKET": None,
        }
        existing = self.root / "already-there.json"
        existing.write_text("{}")
        linked = self.root / "linked"
        linked.symlink_to(self.root, target_is_directory=True)
        rows = [
            ({**base, "BROWSER_CHECK_AGENT": "home"}, "needs a budgeted run"),
            ({**base, **no_run}, "needs a budgeted run"),
            ({**base, "BROWSER_CHECK_FIXTURE_OUT": None}, "must name a report file"),
            ({**base, "BROWSER_CHECK_BUDGET_REAP_AT": None}, "BUDGET_REAP_AT"),
            ({**base, "BROWSER_CHECK_BUDGET_REAP_AT": "soon"}, "BUDGET_REAP_AT"),
            (
                {**base, "BROWSER_CHECK_CAPTURE": str(report)},
                "must name different files",
            ),
            (
                # Through a symlinked directory: the paths are compared once
                # resolved, not as they are spelled.
                {**base, "BROWSER_CHECK_CAPTURE": str(linked / report.name)},
                "must name different files",
            ),
            (
                {**base, "BROWSER_CHECK_CAPTURE": f"{report}.server.log"},
                "must name different files",
            ),
            (
                {
                    **base,
                    "BROWSER_CHECK_CAPTURE": str(report.with_suffix("")),
                    "BROWSER_CHECK_FIXTURE_OUT": f"{report.with_suffix('')}.partial",
                },
                "must name different files",
            ),
            (
                {**base, "BROWSER_CHECK_FIXTURE_OUT": str(existing)},
                "must name a report file",
            ),
            (
                {
                    **base,
                    "BROWSER_CHECK_FIXTURE_OUT": str(
                        self.root / "no-such-dir" / "r.json"
                    ),
                },
                "in a directory that does and can be written",
            ),
            (
                {**base, "BROWSER_CHECK_CAPTURE": str(existing)},
                "BROWSER_CHECK_CAPTURE must not exist yet",
            ),
            (
                {**base, "BROWSER_CHECK_FIXTURE_STOP_WAIT_S": "soon"},
                "number of seconds",
            ),
            (
                {**base, "BROWSER_CHECK_FIXTURE_STOP_WAIT_S": "0150"},
                "number of seconds",
            ),
            (
                {**base, "BROWSER_CHECK_FIXTURE_STOP_WAIT_S": "39"},
                "at least 40 seconds",
            ),
            (
                {**base, "BROWSER_CHECK_FIXTURE_STOP_WAIT_S": "241"},
                "needs at least 301s",
            ),
        ]
        for env, expected in rows:
            with self.subTest(env=env):
                result = self.check(**env)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn(expected, result.stderr)

    def test_the_stop_wait_must_fit_inside_the_manifests_cleanup_reserve(self):
        config = self.dummy_config()
        # The reserve the shared manifest here carries is the accepted minimum,
        # far short of a stop wait of 150 s.
        result = self.check(
            CODETRIAL_CONFIG_ENV=str(config),
            BROWSER_CHECK_AGENT="dispatch",
            BROWSER_CHECK_FIXTURE_TEST="module::serve",
            BROWSER_CHECK_FIXTURE_OUT=str(self.root / "report.json"),
        )
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn("cleanup_reserve_s is 60", result.stderr)
        self.assertIn("needs at least 210s", result.stderr)

    def test_the_fixture_is_started_in_a_session_of_its_own(self):
        script = (ROOT / "scripts" / "browser-check.sh").read_text()
        # A TERM sent to the script's process group must reach the script, whose
        # trap stops the fixture gracefully, and not the fixture first. The
        # launch itself needs a browser, so this reads the script.
        self.assertRegex(script, r'set -- setsid "\$@"')
        self.assertRegex(
            script, r'--ignored --exact "\$FIXTURE_TEST" --nocapture --test-threads=1'
        )

    def kill_pidfile(self, path):
        if path.exists():
            self.kill(int(path.read_text()))

    GOOD_CAPTURE = {
        "flow": "long-interview",
        "mode": "dispatch",
        "plan": {"durationMin": 90, "observationTargetS": 5400},
        "report": {
            "status": "complete",
            "exit_code": 0,
            "anchor": {"source": "server_clock"},
            "validation_fixture": False,
            "schedule_mode": "coding",
            "qualification": "not_evaluated",
            "reply_content": "not_assessed",
            "schedule_origin_unix_ms": 1700000000000,
        },
    }

    def good_capture(self):
        """The capture a finished long interview leaves, with the schedule hash
        the judge will rebuild from the plan."""
        sha = subprocess.run(
            [
                shutil.which("node"),
                "-e",
                "console.log(require('./scripts/long-interview-schedule.cjs')"
                ".generateSchedule('coding',{durationMin:90,observationTargetS:5400})"
                ".schedule.schedule_sha256)",
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
        capture = json.loads(json.dumps(self.GOOD_CAPTURE))
        capture["report"]["schedule_sha256"] = sha
        return json.dumps(capture)

    def run_fixture_path(self, writes_report=True, worker_exit=0, capture_text=None):
        """browser-check.sh from the manifest to the exit trap, with a stand-in
        fixture server and a stand-in worker: no browser, no credentials."""
        fixture = self.root / "stand-in-fixture"
        support.write_executable(
            fixture, support.stub("interview-run-budget/stand-in-fixture.py")
        )
        manifest = self.write_manifest(
            outer_wall_cap_s=3600,
            cleanup_reserve_s=300,
            selected_duration_s=600,
            binaries={"candidate": {"path": str(fixture), "sha256": sha256(fixture)}},
        )
        shim = self.root / "shim"
        shim.mkdir()
        # The script keeps its floor and latency logs outside its own scratch
        # directory by design; here they go inside this test's.
        tmp = self.root / "tmp"
        tmp.mkdir()
        support.write_executable(
            shim / "node", support.stub("interview-run-budget/stand-in-node.sh")
        )
        socket_path = self.root / "budget.sock"
        # Bound and never listened on: the script only asks that it be a socket.
        listener = socket.socket(socket.AF_UNIX)
        listener.bind(str(socket_path))
        self.addCleanup(listener.close)
        report = self.root / "kept-report.json"
        capture = self.root / "capture.json"
        pidfile = self.root / "stand-in.pid"
        self.addCleanup(self.kill_pidfile, pidfile)
        result = self.check(
            BROWSER_CHECK_VALIDATE_ENV_ONLY=None,
            BROWSER_CHECK_AGENT="dispatch",
            BROWSER_CHECK_FIXTURE_TEST="module::serve",
            BROWSER_CHECK_FIXTURE_OUT=str(report),
            BROWSER_CHECK_FIXTURE_STOP_WAIT_S="40",
            BROWSER_CHECK_CAPTURE=str(capture),
            BROWSER_CHECK_RUN_MANIFEST=str(manifest),
            BROWSER_CHECK_RUN_MANIFEST_SHA256=sha256(manifest),
            BROWSER_CHECK_BUDGET_SOCKET=str(socket_path),
            BROWSER_CHECK_BUDGET_REAP_AT=str(int(time.time()) + 3600),
            CODETRIAL_CONFIG_ENV=str(self.dummy_config()),
            PATH=f"{shim}:{os.environ['PATH']}",
            TMPDIR=str(tmp),
            PORT=None,
            INTERVIEW_ROOM_NAME=None,
            STAND_IN_PIDFILE=str(pidfile),
            REAL_NODE=shutil.which("node"),
            STAND_IN_CAPTURE=capture_text or self.good_capture(),
            STAND_IN_WORKER_EXIT=str(worker_exit),
            STAND_IN_WRITES_REPORT="1" if writes_report else None,
        )
        return result, report, capture

    def test_the_fixture_path_runs_from_start_to_a_judged_capture(self):
        result, report, capture = self.run_fixture_path()
        self.assertEqual(result.returncode, 0, result.stderr)
        # The report was kept with a private copy of the server's log.
        kept_report = json.loads(report.read_text())
        self.assertEqual(kept_report["outcome"], "ok")
        # Its drain and settle add up to the stop wait less the slack, and its
        # own deadline leaves the stop wait and the slack before the runner's
        # reap (an hour from the launch here), less what the launch used.
        launched = kept_report["launched_with"]
        self.assertEqual((launched["DRAIN_S"], launched["SETTLE_S"]), ("10", "10"))
        ceiling = 3600 - 40 - 20
        self.assertIn(int(launched["DEADLINE_S"]), range(ceiling - 60, ceiling + 1))
        log = Path(str(report) + ".server.log")
        self.assertIn("stand-in fixture stopping", log.read_text())
        self.assertEqual(log.stat().st_mode & 0o777, 0o600)
        # And judged after it stopped: the verdict is in the capture, the rest
        # of the report is as the worker left it.
        kept = json.loads(capture.read_text())["report"]
        self.assertEqual(kept["reply_content"], "failed", kept)
        self.assertEqual(kept["qualification"], "not_evaluated")
        self.assertEqual(kept["status"], "complete")
        self.assertIn("long-interview reply_content=failed", result.stdout)

    def test_a_judgment_never_turns_a_failed_run_green(self):
        failed, _, capture = self.run_fixture_path(worker_exit=1)
        self.assertNotEqual(failed.returncode, 0, failed.stderr)
        self.assertEqual(
            json.loads(capture.read_text())["report"]["reply_content"], "failed"
        )

    def test_a_judgment_that_cannot_run_leaves_a_good_run_good(self):
        result, _, capture = self.run_fixture_path(capture_text="not a capture")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("cannot read the capture", result.stderr)
        self.assertIn("run's own result stands", result.stderr)
        self.assertEqual(capture.read_text(), "not a capture")

    def test_a_fixture_that_wrote_no_report_fails_the_run_and_says_so(self):
        result, report, capture = self.run_fixture_path(writes_report=False)
        self.assertNotEqual(result.returncode, 0, result.stderr)
        self.assertIn("left no report", json.loads(report.read_text())["outcome"])
        kept = json.loads(capture.read_text())["report"]
        self.assertEqual(kept["reply_content"], "not_assessed", kept)

    def test_the_sdk_fixture_launcher_pins_its_test_name_through_the_shared_check(self):
        script = (ROOT / "scripts" / "live-sdk-fixture.sh").read_text()
        self.assertIn('fixture-server.sh" check "$bin" "$name"', script)


class FixtureServerScriptTests(Scratch):
    """scripts/fixture-server.sh against stand-in executables."""

    SCRIPT = ROOT / "scripts" / "fixture-server.sh"

    def run_script(self, *args, **env):
        environment = dict(os.environ)
        environment.update(env)
        return subprocess.run(
            ["sh", str(self.SCRIPT), *map(str, args)],
            env=environment,
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )

    def pin(self, listing):
        pin = self.root / "pin"
        support.write_executable(
            pin,
            '#!/bin/sh\nif [ "$1" = --list ] && [ "$2" = --ignored ]; then\n'
            f"cat <<'EOF'\n{listing}EOF\nfi\n",
        )
        return pin

    GOOD_REPORT = {
        "outcome": "ok",
        "bound": "127.0.0.1:1",
        "arming_failure": None,
        "capture": {
            "failure": None,
            "receipts": [],
            "batches": [],
            "replies": [],
            "boundaries": [],
        },
    }

    def keep(self, report, log="server output\n", before=None):
        """Runs `keep` over a stand-in fixture directory; returns the result and
        the path the report was kept at."""
        directory = Path(tempfile.mkdtemp(dir=self.root))
        if report is not None:
            (directory / "out.json").write_text(json.dumps(report))
        server_log = directory / "web.log"
        server_log.write_text(log)
        out = directory / "kept.json"
        if before:
            before(out)
        result = self.run_script("keep", directory, out, server_log)
        return result, out

    def test_keep_holds_a_good_report_and_a_private_copy_of_the_log(self):
        result, out = self.keep(self.GOOD_REPORT)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(out.read_text())["outcome"], "ok")
        # Both private: the report holds the interviewer's replies and the
        # tool results.
        self.assertEqual(out.stat().st_mode & 0o777, 0o600)
        kept = Path(str(out) + ".server.log")
        self.assertEqual(kept.read_text(), "server output\n")
        self.assertEqual(kept.stat().st_mode & 0o777, 0o600)

    def test_keep_never_replaces_a_file_already_at_the_report_path(self):
        # A capture written there by this run, or another run's report, is
        # left as it was and the keep fails.
        result, out = self.keep(
            self.GOOD_REPORT, before=lambda out: out.write_text("the capture\n")
        )
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(out.read_text(), "the capture\n")

    def test_keep_replaces_neither_a_log_nor_a_report_already_there(self):
        result, out = self.keep(
            self.GOOD_REPORT,
            before=lambda out: Path(str(out) + ".server.log").write_text("earlier\n"),
        )
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(Path(str(out) + ".server.log").read_text(), "earlier\n")
        result, out = self.keep(None, before=lambda out: out.write_text("earlier\n"))
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(out.read_text(), "earlier\n")
        self.assertIn("not this run's report", result.stderr)

    def test_keep_leaves_an_envelope_when_the_fixture_wrote_nothing(self):
        result, out = self.keep(None)
        self.assertEqual(result.returncode, 1, result.stderr)
        envelope = json.loads(out.read_text())
        self.assertEqual(out.stat().st_mode & 0o777, 0o600)
        self.assertIn("left no report", envelope["outcome"])
        self.assertIsNone(envelope["capture"])
        self.assertIn("server output", result.stderr)

    def test_keep_refuses_a_report_the_judge_could_not_read(self):
        reports = (
            {**self.GOOD_REPORT, "outcome": "shutdown bound"},
            {k: v for k, v in self.GOOD_REPORT.items() if k != "bound"},
            {**self.GOOD_REPORT, "capture": None},
        )
        for report in reports:
            with self.subTest(report=report):
                result, out = self.keep(report)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertTrue(out.exists())

    def test_keep_fails_when_the_log_cannot_be_kept(self):
        # A directory sits where the log copy has to go.
        result, _ = self.keep(
            self.GOOD_REPORT, before=lambda out: Path(str(out) + ".server.log").mkdir()
        )
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("could not keep the server log", result.stderr)

    def test_the_pin_must_list_the_test_exactly_once(self):
        pin = self.pin("a::one: test\nmodule::serve: test\n")
        self.assertEqual(self.run_script("check", pin, "module::serve").returncode, 0)
        rows = [
            ("module::missing", "lists 0"),
            ("serve", "lists 0"),
        ]
        for name, expected in rows:
            with self.subTest(name=name):
                result = self.run_script("check", pin, name)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn(expected, result.stderr)
        twice = self.pin("module::serve: test\nmodule::serve: test\n")
        result = self.run_script("check", twice, "module::serve")
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn("lists 2", result.stderr)

    def test_a_pin_that_is_not_runnable_or_a_name_that_is_not_one_name_is_refused(self):
        plain = self.root / "plain"
        plain.write_text("not executable")
        failing = self.root / "failing"
        support.write_executable(failing, "#!/bin/sh\nexit 1\n")
        for args in [
            (plain, "module::serve"),
            (failing, "module::serve"),
            (self.pin("module::serve: test\n"), ""),
            (self.pin("module::serve: test\n"), "two names"),
        ]:
            with self.subTest(args=args):
                self.assertEqual(self.run_script("check", *args).returncode, 2)

    def test_a_fixture_that_stops_on_request_is_waited_for_and_leaves_its_report(self):
        fixture = self.root / "fixture"
        support.write_executable(
            fixture,
            '#!/bin/sh\nwhile [ ! -e "$1/stop" ]; do sleep 0.1; done\n'
            'sleep 1\necho \'{"outcome":"ok"}\' > "$1/out.json"\n',
        )
        directory = self.root / "fixture-dir"
        directory.mkdir()
        process = subprocess.Popen([str(fixture), str(directory)])
        self.spawned.append(process.pid)
        result = self.run_script("stop", directory, process.pid, 20)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((directory / "stop").exists())
        # It finished its own work first: the stop did not cut it short. The
        # process is still unreaped here, which the script must not mistake for
        # running.
        self.assertEqual(
            (directory / "out.json").read_text().strip(), '{"outcome":"ok"}'
        )
        self.assertEqual(process.wait(timeout=10), 0)

    def test_a_fixture_that_will_not_stop_is_terminated_and_that_is_a_failure(self):
        stubborn = self.root / "stubborn"
        support.write_executable(
            stubborn, "#!/bin/sh\ntrap '' TERM\nwhile :; do sleep 1; done\n"
        )
        directory = self.root / "stubborn-dir"
        directory.mkdir()
        process = subprocess.Popen([str(stubborn)])
        self.spawned.append(process.pid)
        result = self.run_script(
            "stop", directory, process.pid, 0, FIXTURE_SERVER_TERM_GRACE_S="1"
        )
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("had not stopped after 0s", result.stderr)
        process.wait(timeout=10)
        self.assertIsNotNone(process.returncode)

    def test_a_stop_file_that_cannot_be_written_falls_back_to_a_bounded_term(self):
        # TERM is a stop request to the fixture: it finishes and exits cleanly
        # inside the wait, past the kill grace, so only the wait lets it.
        fixture = self.root / "on-term"
        support.write_executable(
            fixture,
            "#!/bin/sh\ntrap 'sleep 2; exit 0' TERM\nwhile :; do sleep 0.1; done\n",
        )
        process = subprocess.Popen([str(fixture)])
        self.spawned.append(process.pid)
        time.sleep(0.5)
        started = time.monotonic()
        result = self.run_script(
            "stop",
            self.root / "no-such-dir",
            process.pid,
            20,
            FIXTURE_SERVER_TERM_GRACE_S="1",
        )
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("could not write the stop file", result.stderr)
        self.assertEqual(process.wait(timeout=10), 0, "it was asked, not killed")
        self.assertLess(time.monotonic() - started, 15)

    def test_a_padded_zombie_state_from_ps_still_counts_as_exited(self):
        # A system that pads the state column (" Z") must not leave the stop
        # waiting out its whole limit for a process that has already exited.
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        support.write_executable(bin_dir / "ps", "#!/bin/sh\necho ' Z+'\n")
        sleeper = subprocess.Popen(["sleep", "300"])
        self.spawned.append(sleeper.pid)
        directory = self.root / "padded-dir"
        directory.mkdir()
        started = time.monotonic()
        result = self.run_script(
            "stop",
            directory,
            sleeper.pid,
            30,
            PATH=f"{bin_dir}{os.pathsep}{os.environ['PATH']}",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertLess(time.monotonic() - started, 10)

    def test_stop_refuses_a_pid_or_wait_that_is_not_a_number(self):
        for args in [
            ("stop", self.root, "abc", 1),
            ("stop", self.root, 0, 1),
            ("stop", self.root, 1, 1),
            ("stop", self.root, "007", 1),
            ("stop", self.root, 1, "soon"),
            ("stop", self.root),
            ("bogus",),
        ]:
            with self.subTest(args=args):
                self.assertEqual(self.run_script(*args).returncode, 2)


class WorkerGuardNodeTests(Scratch):
    """scripts/run-budget-guard.cjs against the real Python permit service."""

    def setUp(self):
        super().setUp()
        self.path = self.write_manifest()
        raw = self.path.read_bytes()
        self.digest = irb.manifest_digest(raw)
        self.service = irb.PermitService(
            json.loads(raw), self.digest, str(self.root / "ledger")
        )
        self.socket_path = str(self.root / "p.sock")
        self.service.start(self.socket_path)
        self.addCleanup(self.service.stop)

    def drive(self, *ids, **env):
        environment = {
            k: v for k, v in os.environ.items() if not k.startswith("BROWSER_CHECK_")
        }
        environment.update(
            IRB_ROOT=str(ROOT),
            BROWSER_CHECK_RUN_MANIFEST=str(self.path),
            BROWSER_CHECK_RUN_ID="run-test-1",
            BROWSER_CHECK_RUN_MANIFEST_SHA256=self.digest,
            BROWSER_CHECK_BUDGET_SOCKET=self.socket_path,
        )
        environment.update(env)
        environment = {k: v for k, v in environment.items() if v is not None}
        result = subprocess.run(
            ["node", str(DRIVER), *ids],
            env=environment,
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
        return result, json.loads(result.stdout)

    def test_a_second_process_continues_the_count_of_the_first(self):
        _, first = self.drive("candidate_join", "mic_1")
        self.assertEqual(first["granted"], ["candidate_join", "mic_1"])
        _, reloaded = self.drive("edit_1", "board_1", "end")
        self.assertEqual(reloaded["granted"], ["edit_1", "board_1", "end"])
        self.assertEqual(self.service.count(), 5)

    def test_a_denied_action_is_not_dispatched_and_kills_the_guard(self):
        _, out = self.drive("candidate_join", "candidate_join", "mic_1")
        self.assertEqual(out["granted"], ["candidate_join"])
        self.assertEqual(out["dispatched"], ["candidate_join"])
        self.assertEqual(out["denied"]["candidate_join"], "duplicate_action")
        self.assertIn("guard is dead", out["denied"]["mic_1"])
        self.assertEqual(self.service.count(), 1)

    def test_a_manifest_the_launcher_did_not_hash_ends_the_worker_before_any_ask(self):
        for env, expected in (
            ({"BROWSER_CHECK_RUN_MANIFEST_SHA256": HEX}, "hash does not match"),
            ({"BROWSER_CHECK_RUN_ID": "other"}, "run ID does not match"),
            ({"BROWSER_CHECK_BUDGET_SOCKET": None}, "incomplete"),
            (
                {"BROWSER_CHECK_RUN_MANIFEST": "/nonexistent/manifest.json"},
                "cannot read the run manifest",
            ),
        ):
            with self.subTest(expected=expected):
                result, out = self.drive("candidate_join", **env)
                self.assertEqual(result.returncode, 2)
                self.assertIn(expected, out["refused"])
        self.assertEqual(self.service.count(), 0)

    def test_without_a_budget_the_guard_is_absent(self):
        _, out = self.drive(
            BROWSER_CHECK_RUN_MANIFEST=None,
            BROWSER_CHECK_RUN_ID=None,
            BROWSER_CHECK_RUN_MANIFEST_SHA256=None,
            BROWSER_CHECK_BUDGET_SOCKET=None,
        )
        self.assertEqual(out, {"budget": None})

    def test_overlapping_asks_are_serialized_not_sent_with_one_sequence(self):
        _, out = self.drive(
            "candidate_join", "mic_1", "edit_1", "board_1", DRIVE_CONCURRENT="1"
        )
        self.assertEqual(out["denied"], {})
        self.assertEqual(sorted(out["granted"]), sorted(ACTIONS))
        self.assertEqual(self.service.count(), 4)

    def test_a_silent_launcher_denies_within_the_timeout(self):
        stub = str(self.root / "silent.sock")
        server = StubServer(stub, lambda conn, request: None)
        self.addCleanup(server.stop)
        started = time.monotonic()
        # The stub answers the status call, then holds the permit request.
        _, out = self.drive("candidate_join", BROWSER_CHECK_BUDGET_SOCKET=stub)
        self.assertLess(time.monotonic() - started, 5)
        self.assertIn("candidate_join", out["denied"])
        self.assertEqual(out["dispatched"], [])
        self.assertEqual(server.connections, 1)


class FakeProc:
    """A /proc tree in a scratch directory: stat, status and fd per process."""

    def __init__(self, root):
        self.root = Path(root)

    def add(self, pid, start, rss_kb=None, threads=None, fds=None):
        base = self.root / str(pid)
        base.mkdir(parents=True, exist_ok=True)
        # pid (comm) state ppid pgid ... field 22 is the start time
        fields = ["S", "1", "1"] + ["0"] * 16 + [str(start)] + ["0"] * 5
        (base / "stat").write_text(f"{pid} (worker) " + " ".join(fields) + "\n")
        lines = ["Name:\tworker"]
        if threads is not None:
            lines.append(f"Threads:\t{threads}")
        if rss_kb is not None:
            lines.append(f"VmRSS:\t{rss_kb} kB")
        (base / "status").write_text("\n".join(lines) + "\n")
        if fds is not None:
            (base / "fd").mkdir(exist_ok=True)
            for number in range(fds):
                (base / "fd" / str(number)).write_text("")


class FakeTree:
    def __init__(self, owned):
        self.owned = dict(owned)

    def alive(self, table):
        return sorted(self.owned)


class ResourceReadTests(Scratch):
    def proc(self):
        return FakeProc(self.root / "proc")

    def test_a_process_is_read_for_rss_threads_and_descriptors(self):
        self.proc().add(41, 900, rss_kb=2048, threads=6, fds=5)
        got = irb.read_process_resources(41, 900, str(self.root / "proc"))
        self.assertEqual(got, {"rss_kb": 2048, "fds": 5, "threads": 6, "reused": False})

    # Expected: every value that cannot be read is None. A mutant that turns a
    # missing value into 0 changes these to 0 and fails.
    def test_a_missing_value_is_unknown_never_zero(self):
        self.proc().add(42, 900)
        got = irb.read_process_resources(42, 900, str(self.root / "proc"))
        self.assertEqual(
            got, {"rss_kb": None, "fds": None, "threads": None, "reused": False}
        )
        gone = irb.read_process_resources(43, 900, str(self.root / "proc"))
        self.assertEqual(
            gone, {"rss_kb": None, "fds": None, "threads": None, "reused": False}
        )

    def test_a_reused_pid_is_another_process_and_reads_as_unknown(self):
        self.proc().add(44, 777, rss_kb=9999, threads=9, fds=9)
        got = irb.read_process_resources(44, 900, str(self.root / "proc"))
        self.assertTrue(got["reused"])
        self.assertEqual((got["rss_kb"], got["fds"], got["threads"]), (None,) * 3)

    def test_a_pid_reused_between_the_reads_is_unknown_too(self):
        self.proc().add(45, 900, rss_kb=10, threads=1, fds=1)
        starts = iter([900, 901])
        real = irb.parse_proc_stat

        def parse(text):
            pid, state, ppid, pgid, start = real(text)
            # Tests share this module across jobs: only the pid under test
            # sees the scripted start times.
            if pid != 45:
                return pid, state, ppid, pgid, start
            return pid, state, ppid, pgid, next(starts)

        irb.parse_proc_stat = parse
        try:
            got = irb.read_process_resources(45, 900, str(self.root / "proc"))
        finally:
            irb.parse_proc_stat = real
        self.assertTrue(got["reused"])
        self.assertIsNone(got["rss_kb"])


class ResourceSamplerTests(Scratch):
    def sampler(self, **kwargs):
        self.proc = FakeProc(self.root / "proc")
        return irb.ResourceSampler(
            str(self.root / "samples.jsonl"),
            kwargs.pop("arm", "candidate"),
            proc_root=str(self.root / "proc"),
            **kwargs,
        )

    def lines(self):
        text = (self.root / "samples.jsonl").read_text()
        return [json.loads(line) for line in text.splitlines()]

    def test_totals_cover_the_values_read_and_count_the_ones_that_were_not(self):
        sampler = self.sampler(wall_cap_s=60)
        self.proc.add(10, 1, rss_kb=100, threads=2, fds=3)
        self.proc.add(11, 2, rss_kb=50, threads=1)
        self.proc.add(12, 3)
        record = sampler.sample(1.0, FakeTree({10: 1, 11: 2, 12: 3}), {})
        self.assertEqual(record["tree"], {"rss_kb": 150, "fds": 3, "threads": 3})
        self.assertEqual(record["tree_unknown"], {"rss_kb": 1, "fds": 2, "threads": 1})
        self.assertEqual(record["arm"], "candidate")
        self.assertEqual(record["processes"], 3)

    def test_a_tree_with_nothing_readable_has_null_totals_not_zero(self):
        sampler = self.sampler(wall_cap_s=60)
        self.proc.add(10, 1)
        record = sampler.sample(1.0, FakeTree({10: 1}), {})
        self.assertEqual(record["tree"], {"rss_kb": None, "fds": None, "threads": None})

    def test_rows_carry_the_start_time_so_a_reused_pid_is_another_row(self):
        sampler = self.sampler(wall_cap_s=60)
        self.proc.add(10, 1, rss_kb=100, threads=1, fds=1)
        first = sampler.sample(1.0, FakeTree({10: 1}), {})
        self.proc.add(10, 5, rss_kb=7, threads=1, fds=1)
        second = sampler.sample(31.0, FakeTree({10: 5}), {})
        self.assertEqual((first["rows"][0]["pid"], first["rows"][0]["start"]), (10, 1))
        self.assertEqual(
            (second["rows"][0]["pid"], second["rows"][0]["start"]), (10, 5)
        )

    def test_each_sample_is_on_disk_before_the_next_is_taken(self):
        sampler = self.sampler(wall_cap_s=300)
        self.proc.add(10, 1, rss_kb=1, threads=1, fds=1)
        for count in (1, 2, 3):
            sampler.sample(count * 30.0, FakeTree({10: 1}), {})
            self.assertEqual(len(self.lines()), count)
        self.assertEqual([r["seq"] for r in self.lines()], [0, 1, 2])
        self.assertEqual(self.lines()[2]["gap_s"], 30.0)

    # Expected: the third sample is refused and the first two stay. A sampler
    # that evicted the oldest to make room would hold [1, 2] and no overflow.
    def test_the_sample_count_is_bounded_and_never_evicts(self):
        sampler = self.sampler(max_samples=2, max_bytes=10_000_000)
        self.proc.add(10, 1, rss_kb=1, threads=1, fds=1)
        for at in (1.0, 31.0, 61.0):
            sampler.sample(at, FakeTree({10: 1}), {})
        self.assertEqual(sampler.overflow, "max_samples")
        self.assertEqual([r["seq"] for r in self.lines()], [0, 1])

    def test_the_byte_size_is_bounded_and_never_evicts(self):
        sampler = self.sampler(max_samples=100, max_bytes=700)
        self.proc.add(10, 1, rss_kb=1, threads=1, fds=1)
        for at in range(1, 8):
            sampler.sample(float(at), FakeTree({10: 1}), {})
        self.assertEqual(sampler.overflow, "max_bytes")
        written = (self.root / "samples.jsonl").stat().st_size
        self.assertLessEqual(written, 700)
        self.assertEqual(written, sampler.bytes)
        self.assertEqual(self.lines()[0]["seq"], 0)

    def test_a_large_tree_keeps_a_bounded_line_and_says_what_it_left_out(self):
        sampler = self.sampler(wall_cap_s=60)
        owned = {}
        for pid in range(100, 140):
            self.proc.add(pid, pid, rss_kb=1, threads=1, fds=1)
            owned[pid] = pid
        record = sampler.sample(1.0, FakeTree(owned), {})
        self.assertEqual(len(record["rows"]) + record["rows_omitted"], 40)
        self.assertEqual(record["tree"]["rss_kb"], 40)
        line = (self.root / "samples.jsonl").read_text()
        self.assertLessEqual(len(line), irb.MAX_SAMPLE_LINE_BYTES)

    def test_the_cadence_is_thirty_seconds_and_a_late_tick_is_not_a_burst(self):
        sampler = self.sampler(wall_cap_s=300)
        self.assertEqual(irb.RESOURCE_CADENCE_S, 30)
        self.proc.add(10, 1, rss_kb=1, threads=1, fds=1)
        self.assertTrue(sampler.due(0.0))
        sampler.sample(0.0, FakeTree({10: 1}), {})
        self.assertFalse(sampler.due(29.9))
        self.assertTrue(sampler.due(30.0))
        sampler.sample(95.0, FakeTree({10: 1}), {})
        self.assertFalse(sampler.due(120.0))
        self.assertTrue(sampler.due(125.0))


class ArchiveBoundTests(Scratch):
    def errors(self, **overrides):
        return irb.validate_manifest(self.manifest(**overrides), min_reserve=1)

    def test_the_bound_is_derived_from_the_wall_cap_and_the_cadence(self):
        self.assertEqual(irb.max_resource_samples(30), 3)
        self.assertEqual(irb.max_resource_samples(5760), 194)
        floor, ceiling = irb.archive_bounds(5760)
        self.assertEqual(
            floor, 194 * irb.MAX_SAMPLE_LINE_BYTES + irb.CONTROL_FILES_ALLOWANCE_BYTES
        )
        self.assertEqual(ceiling - floor, irb.ARCHIVE_EVIDENCE_ALLOWANCE_BYTES)
        faster = irb.archive_bounds(5760, 15)
        self.assertGreater(faster[0], floor)

    def test_a_long_interview_manifest_must_carry_its_archive_bound(self):
        _, ceiling = irb.archive_bounds(30)
        self.assertEqual(self.errors(duration_min=10, max_archive_bytes=ceiling), [])
        joined = "; ".join(self.errors(duration_min=10))
        self.assertIn("duration_min needs one", joined)
        # A manifest that is not a long interview needs none.
        self.assertEqual(self.errors(), [])

    def test_max_archive_bytes_must_sit_between_its_floor_and_ceiling(self):
        floor, ceiling = irb.archive_bounds(30)
        self.assertEqual(self.errors(max_archive_bytes=floor), [])
        self.assertEqual(self.errors(max_archive_bytes=ceiling), [])
        for value, reason in (
            (floor - 1, "below"),
            (ceiling + 1, "above"),
            (0, "positive integer"),
            ("big", "positive integer"),
            (True, "positive integer"),
        ):
            joined = "; ".join(self.errors(max_archive_bytes=value))
            self.assertIn(reason, joined, value)
            self.assertIn("max_archive_bytes", joined, value)


class ResourceRunTests(Scratch):
    def fast(self):
        return ("--resource-cadence", "0.1")

    def samples(self):
        text = (self.out / irb.RESOURCE_SAMPLES_FILE).read_text()
        return [json.loads(line) for line in text.splitlines()]

    def test_a_run_appends_samples_as_it_goes_for_the_owned_tree(self):
        result, _ = self.launch("linger", extra=self.fast())
        self.assertEqual(result.returncode, 0, result.stderr)
        samples = self.samples()
        self.assertGreaterEqual(len(samples), 3)
        self.assertEqual({s["arm"] for s in samples}, {"candidate"})
        self.assertEqual([s["seq"] for s in samples], list(range(len(samples))))
        # The worker is alive for the first sample; one taken after it exited
        # sees no process and has unknown totals, not zero ones.
        self.assertGreaterEqual(samples[0]["processes"], 1)
        for sample in samples:
            if sample["processes"] == 0:
                self.assertEqual(
                    sample["tree"], {"rss_kb": None, "fds": None, "threads": None}
                )
        self.assertTrue(any(s["tree"]["rss_kb"] for s in samples))
        report = self.result()["resources"]
        self.assertEqual(report["samples"], len(samples))
        self.assertIsNone(report["overflow"])
        self.assertEqual(report["cadence_s"], 0.1)

    def test_both_arms_are_sampled_and_labelled(self):
        for arm in ("candidate", "baseline"):
            self.out = self.root / f"out-{arm}"
            result, _ = self.launch("linger", extra=(*self.fast(), "--arm", arm))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual({s["arm"] for s in self.samples()}, {arm})

    # Expected: the sampler's own file fits, a fat worker file does not. The
    # run stops, exits 6, writes incomplete_artifact.json and is not reported
    # as a spent budget.
    def test_an_archive_past_its_bound_is_an_incomplete_artifact(self):
        floor, _ = irb.archive_bounds(30)
        result, _ = self.launch("fat", extra=self.fast(), max_archive_bytes=floor)
        self.assertEqual(result.returncode, irb.EXIT_INCOMPLETE_ARTIFACT, result.stderr)
        report = self.result()
        self.assertEqual(report["status"], "incomplete_artifact")
        self.assertTrue((self.out / "incomplete_artifact.json").exists())
        self.assertIsNone(report["budget_failure"])
        self.assertEqual(report["archive"]["artifact_failure"], "archive_bytes")
        self.assertEqual(report["archive"]["max_archive_bytes"], floor)
        self.assertGreater(report["archive"]["bytes"], floor)
        self.assertTrue((self.out / "fat.bin").exists())
        self.assertFalse((self.out / "incomplete_budget.json").exists())
        # The overflow stopped the run while it was going. A bound checked only
        # after the worker had finished would leave the stop file unwritten and
        # let the worker spend its whole wait.
        self.assertTrue((self.out / "budget-stop").exists())

    # Expected: the artifact failure stopped the run first, and the rogue
    # dispatch that followed is still recorded as the budget failure. The exit
    # is the artifact's (6, which outranks 4) and both files exist.
    def test_a_budget_failure_after_an_artifact_failure_is_not_lost(self):
        floor, _ = irb.archive_bounds(30)
        result, _ = self.launch("fat_rogue", extra=self.fast(), max_archive_bytes=floor)
        self.assertEqual(result.returncode, irb.EXIT_INCOMPLETE_ARTIFACT, result.stderr)
        report = self.result()
        self.assertEqual(report["status"], "incomplete_artifact")
        self.assertEqual(report["archive"]["artifact_failure"], "archive_bytes")
        self.assertEqual(report["budget_failure"], "dispatch_without_permit")
        self.assertTrue((self.out / "incomplete_artifact.json").exists())
        self.assertTrue((self.out / "incomplete_budget.json").exists())

    def test_a_zero_or_negative_resource_cadence_is_refused_before_launch(self):
        for value in ("0", "-5"):
            self.out = self.root / f"out{value}"
            result, _ = self.launch("linger", extra=("--resource-cadence", value))
            self.assertEqual(result.returncode, irb.EXIT_PREFLIGHT, result.stderr)
            self.assertIn("--resource-cadence must be positive", result.stderr)
            self.assertFalse((self.out / irb.RESOURCE_SAMPLES_FILE).exists())

    def test_an_archive_within_its_bound_completes(self):
        _, ceiling = irb.archive_bounds(30)
        result, _ = self.launch("fat", extra=self.fast(), max_archive_bytes=ceiling)
        self.assertNotEqual(result.returncode, irb.EXIT_INCOMPLETE_ARTIFACT)
        self.assertFalse((self.out / "incomplete_artifact.json").exists())
        self.assertEqual(self.result()["archive"]["artifact_failure"], None)

    def test_a_rerun_into_a_directory_holding_samples_is_refused(self):
        self.out.mkdir()
        (self.out / irb.RESOURCE_SAMPLES_FILE).write_text("")
        result, _ = self.launch("linger")
        self.assertEqual(result.returncode, irb.EXIT_PREFLIGHT)
        self.assertIn(irb.RESOURCE_SAMPLES_FILE, result.stderr)


class JoinHashLimitTests(Scratch):
    """The join permit's re-hash limit: default, bounded override, fail closed."""

    class Args:
        sample_interval = 0.5
        reap_budget = 1.5
        resource_cadence = 30.0

    def args(self, value=None):
        namespace = type("Args", (self.Args,), {})
        if value is not None:
            namespace.max_join_hash_s = value
        return namespace

    def preflight(self, args, seconds):
        """Preflight over a verifier that reports `seconds` of hashing."""
        return irb.preflight(
            self.manifest(), args, None, verify=lambda manifest: seconds
        )

    def test_the_default_limit_and_the_ceiling_arithmetic_are_pinned(self):
        self.assertEqual(irb.MAX_JOIN_HASH_S, 0.5)
        self.assertEqual(irb.PERMIT_TIMEOUT_S, 1.0)
        self.assertEqual(irb.JOIN_HANDLING_MARGIN_S, 0.3)
        # The hash at the ceiling plus the margin is exactly the client timeout.
        self.assertAlmostEqual(
            irb.MAX_JOIN_HASH_CEILING_S + irb.JOIN_HANDLING_MARGIN_S,
            irb.PERMIT_TIMEOUT_S,
        )
        self.assertAlmostEqual(irb.MAX_JOIN_HASH_CEILING_S, 0.7)
        self.assertLess(irb.MAX_JOIN_HASH_S, irb.MAX_JOIN_HASH_CEILING_S)

    def test_a_run_without_the_option_records_no_override(self):
        result, _ = self.launch("normal")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("overrides", self.result())

    def test_preflight_uses_the_default_limit_when_no_override_is_given(self):
        errors = self.preflight(self.args(), 0.6)
        self.assertIn("0.5s the join permit", "; ".join(errors))
        self.assertEqual(self.preflight(self.args(), 0.45), [])

    def test_an_explicit_override_admits_a_slower_hash_up_to_the_ceiling(self):
        self.assertEqual(self.preflight(self.args(0.7), 0.65), [])
        self.assertEqual(self.preflight(self.args(0.65), 0.65), [])
        self.assertTrue(self.preflight(self.args(0.6), 0.65))
        errors = self.preflight(self.args(0.7), 0.71)
        self.assertIn("0.7s the join permit", "; ".join(errors))

    def test_an_override_is_recorded_in_the_result(self):
        result, _ = self.launch("normal", extra=("--max-join-hash-s", "0.7"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.result()["overrides"], {"max_join_hash_s": 0.7})

    def test_an_override_equal_to_the_default_is_not_recorded(self):
        result, _ = self.launch("normal", extra=("--max-join-hash-s", "0.5"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("overrides", self.result())

    def test_values_over_the_ceiling_or_not_positive_numbers_are_refused(self):
        rows = {
            "just over the ceiling": ("0.71", "ceiling"),
            "the whole permit timeout": ("1.0", "ceiling"),
            "infinity": ("inf", "positive, finite"),
            "not a number": ("nan", "positive, finite"),
            "zero": ("0", "positive, finite"),
            "negative": ("-0.2", "positive, finite"),
        }
        for name, (value, expected) in rows.items():
            with self.subTest(name):
                self.out = self.root / ("out-" + str(abs(hash(name))))
                result, _ = self.launch("normal", extra=("--max-join-hash-s=" + value,))
                self.assertEqual(result.returncode, irb.EXIT_PREFLIGHT, result.stderr)
                self.assertIn(expected, result.stderr)
                self.assertFalse((self.out / "budget-ledger.jsonl").exists())
                self.assertFalse((self.fake / "pids").exists())
                self.assertFalse((self.out / "budget-result.json").exists())

    SLOW_LAUNCHER = """
import importlib.util, sys, time
spec = importlib.util.spec_from_file_location("irb", sys.argv[1])
irb = importlib.util.module_from_spec(spec)
sys.modules["irb"] = irb
spec.loader.exec_module(irb)
real = irb.file_sha256
def slow(path):
    time.sleep(0.6)
    return real(path)
original = irb.PermitService.__init__
def init(self, *args, **kwargs):
    kwargs["hasher"] = slow
    original(self, *args, **kwargs)
irb.PermitService.__init__ = init
sys.exit(irb.main(sys.argv[2:]))
"""

    def launch_slow(self, *extra):
        wrapper = self.root / "slow_launcher.py"
        wrapper.write_text(self.SLOW_LAUNCHER)
        environment = dict(os.environ)
        environment.update(
            FAKE_MODE="normal", FAKE_DIR=str(self.fake), IRB_MODULE=str(TOOL)
        )
        return subprocess.run(
            [
                sys.executable,
                str(wrapper),
                str(TOOL),
                "run",
                "--manifest",
                str(self.write_manifest()),
                "--artifact-dir",
                str(self.out),
                "--allow-short-reserve",
                "1",
                "--sample-interval",
                "0.05",
                "--reap-budget",
                "1.5",
                *extra,
                "--",
                sys.executable,
                str(WORKER),
            ],
            env=environment,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=90,
            check=False,
        )

    def test_the_launcher_hands_the_override_to_the_join_permit(self):
        # A 0.6 s re-hash at the join: over the default 0.5, under 0.7.
        loose = self.launch_slow("--max-join-hash-s", "0.7")
        self.assertEqual(loose.returncode, 0, loose.stderr)
        self.assertIn("candidate_join", self.grants())
        self.out = self.root / "out-default"
        strict = self.launch_slow()
        self.assertNotEqual(strict.returncode, 0, strict.stderr)
        self.assertNotIn("candidate_join", self.grants())
        self.assertIn(
            "join_hash_too_slow", (self.out / "budget-ledger.jsonl").read_text()
        )

    def test_a_non_numeric_value_is_refused_by_the_parser(self):
        result, _ = self.launch("normal", extra=("--max-join-hash-s", "soon"))
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn("invalid float value", result.stderr)
        self.assertFalse((self.fake / "pids").exists())


class JoinPermitHashTests(Scratch):
    """What the join permit does with the hash, with a scripted hasher."""

    def build(self, hasher, limit=irb.MAX_JOIN_HASH_CEILING_S, clock=None):
        manifest = self.manifest()
        manifest["binaries"]["fixture"] = dict(manifest["binaries"]["candidate"])
        self.pin = manifest["binaries"]["candidate"]["sha256"]
        self.calls = []
        self.ledger = str(self.root / ("ledger-%d" % id(hasher)))

        def counting(path):
            self.calls.append(path)
            return hasher(path)

        kwargs = {"clock": clock} if clock else {}
        service = irb.PermitService(
            manifest,
            HEX,
            self.ledger,
            verify_join_binaries=True,
            join_hash_limit_s=limit,
            hasher=counting,
            **kwargs,
        )
        return manifest, service

    def ask(self, service, action_id, seq):
        return service.decide(
            {
                "v": 1,
                "op": "permit",
                "run_id": "run-test-1",
                "manifest_sha256": HEX,
                "action_id": action_id,
                "seq": seq,
            }
        )

    def test_each_pinned_binary_is_hashed_once_at_the_join_permit(self):
        manifest, service = self.build(lambda path: self.pin)
        self.assertTrue(self.ask(service, "candidate_join", 1)["granted"])
        self.assertEqual(len(manifest["binaries"]), 2)
        self.assertEqual(len(self.calls), 2)
        self.assertTrue(self.ask(service, "mic_1", 2)["granted"])
        self.assertEqual(len(self.calls), 2)

    def test_a_hash_exactly_at_the_limit_is_granted_and_over_it_fails_closed(self):
        now = [0.0]

        def slow(seconds):
            def hasher(path):
                now[0] += seconds / 2
                return self.pin

            return hasher

        _, at_limit = self.build(slow(0.7), limit=0.7, clock=lambda: now[0])
        self.assertTrue(self.ask(at_limit, "candidate_join", 1)["granted"])

        _, over = self.build(slow(0.71), limit=0.7, clock=lambda: now[0])
        reply = self.ask(over, "candidate_join", 1)
        self.assertEqual(
            (reply["granted"], reply["reason"]), (False, "join_hash_too_slow")
        )
        self.assertEqual(over.count(), 0)
        self.assertEqual(over.phase, "draining")
        self.assertEqual(over.fault, "join_hash_too_slow")
        self.assertFalse(self.ask(over, "candidate_join", 1)["granted"])

    def serve(self, hasher):
        _, service = self.build(hasher)
        self.socket_path = str(self.root / "p.sock")
        service.start(self.socket_path)
        self.addCleanup(service.stop)
        return service

    def test_a_join_permit_at_the_ceiling_still_answers_inside_the_client_timeout(self):
        # Two binaries sleeping 0.3 s each: 0.6 s of real hashing, under the
        # 0.7 s ceiling, against the client's real 1 s deadline.
        def hasher(path):
            time.sleep(0.3)
            return self.pin

        service = self.serve(hasher)
        guard = irb.WorkerGuard(self.socket_path, "run-test-1", HEX)
        started = time.monotonic()
        guard.permit("candidate_join")
        elapsed = time.monotonic() - started
        self.assertGreaterEqual(elapsed, 0.6)
        self.assertLess(elapsed, irb.PERMIT_TIMEOUT_S)
        self.assertEqual(service.count(), 1)
        self.assertIsNone(guard.dead)

    def test_a_join_permit_over_the_ceiling_is_denied_not_granted_late(self):
        def hasher(path):
            time.sleep(0.4)
            return self.pin

        service = self.serve(hasher)
        guard = irb.WorkerGuard(self.socket_path, "run-test-1", HEX)
        started = time.monotonic()
        with self.assertRaises(irb.PermitDenied) as caught:
            guard.permit("candidate_join")
        self.assertIn("join_hash_too_slow", str(caught.exception))
        self.assertLess(time.monotonic() - started, irb.PERMIT_TIMEOUT_S)
        self.assertEqual(service.count(), 0)
        self.assertNotIn('"grant"', Path(self.ledger).read_text())


if __name__ == "__main__":
    unittest.main()
