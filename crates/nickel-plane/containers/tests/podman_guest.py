"""Rootless Quadlet qualification; run only in the disposable VM as root."""
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess

from guest import INPUT, confined_limits, run, wait_for

OPERATOR = Path("/home/operator")
STATE = OPERATOR / ".local/share/nickel-plane"
CONFIG = OPERATOR / ".config/nickel-plane"
UNITS = OPERATOR / ".config/containers/systemd"
ENV = ["PATH=/usr/bin", "HOME=/home/operator", "USER=operator", "LOGNAME=operator", "XDG_RUNTIME_DIR=/run/user/1000",
       "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"]


def user_command(args):
    return ["setpriv", "--reuid=1000", "--regid=1000", "--clear-groups", "env", *ENV, *args]


def user_run(args, **kwargs):
    return run(user_command(args), **kwargs)


def podman_command(args):
    # Start inside the delegated user subtree. A setuid command launched by the
    # root VM agent cannot move its process across the root-owned cgroup ancestor.
    return user_command(["systemd-run", "--user", "--pipe", "--wait", "--collect", "--quiet", "podman", *args])


def podman(*args, **kwargs):
    return run(podman_command(args), **kwargs)


def user_service(action, name="nickel-plane", **kwargs):
    return user_run(["systemctl", "--user", action, name + ".service"], **kwargs)


def leader(name="nickel-plane"):
    return podman("inspect", "--format", "{{.State.Pid}}", name).stdout.strip()


def inside_command(args, *, user=False, name="nickel-plane"):
    return podman_command(["exec", "--user", "1000:1000" if user else "0:0", name,
                         "env", "PATH=/usr/bin", "HOME=/var/cache/nickel-plane",
                         "XDG_CACHE_HOME=/var/cache/nickel-plane", "NIX_REMOTE=local",
                         "NIX_IGNORE_SYMLINK_STORE=1", *args])


def inside(args, *, user=False, name="nickel-plane", **kwargs):
    return run(inside_command(args, user=user, name=name), **kwargs)


def service(action, **kwargs):
    return inside(["systemctl", action, "nickel-plane.service"], **kwargs)


def plane(command, *args, good=True, name="nickel-plane"):
    if command != "status":
        print("RUN", name, command, "expect-success" if good else "expect-refusal", flush=True)
    argv = ["nickel-plane", command]
    if command not in ["status", "remove"]:
        argv += INPUT
    if command not in ["check", "render", "lock"]:
        argv += ["--store", "/var/lib/nickel-plane"]
    result = inside([*argv, *args], user=True, name=name, good=good)
    if not good:
        assert result.returncode != 0, result.stdout
        return result
    return json.loads(result.stdout)


def own_tree(path):
    for item in [path, *path.rglob("*")]:
        os.chown(item, 1000, 1000)


def install_unit(name="nickel-plane", *, image="quadlet", config="nickel-plane", state="nickel-plane"):
    value = Path("/opt/qualification", image, "nickel-plane.container").read_text()
    value = value.replace("ContainerName=nickel-plane", "ContainerName=" + name)
    value = value.replace("%h/.config/nickel-plane", "%h/.config/" + config)
    value = value.replace("%h/.local/share/nickel-plane", "%h/.local/share/" + state)
    (UNITS / (name + ".container")).write_text(value)
    dropins = OPERATOR / ".config/systemd/user" / (name + ".service.d")
    dropins.mkdir(parents=True, exist_ok=True)
    shutil.copy2(Path("/opt/qualification", image, "nickel-plane.service.d/10-container-id.conf"), dropins)
    own_tree(OPERATOR / ".config/systemd")
    own_tree(UNITS)
    user_run(["systemctl", "--user", "daemon-reload"])
    effective = user_run(["systemctl", "--user", "show", name + ".service", "--property=ExecStop,ExecStopPost"]).stdout
    assert effective.count("--cidfile=") == 2, effective
    assert "rm -v -f -i " not in effective, effective


def start_container(name="nickel-plane"):
    user_service("start", name, timeout=180)
    def ready():
        result = inside(["systemctl", "is-system-running"], name=name, good=False)
        assert result.returncode in [0, 1], (result.returncode, result.stdout, result.stderr)
        return result.stdout.strip() in ["running", "degraded"]
    wait_for(ready)


def fixture_state():
    return json.loads(Path("/run/fixture-state.json").read_text())


def start_fixture():
    Path("/run/fixture-ready").unlink(missing_ok=True)
    with open("/var/log/api-fixture.log", "a") as log:
        process = subprocess.Popen(["nsenter", "--target", leader(), "--net", "--", "python3",
                                    "/opt/qualification/api_fixture.py"], stdout=log, stderr=log)
    wait_for(lambda: Path("/run/fixture-ready").exists())
    return process


def stop_fixture(process):
    process.terminate()
    process.wait(timeout=10)


def main():
    assert os.geteuid() == 0 and 'ID=nickel-qualification' in Path('/usr/lib/os-release').read_text()
    assert not OPERATOR.exists(), "Fixture requires a fresh disposable VM"
    STATE.mkdir(parents=True, mode=0o700)
    (CONFIG / "instance").mkdir(parents=True)
    UNITS.mkdir(parents=True)
    Path("/run/user/1000").mkdir(parents=True, mode=0o700)
    os.chown("/run/user/1000", 1000, 1000)
    for name in ["newuidmap", "newgidmap"]:
        target = Path("/usr/bin", name)
        target.unlink(missing_ok=True)
        shutil.copy2(Path("/opt/qualification/shadow/bin", name), target)
        target.chmod(0o4755)
    (OPERATOR / ".config/containers/storage.conf").write_text('[storage]\ndriver="vfs"\n')
    (OPERATOR / ".config/containers/containers.conf").write_text('[engine]\nevents_logger="file"\ncgroup_manager="systemd"\n')
    (OPERATOR / ".config/containers/policy.json").write_text(json.dumps({"default": [{"type": "insecureAcceptAnything"}]}))
    (CONFIG / "container.env").write_text("container_uuid=0123456789abcdef0123456789abcdef\n")
    (CONFIG / "controller.conf").write_text("PLANE_ARGS=\n")
    own_tree(OPERATOR)
    run(["systemctl", "start", "qualification-user.service"])
    wait_for(lambda: user_run(["systemctl", "--user", "is-system-running"], good=False).stdout.strip() == "running")
    # This minimal Nix host has no login environment to supply the helper path.
    user_run(["systemctl", "--user", "set-environment", "PATH=/usr/bin"])
    user_run(["systemctl", "--user", "start", "dbus.service"])
    wait_for(lambda: "org.freedesktop.systemd1" in user_run(["busctl", "--user", "list"]).stdout)
    info = json.loads(podman("info", "--format", "json").stdout)
    assert info["host"]["security"]["rootless"] and info["host"]["security"]["seccompEnabled"]
    assert info["host"]["cgroupVersion"] == "v2"
    for kind in ["uidmap", "gidmap"]:
        assert any(mapping["host_id"] == 65536 and mapping["size"] == 65536
                   for mapping in info["host"]["idMappings"][kind]), info["host"]["idMappings"]
    print("HOST", run(["uname", "-r"]).stdout.strip(), podman("--version").stdout.strip(), "rootless/seccomp/cgroup-v2", flush=True)
    podman("load", "-i", "/opt/qualification/image.tar.gz", timeout=180)
    print("PASS mixed image loaded with full subordinate UID/GID mappings", flush=True)
    install_unit()
    # Establish a healthy positive control before testing refused launches.
    start_container()
    inside(["systemctl", "start", "nickel-nix-init.service"])
    user_service("stop")
    assert podman("container", "exists", "nickel-plane", good=False).returncode == 1
    print("PASS initial rootless startup and cleanup by owned container ID", flush=True)
    image_id = Path("/opt/qualification/quadlet/image-id").read_text().strip()
    existing = podman("create", "--name", "nickel-plane", "--network", "none", "--entrypoint", "/usr/bin/true", image_id).stdout.strip()
    override = OPERATOR / ".config/systemd/user/nickel-plane.service.d/10-container-id.conf"
    saved_override = override.read_text()
    override.unlink()
    user_run(["systemctl", "--user", "daemon-reload"])
    user_service("start")
    assert user_service("is-active", good=False).returncode != 0
    assert podman("inspect", "--format", "{{.Id}}", "nickel-plane").stdout.strip() == existing
    override.write_text(saved_override)
    own_tree(override.parent)
    user_run(["systemctl", "--user", "daemon-reload"])
    for _ in range(2):
        assert user_service("start", good=False).returncode != 0
        user_service("stop")
        user_service("reset-failed", good=False)
        assert podman("inspect", "--format", "{{.Id}}", "nickel-plane").stdout.strip() == existing
    podman("rm", existing)
    print("PASS missing cleanup override fails closed and name collision preserves existing container", flush=True)

    for owner, mode in [(1000, 0o755), (1001, 0o700)]:
        os.chown(STATE, owner, owner)
        STATE.chmod(mode)
        started = user_service("start", good=False)
        if started.returncode == 0:
            wait_for(lambda: inside(["systemctl", "is-system-running"], good=False).stdout.strip() in ["running", "degraded"])
            assert inside(["systemctl", "start", "nickel-nix-init.service"], good=False).returncode != 0
        user_service("stop", good=False)
        user_service("reset-failed", good=False)
        assert STATE.stat().st_uid == owner and STATE.stat().st_mode & 0o777 == mode
        assert not (STATE / "store.sqlite").exists()
    os.chown(STATE, 1000, 1000)
    STATE.chmod(0o700)
    print("PASS wrong state owner/mode refused without adoption or permission changes", flush=True)
    start_container()
    assert inside(["systemctl", "is-system-running"]).stdout.strip() == "running"
    inside(["systemctl", "start", "nickel-nix-init.service"])
    assert inside(["test", "-r", "/run/nickel-proc-private/proc/self/status"], user=True, good=False).returncode != 0
    inspect = json.loads(podman("inspect", "nickel-plane").stdout)[0]
    assert not inspect["HostConfig"]["Privileged"]
    assert inspect["HostConfig"]["ReadonlyRootfs"]
    assert "unconfined" not in json.dumps(inspect["HostConfig"].get("SecurityOpt", []))
    network = inside(["cat", "/proc/net/dev"]).stdout
    assert [line.split(":")[0].strip() for line in network.splitlines() if ":" in line] == ["lo"]
    print("PASS real rootless Quadlet startup with private Nix and retained runtime protections", flush=True)

    shutil.copytree("/opt/qualification/local-schema", CONFIG / "schemas/local")
    (CONFIG / "credentials").mkdir()
    token = CONFIG / "credentials/token"
    token.write_text("fixture-token")
    token.chmod(0o600)
    os.chown(token, 1000, 1000)
    (CONFIG / "credentials/secret.ncl").write_text('"synthetic-secret"')
    Path("/root/host-secret.ncl").write_text('"host-secret"')
    (STATE / "state-secret.ncl").write_text('"state-secret"')
    source = CONFIG / "instance/main.ncl"
    baseline = Path("/opt/qualification/main.ncl").read_text()
    source.write_text(baseline)
    catalog = {"version": 1, "targets": {
        "local": {"backend": "opentofu", "config": {"binary": "/no-such-provider", "stack": "fixture"}},
        "cluster": {"backend": "kubernetes", "config": {"server": "http://127.0.0.1:18443", "namespace": "demo", "allowHttpLoopback": True, "tokenFile": "/etc/nickel-plane/credentials/token"}},
    }}
    targets = CONFIG / "targets.json"
    targets.write_text(json.dumps(catalog))
    assert plane("check")["revision"] == plane("render")["revision"]
    receipt = plane("lock")
    (CONFIG / "lock.json").write_text(json.dumps(receipt))
    plane("check", "--locked", "/etc/nickel-plane/lock.json")
    assert not (STATE / "store.sqlite").exists() and not Path("/run/fixture-state.json").exists()
    for path in ["/etc/nickel-plane/credentials/secret.ncl", "/root/host-secret.ncl", "/var/lib/nickel-plane/state-secret.ncl"]:
        source.write_text(baseline.replace('"hello from OpenTofu"', f'(import "{path}")'))
        result = plane("check", good=False)
        assert "synthetic-secret" not in result.stdout + result.stderr
        assert any(message in result.stderr for message in ["No such file", "not found", "could not find import"]), result.stderr
    source.write_text("x" * (1024 * 1024 + 1))
    result = plane("check", good=False)
    assert "volumineux" in result.stderr or "limit" in result.stderr.lower()
    source.write_text(baseline)
    confined_limits(source, baseline, command_builder=inside_command, container_leader=leader)
    deny_unit = "\n".join(["[Service]", "Type=oneshot", "User=nickel-plane",
        "Environment=PATH=/usr/bin NIX_REMOTE=local NIX_IGNORE_SYMLINK_STORE=1 HOME=/var/cache/nickel-plane",
        "RestrictNamespaces=yes", "ExecStart=/usr/bin/nickel-plane check " + " ".join(INPUT), ""])
    inside(["sh", "-ec", "cat > /run/systemd/system/reject-eval.service <<'EOF'\n" + deny_unit + "EOF\n"])
    inside(["systemctl", "daemon-reload"])
    assert inside(["systemctl", "start", "reject-eval.service"], good=False).returncode != 0
    assert "bwrap" in inside(["journalctl", "--no-pager", "-u", "reject-eval.service"]).stdout
    inside(["systemctl", "reset-failed", "reject-eval.service"])
    print("PASS offline compiler, inaccessible credentials/host/state and inner evaluator limits", flush=True)

    catalog["targets"]["local"]["config"]["binary"] = "/usr/bin/tofu"
    targets.write_text(json.dumps(catalog))
    fixture = start_fixture()
    try:
        plane("reconcile", "--once")
        assert fixture_state()["writes"] == 0
        managed = baseline.replace('mode="observe"', 'mode="manage"')
        source.write_text(managed)
        operation = plane("plan")["units"]["infrastructure"]["change"]["operation"]
        plane("apply", "--plan", operation)
        plane("apply", "--plan", operation, good=False)
        (CONFIG / "controller.conf").write_text('PLANE_ARGS="--tf-resources /opt/nickel/tf --k8s-resources /opt/nickel/k8s --schema local=/etc/nickel-plane/schemas/local"\n')
        print("RUN controller interruption after committed Kubernetes creation", flush=True)
        Path("/run/pause-fixture").touch()
        service("start")
        wait_for(lambda: Path("/run/fixture-paused").exists())
        pid = inside(["systemctl", "show", "nickel-plane.service", "--property=MainPID", "--value"]).stdout.strip()
        status = inside(["cat", f"/proc/{pid}/status"]).stdout
        assert "CapEff:\t0000000000000000" in status and "CapBnd:\t0000000000000000" in status
        assert "Uid:\t1000\t1000\t1000\t1000" in status
        inside(["systemctl", "kill", "--signal=KILL", "nickel-plane.service"])
        print("RUN stopping killed controller before recovery", flush=True)
        service("stop")
        Path("/run/pause-fixture").unlink()
        print("RUN restarting controller and checking recovery", flush=True)
        service("start")
        wait_for(lambda: all(unit["conditions"]["Applied"] for unit in plane("status", "--instance", "mixed-demo")["units"].values()))
        assert fixture_state()["writes"] == 1
        assert "Another controller" in plane("reconcile", "--once", good=False).stderr
        print("PASS real OpenTofu and recovery without repeated mutation; controller UID 1000, zero capabilities", flush=True)

        second_config = OPERATOR / ".config/plane-second"
        shutil.copytree(CONFIG, second_config)
        (second_config / "container.env").write_text("container_uuid=1123456789abcdef0123456789abcdef\n")
        own_tree(second_config)
        install_unit("plane-second", config="plane-second")
        start_container("plane-second")
        inside(["systemctl", "start", "nickel-nix-init.service"], name="plane-second")
        assert "Another controller" in plane("reconcile", "--once", name="plane-second", good=False).stderr
        user_service("stop", "plane-second")

        machine = inside(["cat", "/etc/machine-id"]).stdout
        with sqlite3.connect(STATE / "store.sqlite") as database:
            owner = database.execute("SELECT value FROM metadata WHERE key='owner'").fetchone()[0]
            inventory = {unit: json.loads(value)["inventory"] for unit, value in database.execute("SELECT unit,state FROM units")}
        native = {str(p.relative_to(STATE)): p.read_bytes() for p in STATE.rglob("terraform.tfstate")}
        assert native
        old_container = podman("inspect", "--format", "{{.Id}}", "nickel-plane").stdout
        service("stop")
        user_service("stop")
        stop_fixture(fixture)
        start_container()
        fixture = start_fixture()
        wait_for(lambda: inside(["systemctl", "is-active", "nickel-plane.service"], good=False).returncode == 0)
        # Active means the process exists, not that its first reconciliation is
        # complete. Its status is printed after that cycle; the fresh /var has
        # no journal entries from the previous container.
        wait_for(lambda: '"instance": "mixed-demo"' in inside([
            "journalctl", "--no-pager", "-u", "nickel-plane.service", "-o", "cat",
            "-n", "200"]).stdout)
        service("stop")
        assert all(unit["conditions"]["Applied"] for unit in plane("status", "--instance", "mixed-demo")["units"].values())
        assert podman("inspect", "--format", "{{.Id}}", "nickel-plane").stdout != old_container
        assert inside(["cat", "/etc/machine-id"]).stdout == machine
        with sqlite3.connect(STATE / "store.sqlite") as database:
            assert database.execute("SELECT value FROM metadata WHERE key='owner'").fetchone()[0] == owner
            assert {unit: json.loads(value)["inventory"] for unit, value in database.execute("SELECT unit,state FROM units")} == inventory
        assert {str(p.relative_to(STATE)): p.read_bytes() for p in STATE.rglob("terraform.tfstate")} == native
        assert STATE.stat().st_uid == 1000 and STATE.stat().st_gid == 1000 and STATE.stat().st_mode & 0o777 == 0o700
        assert all(unit["conditions"]["Applied"] for unit in plane("reconcile", "--once")["units"].values())
        plane("remove", "--instance", "mixed-demo", "--targets", "/etc/nickel-plane/targets.json", "--delete", good=False)
        plane("remove", "--instance", "mixed-demo", "--targets", "/etc/nickel-plane/targets.json")
        assert fixture_state()["writes"] == 1 and fixture_state()["objects"]
        print("PASS writer exclusion, container recreation preserving identity/state/ownership and default retention", flush=True)
    finally:
        stop_fixture(fixture)
    user_service("stop")

    tf_config = OPERATOR / ".config/plane-tf"
    tf_state = OPERATOR / ".local/share/plane-tf"
    (tf_config / "instance").mkdir(parents=True)
    tf_state.mkdir(mode=0o700)
    (tf_config / "container.env").write_text("container_uuid=2123456789abcdef0123456789abcdef\n")
    (tf_config / "controller.conf").write_text("PLANE_ARGS=\n")
    own_tree(tf_config)
    own_tree(tf_state)
    podman("load", "-i", "/opt/qualification/image-tf.tar.gz", timeout=180)
    install_unit("plane-tf", image="quadlet-tf", config="plane-tf", state="plane-tf")
    start_container("plane-tf")
    inside(["systemctl", "start", "nickel-nix-init.service"], name="plane-tf")
    assert inside(["test", "-e", "/opt/nickel/k8s"], name="plane-tf", good=False).returncode != 0
    shutil.copytree("/opt/qualification/local-schema", tf_config / "schemas/local")
    shutil.copy2("/opt/qualification/local-main.ncl", tf_config / "instance/main.ncl")
    (tf_config / "targets.json").write_text(json.dumps({"version": 1, "targets": {"local": catalog["targets"]["local"]}}))
    tf_args = ["--config", "/etc/nickel-plane/instance/main.ncl", "--resources", "/opt/nickel/plane",
               "--tf-resources", "/opt/nickel/tf", "--schema", "local=/etc/nickel-plane/schemas/local",
               "--targets", "/etc/nickel-plane/targets.json"]
    inside(["nickel-plane", "check", *tf_args], name="plane-tf", user=True)
    user_service("stop", "plane-tf")
    print("PASS rootless OpenTofu-only image evaluates without Kubernetes resources", flush=True)


if __name__ == "__main__":
    try:
        main()
    except BaseException:
        print(run(["journalctl", "--no-pager", "-n", "100"], good=False).stdout, flush=True)
        print(user_run(["journalctl", "--user", "--no-pager", "-n", "100"], good=False).stdout, flush=True)
        print(podman("ps", "-a", good=False).stdout, flush=True)
        logs = podman("logs", "--tail", "100", "nickel-plane", good=False)
        print(logs.stdout + logs.stderr, flush=True)
        try:
            logs = inside(["journalctl", "--no-pager", "-n", "100"], good=False)
            print(logs.stdout + logs.stderr, flush=True)
        except BaseException:
            pass
        raise
