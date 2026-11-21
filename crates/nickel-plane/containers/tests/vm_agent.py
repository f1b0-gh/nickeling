"""Bounded guest-agent transport for disposable container qualification VMs."""
import base64
import json
import shlex
import socket
import time


class Guest:
    def __init__(self, connection, deadline, events):
        self.connection = connection
        self.deadline = deadline
        self.events = events
        self.buffer = b""
        self.sequence = 0

    def record(self, kind, **fields):
        self.events.write(json.dumps({"kind": kind, **fields}) + "\n")
        self.events.flush()

    def remaining(self, maximum=10):
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("VM test wall-clock deadline exceeded")
        return min(maximum, remaining)

    def rpc(self, command, arguments=None):
        self.sequence += 1
        request = {"execute": command, "id": self.sequence}
        if arguments is not None:
            request["arguments"] = arguments
        self.connection.settimeout(self.remaining())
        self.connection.sendall(json.dumps(request).encode() + b"\n")
        while True:
            while b"\n" not in self.buffer:
                self.connection.settimeout(self.remaining())
                chunk = self.connection.recv(65536)
                if not chunk:
                    raise ConnectionError("Guest agent disconnected")
                self.buffer += chunk
            line, self.buffer = self.buffer.split(b"\n", 1)
            if not line.strip():
                continue
            response = json.loads(line.lstrip(b"\xff"))
            if response.get("id") != self.sequence:
                continue
            if "error" in response:
                raise RuntimeError(f"{command}: {response['error']}")
            return response["return"]

    def start(self, script):
        return self.rpc("guest-exec", {
            "path": "/usr/bin/sh", "arg": ["-c", "export PATH=/usr/bin; " + script],
            "capture-output": True,
        })["pid"]

    def run(self, script, *, check=True, timeout=30):
        pid = self.start(script)
        deadline = min(self.deadline, time.monotonic() + timeout)
        while True:
            result = self.rpc("guest-exec-status", {"pid": pid})
            if result.get("exited"):
                result = {
                    "exitcode": result.get("exitcode", -1),
                    "stdout": base64.b64decode(result.get("out-data", "")).decode(errors="replace"),
                    "stderr": base64.b64decode(result.get("err-data", "")).decode(errors="replace"),
                    "truncated": result.get("out-truncated", False) or result.get("err-truncated", False),
                }
                self.record("guest-command", script=script, **result)
                if result["truncated"]:
                    raise RuntimeError("Guest command output was truncated")
                if check and result["exitcode"] != 0:
                    raise RuntimeError(f"Guest command failed: {script}\n{result}")
                return result
            if time.monotonic() >= deadline:
                raise TimeoutError(f"Guest command timed out: {script}")
            time.sleep(0.1)

    def json(self, script):
        return json.loads(self.run(script)["stdout"])

    def put_json(self, path, value):
        encoded = base64.b64encode(json.dumps(value).encode()).decode()
        self.run(f"printf %s {shlex.quote(encoded)} | base64 -d > {shlex.quote(path)}")


def connect_agent(path, process, deadline, events):
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"QEMU exited before guest agent readiness: {process.returncode}")
        connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            connection.settimeout(1)
            connection.connect(str(path))
            guest = Guest(connection, min(deadline, time.monotonic() + 2), events)
            guest.rpc("guest-ping")
            guest.deadline = deadline
            return guest
        except (OSError, TimeoutError):
            connection.close()
            time.sleep(0.25)
    raise TimeoutError("Guest agent did not become ready before the VM deadline")
