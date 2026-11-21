"""Run only inside the disposable qualification VM as root."""
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import time


def run(args, *, good=True, timeout=120):
    result = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
    if good and result.returncode:
        raise AssertionError((args, result.returncode, result.stdout, result.stderr))
    return result


def leader(name="plane"):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        main = run(["systemctl", "show", f"qualification-container@{name}", "--property=MainPID", "--value"]).stdout.strip()
        try:
            for child in Path(f"/proc/{main}/task/{main}/children").read_text().split():
                if Path(f"/proc/{child}/comm").read_text().strip() == "systemd":
                    return child
        except FileNotFoundError:
            pass
        time.sleep(0.05)
    raise AssertionError(("No container PID1", name, main))


def inside_command(args, *, name="plane", user=False):
    command = ["nsenter", "--target", leader(name), "--mount", "--uts", "--ipc", "--net", "--pid", "--user", "--root", "--wdns=/", "--"]
    if user:
        command += ["/usr/bin/setpriv", "--reuid=1000", "--regid=1000", "--clear-groups", "--no-new-privs"]
    command += ["/usr/bin/env", "PATH=/usr/bin", "HOME=/var/cache/nickel-plane", "XDG_CACHE_HOME=/var/cache/nickel-plane", "NIX_REMOTE=local", "NIX_IGNORE_SYMLINK_STORE=1", *args]
    return command


def inside(args, *, name="plane", user=False, good=True, timeout=120):
    return run(inside_command(args, name=name, user=user), good=good, timeout=timeout)


INPUT = ["--config", "/etc/nickel-plane/instance/main.ncl", "--resources", "/opt/nickel/plane", "--targets", "/etc/nickel-plane/targets.json", "--tf-resources", "/opt/nickel/tf", "--k8s-resources", "/opt/nickel/k8s", "--schema", "local=/etc/nickel-plane/schemas/local"]


def plane(command, *args, good=True, name="plane"):
    if command != "status":
        print("RUN", name, command, "expect-success" if good else "expect-refusal", flush=True)
    argv = ["nickel-plane", command]
    if command not in ["status", "remove"]:
        argv += INPUT
    if command not in ["check", "render", "lock"]:
        argv += ["--store", "/var/lib/nickel-plane"]
    result = inside([*argv, *args], user=True, good=good, name=name)
    if not good:
        assert result.returncode != 0, ("Expected refusal", command, result.stdout)
        return result
    return json.loads(result.stdout)


def wait_for(predicate, *, timeout=60):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.1)
    raise AssertionError("Timed out waiting for fixture condition")


def service(action, *, good=True, name="plane"):
    return inside(["systemctl", action, "nickel-plane.service"], good=good, name=name)


def start_container(name="plane"):
    run(["systemctl", "start", f"qualification-container@{name}"], timeout=90)
    wait_for(lambda: inside(["systemctl", "is-system-running"], name=name, good=False).stdout.strip() in ["running", "degraded"])


def fixture_state():
    return json.loads(Path("/run/fixture-state.json").read_text())


def start_fixture():
    Path("/run/fixture-ready").unlink(missing_ok=True)
    log = open("/var/log/api-fixture.log", "a")
    proc = subprocess.Popen(["nsenter", "--target", leader(), "--net", "--", "python3", "/opt/qualification/api_fixture.py"], stdout=log, stderr=log)
    log.close()
    wait_for(lambda: Path("/run/fixture-ready").exists())
    return proc


def stop_fixture(proc):
    proc.terminate()
    proc.wait(timeout=10)


def confined_limits(source, baseline, *, command_builder=inside_command, container_leader=leader):
    source.write_text(baseline.replace('"hello from OpenTofu"', '(let rec loop = fun x => loop (x + 1) in loop 0)'))
    command = command_builder(["nickel-plane", "check", *INPUT], user=True)
    started = time.monotonic()
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        def find_nickel():
            for entry in Path("/proc").iterdir():
                if entry.name.isdigit():
                    try:
                        if (entry / "comm").read_text().strip() == "nickel":
                            return entry
                    except FileNotFoundError:
                        pass
            return None
        evaluation = wait_for(find_nickel, timeout=15)
        outer = Path("/proc") / container_leader()
        for namespace in ["net", "mnt", "pid", "user"]:
            assert (evaluation / "ns" / namespace).readlink() != (outer / "ns" / namespace).readlink(), namespace
        mounts = (evaluation / "mountinfo").read_text().splitlines()
        for destination in ["/config", "/libraries"]:
            mounted = [line.split() for line in mounts if line.split()[4] == destination]
            assert len(mounted) == 1 and "ro" in mounted[0][5].split(","), mounted
        limits = (evaluation / "limits").read_text()
        assert "1073741824" in limits and "20" in next(line for line in limits.splitlines() if "Max cpu time" in line), limits
        assert not (evaluation / "root/etc/nickel-plane").exists()
        stdout, stderr = process.communicate(timeout=40)
        assert process.returncode != 0 and time.monotonic() - started < 45, (stdout, stderr)
        print("PASS actual evaluator has separate user/mount/PID/network namespaces, read-only inputs and enforced limits", flush=True)
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        source.write_text(baseline)


def main():
    wait_for(lambda: run(["systemctl", "is-system-running"], good=False).stdout.strip() in ["running", "degraded"])
    assert run(["systemctl", "is-system-running"]).stdout.strip() == "running"
    state = Path("/var/lib/nickel-plane")
    state.mkdir(mode=0o700, exist_ok=True)
    state.chmod(0o700)
    os.chown(state, 1000, 1000)
    config = Path("/etc/nickel-plane")
    (config / "instance").mkdir(parents=True, exist_ok=True)
    (config / "controller.conf").write_text("PLANE_ARGS=\n")
    Path("/run/qualification-plane.conf").write_text("CONTAINER_ROOTFS=/var/lib/machines/plane-v1\nCONTAINER_UUID=0123456789abcdef0123456789abcdef\n")
    start_container()
    assert inside(["systemctl", "is-system-running"]).stdout.strip() == "running"
    print("HOST", run(["uname", "-r"]).stdout.strip(), run(["systemd-nspawn", "--version"]).stdout.splitlines()[0], flush=True)
    print("CONTAINER", inside(["id"]).stdout.strip(), flush=True)
    network = inside(["cat", "/proc/net/dev"]).stdout
    assert [line.split(":")[0].strip() for line in network.splitlines() if ":" in line] == ["lo"], network
    for tool in ["nickel", "nix", "tofu"]:
        print("VERSION", inside([tool, "--version"], user=True).stdout.strip(), flush=True)
    inside(["systemctl", "start", "nickel-nix-init.service"])
    inside(["sh", "-ec", "test ! -e /nix/var/nix/daemon-socket/socket; test ! -e /run/nix/daemon-socket/socket"])
    print("NIX", inside(["sh", "-ec", "nix-store --query --requisites $(readlink /usr/bin/nickel | sed 's,/bin/nickel$,,')"], user=True).stdout, flush=True)
    print("PASS initial container boot", flush=True)
    shutil.copytree("/opt/qualification/local-schema", config / "schemas/local")
    (config / "credentials").mkdir()
    token = config / "credentials/token"
    token.write_text("fixture-token")
    token.chmod(0o600)
    os.chown(token, 1000, 1000)
    (config / "credentials/secret.ncl").write_text('"synthetic-secret"')
    Path("/root/host-secret.ncl").write_text('"host-secret"')
    (state / "state-secret.ncl").write_text('"state-secret"')
    source = config / "instance/main.ncl"
    baseline = Path("/opt/qualification/main.ncl").read_text()
    source.write_text(baseline)
    catalog = {"version": 1, "targets": {
        "local": {"backend": "opentofu", "config": {"binary": "/no-such-provider", "stack": "fixture"}},
        "cluster": {"backend": "kubernetes", "config": {"server": "http://127.0.0.1:18443", "namespace": "demo", "allowHttpLoopback": True, "tokenFile": "/etc/nickel-plane/credentials/token"}},
    }}
    targets = config / "targets.json"
    targets.write_text(json.dumps(catalog))
    assert inside(["test", "-r", "/run/nickel-proc-private/proc/self/status"], user=True, good=False).returncode != 0
    checked = plane("check")
    assert checked["revision"] == plane("render")["revision"]
    receipt = plane("lock")
    assert "hello from" not in json.dumps(receipt)
    (config / "lock.json").write_text(json.dumps(receipt))
    plane("check", "--locked", "/etc/nickel-plane/lock.json")
    assert not Path("/run/fixture-state.json").exists()
    assert not (state / "store.sqlite").exists()
    print("PASS offline check/render/lock without provider executable or API", flush=True)

    for path in ["/etc/nickel-plane/credentials/secret.ncl", "/root/host-secret.ncl", "/var/lib/nickel-plane/state-secret.ncl"]:
        source.write_text(baseline.replace('"hello from OpenTofu"', f'(import "{path}")'))
        refused = plane("check", good=False)
        assert "synthetic-secret" not in refused.stdout + refused.stderr
        assert any(message in refused.stderr for message in ["No such file", "not found", "could not find import"]), refused.stderr
    source.write_text("x" * (1024 * 1024 + 1))
    refused = plane("check", good=False)
    assert "volumineux" in refused.stderr or "limit" in refused.stderr.lower(), refused.stderr
    source.write_text(baseline)
    confined_limits(source, baseline)
    deny_unit = "\n".join(["[Service]", "Type=oneshot", "User=nickel-plane", "Environment=PATH=/usr/bin NIX_REMOTE=local NIX_IGNORE_SYMLINK_STORE=1 HOME=/var/cache/nickel-plane", "RestrictNamespaces=yes", "ExecStart=/usr/bin/nickel-plane check " + " ".join(INPUT), ""])
    inside(["sh", "-ec", "cat > /run/systemd/system/reject-eval.service <<'EOF'\n" + deny_unit + "EOF\n"])
    inside(["systemctl", "daemon-reload"])
    denied = inside(["systemctl", "start", "reject-eval.service"], good=False)
    assert denied.returncode != 0
    diagnostic = inside(["journalctl", "--no-pager", "-u", "reject-eval.service"]).stdout
    assert "bwrap" in diagnostic, diagnostic
    inside(["systemctl", "reset-failed", "reject-eval.service"])
    print("PASS inaccessible credentials/host/state, source bound and namespace denial", flush=True)

    catalog["targets"]["local"]["config"]["binary"] = "/usr/bin/tofu"
    targets.write_text(json.dumps(catalog))
    fixture = start_fixture()
    try:
        # Default observe policy can inspect but cannot create infrastructure.
        plane("reconcile", "--once")
        assert fixture_state()["writes"] == 0
        managed = baseline.replace('mode="observe"', 'mode="manage"')
        source.write_text(managed)
        plan = plane("plan")
        operation = plan["units"]["infrastructure"]["change"]["operation"]
        plane("apply", "--plan", operation)
        plane("apply", "--plan", operation, good=False)
        (config / "controller.conf").write_text('PLANE_ARGS="--tf-resources /opt/nickel/tf --k8s-resources /opt/nickel/k8s --schema local=/etc/nickel-plane/schemas/local"\n')
        Path("/run/pause-fixture").touch()
        service("start")
        wait_for(lambda: Path("/run/fixture-paused").exists())
        database = sqlite3.connect(state / "store.sqlite")
        operations = database.execute("SELECT unit,phase FROM operations").fetchall()
        assert sum(phase == "running" for _, phase in operations) == 1, operations
        owner = database.execute("SELECT value FROM metadata WHERE key='owner'").fetchone()[0]
        database.close()
        inside(["systemctl", "kill", "--signal=KILL", "nickel-plane.service"])
        service("stop")
        Path("/run/pause-fixture").unlink()
        writes = fixture_state()["writes"]
        assert writes == 1
        service("start")
        wait_for(lambda: all(u["conditions"]["Applied"] for u in plane("status", "--instance", "mixed-demo")["units"].values()))
        assert fixture_state()["writes"] == writes
        assert next(iter(fixture_state()["objects"].values()))["data"]["message"] == "hello from OpenTofu"
        print("PASS real OpenTofu plan/apply and recovery after lost Kubernetes create response", flush=True)
        refused = plane("reconcile", "--once", good=False)
        assert "Another controller" in refused.stderr
        # A second container gets a distinct user namespace but the same mapped volume.
        settings = Path("/etc/systemd/nspawn/plane.nspawn").read_text().replace("65536:65536", "131072:65536")
        Path("/etc/systemd/nspawn/second.nspawn").write_text(settings)
        Path("/run/qualification-second.conf").write_text("CONTAINER_ROOTFS=/var/lib/machines/plane-v1\nCONTAINER_UUID=1123456789abcdef0123456789abcdef\n")
        start_container("second")
        inside(["systemctl", "start", "nickel-nix-init.service"], name="second")
        assert inside(["cat", "/etc/machine-id"], name="second").stdout != inside(["cat", "/etc/machine-id"]).stdout
        refused = plane("reconcile", "--once", good=False, name="second")
        assert "Another controller" in refused.stderr, refused.stderr
        run(["systemctl", "stop", "qualification-container@second"])
        print("PASS second container cannot acquire the persistent controller lease", flush=True)

        machine = inside(["cat", "/etc/machine-id"]).stdout
        before = plane("status", "--instance", "mixed-demo")
        with sqlite3.connect(state / "store.sqlite") as database:
            inventory = {unit: json.loads(value)["inventory"] for unit, value in database.execute("SELECT unit,state FROM units")}
        native = {str(p.relative_to(state)): p.read_bytes() for p in state.rglob("terraform.tfstate")}
        assert native
        service("stop")
        run(["systemctl", "stop", "qualification-container@plane"])
        stop_fixture(fixture)
        # Replace the filesystem tree; retain only the declared state and config volumes.
        shutil.copytree("/var/lib/machines/plane-v1", "/var/lib/machines/plane-v2", symlinks=True)
        Path("/var/lib/machines/plane-v2/usr/share/nickel-container/replacement-test").write_text("replacement rootfs\n")
        Path("/run/qualification-plane.conf").write_text("CONTAINER_ROOTFS=/var/lib/machines/plane-v2\nCONTAINER_UUID=0123456789abcdef0123456789abcdef\n")
        start_container()
        fixture = start_fixture()
        wait_for(lambda: inside(["systemctl", "is-active", "nickel-plane.service"], good=False).returncode == 0)
        assert inside(["cat", "/etc/machine-id"]).stdout == machine
        after = plane("status", "--instance", "mixed-demo")
        assert before["instance"] == after["instance"]
        with sqlite3.connect(state / "store.sqlite") as database:
            assert database.execute("SELECT value FROM metadata WHERE key='owner'").fetchone()[0] == owner
            assert {unit: json.loads(value)["inventory"] for unit, value in database.execute("SELECT unit,state FROM units")} == inventory
        assert {str(p.relative_to(state)): p.read_bytes() for p in state.rglob("terraform.tfstate")} == native
        assert state.stat().st_uid == 1000 and state.stat().st_mode & 0o777 == 0o700
        assert fixture_state()["writes"] == writes
        print("PASS image replacement preserves machine identity, owner, inventory and native state", flush=True)
        service("stop")
        assert all(u["conditions"]["Applied"] for u in plane("reconcile", "--once")["units"].values())
        refused = plane("remove", "--instance", "mixed-demo", "--targets", "/etc/nickel-plane/targets.json", "--delete", good=False)
        assert fixture_state()["writes"] == writes
        plane("remove", "--instance", "mixed-demo", "--targets", "/etc/nickel-plane/targets.json")
        assert fixture_state()["objects"]
        print("PASS destructive refusal and default resource retention", flush=True)
    finally:
        stop_fixture(fixture)


if __name__ == "__main__":
    try:
        main()
    except BaseException:
        print(run(["tail", "-n", "80", "/var/log/api-fixture.log"], good=False).stdout, flush=True)
        for name in ["plane", "second"]:
            print(run(["journalctl", "--no-pager", "-u", f"qualification-container@{name}", "-n", "80"], good=False).stdout, flush=True)
            try:
                print(inside(["journalctl", "--no-pager", "-n", "100"], name=name).stdout, flush=True)
            except BaseException:
                pass
        raise
