#!/usr/bin/env python3
"""A scripted stand-in for a live worker, driven by FAKE_MODE.

It speaks the real permit protocol through the controller's own WorkerGuard and
dispatches nothing but lines in FAKE_DIR/dispatch.log, so a test can compare
what was dispatched with what was granted. No network, no browser, no model.
"""

import importlib.util
import json
import os
import signal
import socket
import subprocess
import sys
import threading
import time

MODE = os.environ["FAKE_MODE"]
OUT = os.environ["FAKE_DIR"]

spec = importlib.util.spec_from_file_location("irb", os.environ["IRB_MODULE"])
irb = importlib.util.module_from_spec(spec)
sys.modules["irb"] = irb
spec.loader.exec_module(irb)


def note(name, text):
    with open(os.path.join(OUT, name), "a") as handle:
        handle.write(text + "\n")


def telemetry(**event):
    with open(os.environ["BROWSER_CHECK_BUDGET_TELEMETRY"], "a") as handle:
        handle.write(json.dumps(event) + "\n")


def dispatch(action_id):
    note("dispatch.log", action_id)
    telemetry(event="dispatched", action_id=action_id)


def manifest_actions():
    with open(os.environ["BROWSER_CHECK_RUN_MANIFEST"]) as handle:
        return json.load(handle)["actions"]


def spawn_stubborn_descendant():
    """A grandchild that ignores TERM and has left the process group."""
    code = (
        "import os, signal, time;"
        "signal.signal(signal.SIGTERM, signal.SIG_IGN);"
        "os.setsid();"
        f"open({os.path.join(OUT, 'grandchild.pid')!r}, 'w').write(str(os.getpid()));"
        "time.sleep(300)"
    )
    subprocess.Popen([sys.executable, "-c", code], stdin=subprocess.DEVNULL)


def wait_for_file(path, seconds=10):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline and not os.path.exists(path):
        time.sleep(0.02)


def raw_permit_request(guard, action_id, seq):
    return {
        "v": 1,
        "op": "permit",
        "run_id": guard.run_id,
        "manifest_sha256": guard.manifest_sha256,
        "action_id": action_id,
        "seq": seq,
    }


if os.environ.get("FAKE_IGNORE_TERM"):
    signal.signal(signal.SIGTERM, signal.SIG_IGN)

note("pids", str(os.getpid()))
telemetry(event="hb")
guard = irb.WorkerGuard.from_env()
stop = os.environ["BROWSER_CHECK_BUDGET_STOP"]


def run(action_id):
    guard.run_action(action_id, lambda: dispatch(action_id))


try:
    if MODE == "normal":
        telemetry(event="hb")
        if os.environ.get("FAKE_OBSERVED"):
            telemetry(event="observed", name="model_turns", value=7)
        for action in manifest_actions():
            run(action)
        run("end")
    elif MODE == "linger":
        # Stays up long enough for the fast-cadence sampler to take several
        # samples, then finishes the run normally.
        telemetry(event="hb")
        until = time.monotonic() + float(os.environ.get("FAKE_LINGER_S", "1.2"))
        while time.monotonic() < until:
            telemetry(event="hb")
            time.sleep(0.05)
        for action in manifest_actions():
            run(action)
        run("end")
    elif MODE == "fat":
        # Fills the archive past the manifest's bound, then waits to be stopped.
        run(manifest_actions()[0])
        artifacts = os.path.dirname(os.environ["BROWSER_CHECK_BUDGET_TELEMETRY"])
        with open(os.path.join(artifacts, "fat.bin"), "wb") as handle:
            handle.write(b"x" * int(os.environ.get("FAKE_FAT_BYTES", "600000")))
        until = time.monotonic() + 20
        while not os.path.exists(stop) and time.monotonic() < until:
            telemetry(event="hb")
            time.sleep(0.05)
        run("end")
    elif MODE == "fat_rogue":
        # Fills the archive, and once stopped for it dispatches something the
        # ledger never granted: a budget failure that follows the artifact one.
        run(manifest_actions()[0])
        artifacts = os.path.dirname(os.environ["BROWSER_CHECK_BUDGET_TELEMETRY"])
        with open(os.path.join(artifacts, "fat.bin"), "wb") as handle:
            handle.write(b"x" * 600000)
        until = time.monotonic() + 20
        while not os.path.exists(stop) and time.monotonic() < until:
            telemetry(event="hb")
            time.sleep(0.05)
        telemetry(event="dispatched", action_id="rogue")
        time.sleep(0.4)
    elif MODE == "overrun":
        for action in manifest_actions():
            run(action)
        run("surplus_action")
    elif MODE == "duplicate":
        first = manifest_actions()[0]
        run(first)
        run(first)
    elif MODE == "hang":
        run(manifest_actions()[0])
        spawn_stubborn_descendant()
        wait_for_file(os.path.join(OUT, "grandchild.pid"))
        time.sleep(300)
    elif MODE == "polite":
        # Alive, then silent: the telemetry a hard exposure limit depends on
        # stops, and the worker keeps going until it is asked to end.
        run(manifest_actions()[0])
        telemetry(event="agent_alive", value=True)
        while not os.path.exists(stop):
            time.sleep(0.02)
        run("end")
    elif MODE == "exposure":
        run(manifest_actions()[0])
        telemetry(event="agent_alive", value=True)
        while not os.path.exists(stop):
            telemetry(event="hb")
            time.sleep(0.05)
        run("end")
    elif MODE in ("reload", "bad_reload"):
        actions = manifest_actions()
        half = len(actions) // 2
        for action in actions[:half]:
            run(action)
        # A page reload is a new guard in the same run, not a new allowance.
        guard = irb.WorkerGuard.from_env()
        if MODE == "bad_reload":
            guard.next_seq = 1
        for action in actions[half:]:
            run(action)
        run("end")
    elif MODE == "die_mid_permit":
        first = manifest_actions()[0]
        irb.exchange(guard.socket_path, raw_permit_request(guard, first, 1))
        os._exit(9)
    elif MODE == "lose_reply":
        first = manifest_actions()[0]
        conn = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        conn.connect(guard.socket_path)
        conn.sendall(json.dumps(raw_permit_request(guard, first, 1)).encode() + b"\n")
        conn.close()
        time.sleep(0.2)
        # The reply was lost, so the worker cannot know whether it was granted.
        # Asking again is a duplicate, and nothing is dispatched either way.
        run(first)
    elif MODE == "lose_launcher":
        sleeper = subprocess.Popen(["sleep", "300"])
        note("sleeper.pid", str(sleeper.pid))
        lost = threading.Event()
        guard.watch_launcher(lost.set, interval=0.05)
        note("socket", os.environ["BROWSER_CHECK_BUDGET_SOCKET"])
        note("ready", "1")
        while not lost.is_set():
            time.sleep(0.05)
        try:
            run(manifest_actions()[0])
        except irb.PermitDenied as error:
            note("denied_after_loss", type(error).__name__)

        def cleanup():
            sleeper.kill()
            sleeper.wait()
            note("cleanup_done", "1")

        irb.bounded_cleanup(cleanup, 2.0)
        sys.exit(7)
    else:
        raise SystemExit("unknown FAKE_MODE " + MODE)
except irb.PermitDenied as error:
    note("stopped", str(error))
    sys.exit(3)
