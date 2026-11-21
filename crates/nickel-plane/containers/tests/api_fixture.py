"""Persistent HTTP fixture, run in the container netns with the VM host filesystem."""
import http.server
import json
import os
from pathlib import Path
import time
import threading

os.environ.update(PLANE_BIN="unused", TF_BIN="unused", TOFU_BIN="unused")
from local_e2e import API as Base

STATE = Path("/run/fixture-state.json")
STATE_LOCK = threading.Lock()
if STATE.exists():
    previous = json.loads(STATE.read_text())
    Base.objects, Base.writes, Base.history = previous["objects"], previous["writes"], previous["history"]


def save_state():
    with STATE_LOCK:
        pending = STATE.with_suffix(".pending")
        pending.write_text(json.dumps({"objects": Base.objects, "writes": Base.writes, "history": Base.history}))
        pending.replace(STATE)


class API(Base):
    def reply(self, code, body):
        save_state()
        if self.command == "POST" and "dryRun=All" not in self.path and 200 <= code < 300 and Path("/run/pause-fixture").exists():
            Path("/run/fixture-paused").touch()
            deadline = time.monotonic() + 90
            while Path("/run/pause-fixture").exists() and time.monotonic() < deadline:
                time.sleep(0.05)
        try:
            super().reply(code, body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_GET(self):
        assert self.headers.get("Authorization") == "Bearer fixture-token"
        super().do_GET()

    def mutation(self):
        assert self.headers.get("Authorization") == "Bearer fixture-token"
        super().mutation()

    do_POST = mutation
    do_PATCH = mutation
    do_DELETE = mutation


server = http.server.ThreadingHTTPServer(("127.0.0.1", 18443), API)
save_state()
Path("/run/fixture-ready").touch()
server.serve_forever()
