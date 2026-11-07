#!/usr/bin/env python3
import json
import os
import signal
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

if "--list" in sys.argv:
    print("module::serve: test")
    sys.exit(0)
directory = os.environ["CODETRIAL_FIXTURE_DIR"]
address = os.environ["CODETRIAL_FIXTURE_ADDR"]
host, port = address.rsplit(":", 1)


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"ok")

    def do_POST(self):
        self.send_response(200)
        self.send_header("Set-Cookie", "codetrial_session=stand-in; Path=/")
        self.end_headers()

    def log_message(self, *args):
        pass


server = HTTPServer((host, int(port)), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
with open(os.path.join(directory, "ready"), "w") as ready:
    ready.write(address)
# The two lines the script looks for to know a room got an interviewer.
print("codetrial dispatch room=stand-in", flush=True)
print("starting interview: room=stand-in", flush=True)
terms = []
signal.signal(signal.SIGTERM, lambda *args: terms.append(1))
with open(os.environ["STAND_IN_PIDFILE"], "w") as pidfile:
    pidfile.write(str(os.getpid()))
# A deadline of its own: a run that never reaches the stop file must not leave
# this behind.
deadline = time.time() + 120
while (
    not os.path.exists(os.path.join(directory, "stop"))
    and not terms
    and time.time() < deadline
):
    time.sleep(0.05)
print("stand-in fixture stopping", flush=True)
if os.environ.get("STAND_IN_WRITES_REPORT"):
    report = {
        "outcome": "ok",
        "bound": address,
        "arming_failure": None,
        # What the launcher passed, so the test can hold it to the manifest.
        "launched_with": {
            name: os.environ.get(f"CODETRIAL_FIXTURE_{name}")
            for name in ("DEADLINE_S", "DRAIN_S", "SETTLE_S")
        },
        "capture": {
            "failure": None,
            "receipts": [],
            "batches": [],
            "replies": [],
            "boundaries": [],
        },
    }
    with open(os.path.join(directory, "out.json"), "w") as out:
        json.dump(report, out)
