#!/usr/bin/env python3
"""Run the real Kubernetes API tests in a disposable, explicitly owned kind cluster."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
# Official kind v0.33.0 release, Kubernetes 1.34 matching the contracts' minor.
# https://github.com/kubernetes-sigs/kind/releases/tag/v0.33.0
IMAGE = "kindest/node:v1.34.11@sha256:44e222ee2132dab25ff87301682f89eb82c7880ea3a1bf543bfe9708fd08d67d"


def run(args, *, timeout=60, input=None, env=None, check=True):
    process = subprocess.Popen(args, cwd=ROOT, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True, env=env, start_new_session=True)
    try:
        stdout, stderr = process.communicate(input, timeout=timeout)
    except BaseException:
        os.killpg(process.pid, signal.SIGKILL)
        process.communicate(timeout=10)
        raise
    result = subprocess.CompletedProcess(args, process.returncode, stdout, stderr)
    if check and result.returncode:
        # Never interpolate commands/outputs here: token requests and kubeconfig contain credentials.
        raise RuntimeError(f"{Path(args[0]).name} {args[1]} failed (exit {result.returncode})")
    return result


def source_fingerprints():
    paths = run(["git", "ls-files", "--cached", "--others", "--exclude-standard"]).stdout.splitlines()
    result = {}
    for path in paths:
        source = ROOT / path
        if source.is_symlink():
            raise RuntimeError("Source provenance refuses symlinks")
        if source.is_file():
            with source.open("rb") as stream:
                digest = hashlib.sha256()
                while chunk := stream.read(1024 * 1024):
                    digest.update(chunk)
                result[path] = digest.hexdigest()
    return result


def report(output, summary):
    # Small standalone PDF using only the standard library; never include credentials or API objects.
    lines = ["Nickeling real Kubernetes API test", f"Cluster: {summary['cluster']}",
             f"Server: {summary.get('server_version', 'not ready')}",
             f"Tests exit code: {summary.get('tests_exit_code', 'not run')}",
             f"Cluster removed: {summary.get('cluster_removed', False)}",
             f"Image removed: {summary.get('image_removed', False)}",
             "Scope: local kind API semantics; not live k3s or production qualification.",
             "Reproduction: python3 scripts/test-kind.py", "Evidence: reproduce.json, result.json, tests.log",
             "Source: crates/nickel-plane/tests/kubernetes_real.rs"]
    details = [f"{test['outcome']}: {test['name']}" for test in summary.get("test_outcomes", [])]
    details.extend(summary.get("findings", []))
    # A4, one page; the JSON retains full names and any additional findings.
    lines.extend(line if len(line) <= 80 else line[:77] + "..." for line in details[:12])
    if len(details) > 12:
        lines.append("Additional outcomes/findings are retained in result.json.")
    content = "BT /F1 10 Tf 50 790 Td " + " ".join(
        ("0 -20 Td " if i else "") + "(" + line.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)") + ") Tj"
        for i, line in enumerate(lines)) + " ET"
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
               b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>",
               b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>",
               f"<< /Length {len(content.encode())} >>\nstream\n{content}\nendstream".encode()]
    data = bytearray(b"%PDF-1.4\n")
    offsets = [0]
    for i, body in enumerate(objects, 1):
        offsets.append(len(data))
        data.extend(f"{i} 0 obj\n".encode() + body + b"\nendobj\n")
    position = len(data)
    data.extend(f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode())
    for offset in offsets[1:]:
        data.extend(f"{offset:010d} 00000 n \n".encode())
    data.extend(f"trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{position}\n%%EOF\n".encode())
    (output / "report.pdf").write_bytes(data)


def pinned(package):
    args = ["nix", "--extra-experimental-features", "nix-command flakes"]
    ref = f"path:{ROOT}/crates/nickel-plane#{package}"
    run(args + ["build", "--no-link", "--no-update-lock-file", "--no-write-lock-file",
                "--max-jobs", "1", "--cores", "2", ref], timeout=600)
    return Path(run(args + ["eval", "--offline", "--raw", "--no-update-lock-file",
                           "--no-write-lock-file", f"{ref}.outPath"]).stdout.strip()) / "bin" / package


def private(path, value):
    with path.open("x", encoding="utf-8") as stream:
        os.chmod(path, 0o600)
        stream.write(value)


def local_docker_endpoint(environment):
    # Docker gives an explicitly selected context priority over DOCKER_HOST.
    context = environment.get("DOCKER_CONTEXT")
    if context:
        info = json.loads(run(["docker", "context", "inspect", context]).stdout)[0]
        endpoint = info["Endpoints"]["docker"]["Host"]
    elif environment.get("DOCKER_HOST"):
        endpoint = environment["DOCKER_HOST"]
    else:
        info = json.loads(run(["docker", "context", "inspect"]).stdout)[0]
        endpoint = info["Endpoints"]["docker"]["Host"]
    if not endpoint.startswith("unix://"):
        raise RuntimeError("Refusing a remote Docker endpoint")
    return endpoint


def cargo_target_directory(value):
    target = Path(value)
    if not target.is_absolute():
        target = ROOT / target
    if ".." in target.parts or any(path.is_symlink() for path in [target, *target.parents]):
        raise RuntimeError("Cargo target path must be normalized and free of symlinks")
    if any(target.is_relative_to(crate) for crate in (ROOT / "crates").iterdir()):
        raise RuntimeError("Cargo target must stay outside Nix crate source trees")
    return str(target)


def test_command(command, env, output):
    with (output / "tests.log").open("w") as log:
        process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=log,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        try:
            returncode = process.wait(timeout=1200)
        except BaseException:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=10)
            raise
    with (output / "tests.log").open("rb") as log:
        log.seek(0, os.SEEK_END)
        log.seek(max(0, log.tell() - 16000))
        tail = log.read().decode("utf-8", errors="replace")
        print(tail, end="", flush=True)
    if returncode == 0 and not re.search(r"test result: ok\. [1-9][0-9]* passed;", tail):
        raise RuntimeError("The selected real API test filter ran no tests")
    return returncode


def cleanup(kind, name, node, image_preexisting, summary):
    # Scope deletion to the unique cluster name and verify the actual container label.
    inspection = run(["docker", "inspect", node], check=False)
    if inspection.returncode == 0:
        objects = json.loads(inspection.stdout)
        if len(objects) != 1 or objects[0]["Config"]["Labels"].get("io.x-k8s.kind.cluster") != name:
            raise RuntimeError("Refusing cleanup of a container without this fixture's cluster label")
    run([str(kind), "delete", "cluster", "--name", name], timeout=120)
    remaining = run(["docker", "ps", "-aq", "--filter", f"label=io.x-k8s.kind.cluster={name}"]).stdout.strip()
    if remaining:
        raise RuntimeError("Owned kind containers remain after cleanup")
    summary["cluster_removed"] = True
    if image_preexisting:
        summary["image_retained"] = "preexisting cache"
    elif run(["docker", "ps", "-aq", "--filter", f"ancestor={IMAGE}"]).stdout.strip():
        summary["image_retained"] = "in use by another container"
    else:
        removal = run(["docker", "image", "rm", IMAGE], timeout=120, check=False)
        summary["image_removed"] = removal.returncode == 0
        if removal.returncode:
            summary["image_retained"] = "Docker refused non-forced image removal"


def finish(kind, name, node, image_preexisting, summary, secrets, output, attempted):
    summary.setdefault("cluster", name)
    errors = []
    try:
        if attempted:
            cleanup(kind, name, node, image_preexisting, summary)
    except BaseException as error:
        summary["cleanup_error"] = str(error)
        summary["remaining_owned_cluster"] = name
        errors.append("cluster cleanup failed")
    for secret in secrets:
        try:
            secret.unlink(missing_ok=True)
        except OSError:
            errors.append(f"could not remove private {secret.name} file")
    summary["finished_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    try:
        report(output, summary)
    except OSError:
        errors.append("could not write PDF report")
    summary["finalization_errors"] = errors
    try:
        (output / "result.json").write_text(json.dumps(summary, indent=2))
    except OSError:
        errors.append("could not write result summary")
    print(f"Evidence: {output}", flush=True)
    if errors:
        raise RuntimeError("; ".join(errors))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="New private evidence directory, outside crate resources")
    parser.add_argument("--filter", default="", help="Rust test name filter, for targeted diagnosis")
    args = parser.parse_args()
    os.umask(0o077)
    os.environ["KIND_EXPERIMENTAL_PROVIDER"] = "docker"
    target_directory = cargo_target_directory(os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target" / "kind")))
    name = "nickeling-review-api-" + uuid.uuid4().hex[:12]
    output = args.output or ROOT / ".local" / "kind" / name
    output = output.absolute()
    if ".." in output.parts or any(p.is_symlink() for p in [output, *output.parents]):
        raise RuntimeError("Evidence path must be normalized and free of symlinks")
    if any(output.is_relative_to(p) for p in (ROOT / "crates").iterdir()):
        raise RuntimeError("Evidence must stay outside Nix crate source trees")
    output.mkdir(parents=True, exist_ok=False)
    kind = pinned("kind")
    kubectl = pinned("kubectl")
    local_docker_endpoint(os.environ)
    run(["docker", "version"])
    clusters = run([str(kind), "get", "clusters"]).stdout.splitlines()
    if name in clusters:
        raise RuntimeError("Refusing an existing cluster name")
    image_preexisting = run(["docker", "image", "inspect", IMAGE], check=False).returncode == 0
    initial_image_ids = set(run(["docker", "image", "ls", "--no-trunc", "--quiet"]).stdout.splitlines())
    summary = {"cluster":name, "image":IMAGE, "image_preexisting":image_preexisting,
               "kind":run([str(kind), "version"]).stdout.strip(),
               "started_at":time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    fingerprints = source_fingerprints()
    (output / "reproduce.json").write_text(json.dumps({"command":["python3", "scripts/test-kind.py", "--filter", args.filter],
        "image":IMAGE,"source_revision":run(["git", "rev-parse", "HEAD"]).stdout.strip(),
        "kind_path":str(kind),"kubectl_path":str(kubectl),
        "kind_version":summary["kind"],"rustc":run(["rustc", "--version"]).stdout.strip(),
        "cargo":run(["cargo", "--version"]).stdout.strip(),
        "docker":run(["docker", "version", "--format", "{{.Client.Version}} / {{.Server.Version}}"]).stdout.strip(),
        "cargo_target_dir":target_directory,
        "source_sha256":fingerprints}, indent=2))
    node = name + "-control-plane"
    attempted = False
    secrets = []
    old_handlers = {}
    def interrupted(signum, _frame):
        raise KeyboardInterrupt(f"Signal {signum}")
    for sig in (signal.SIGTERM, signal.SIGINT):
        old_handlers[sig] = signal.signal(sig, interrupted)
    try:
        # This home is private; kind never merges into the user's kubeconfig.
        kubeconfig = output / "kubeconfig"
        secrets.append(kubeconfig)
        config = output / "kind.json"
        config.write_text(json.dumps({"kind":"Cluster","apiVersion":"kind.x-k8s.io/v1alpha4",
            "networking":{"apiServerAddress":"127.0.0.1","apiServerPort":0},
            "nodes":[{"role":"control-plane","labels":{"nickeling.io/fixture":name}}]}))
        attempted = True
        print(f"Creating owned kind cluster {name} with the pinned node image", flush=True)
        result = run([str(kind), "create", "cluster", "--name", name, "--image", IMAGE,
                      "--config", str(config), "--kubeconfig", str(kubeconfig), "--wait", "180s"], timeout=600, check=False)
        (output / "cluster.log").write_text(result.stdout + result.stderr)
        if result.returncode:
            raise RuntimeError("kind cluster creation failed; see private cluster.log")
        image = json.loads(run(["docker", "image", "inspect", IMAGE]).stdout)[0]
        image_preexisting |= image["Id"] in initial_image_ids
        summary.update(image_preexisting=image_preexisting, image_id=image["Id"], image_bytes=image["Size"])
        os.chmod(kubeconfig, 0o600)
        # API operations use kubectl shipped in the same pinned node image (no version skew).
        admin = ["docker", "exec", "-i", node, "kubectl", "--kubeconfig=/etc/kubernetes/admin.conf", "--request-timeout=30s"]
        resources = {"apiVersion":"v1","kind":"List","items":[
            {"apiVersion":"v1","kind":"Namespace","metadata":{"name":"nickeling-api"}},
            {"apiVersion":"v1","kind":"ServiceAccount","metadata":{"name":"fixture","namespace":"nickeling-api"}},
            {"apiVersion":"rbac.authorization.k8s.io/v1","kind":"Role","metadata":{"name":"fixture","namespace":"nickeling-api"},
             "rules":[{"apiGroups":["","apps","networking.k8s.io"],"resources":["configmaps","services","deployments","networkpolicies"],"verbs":["get","list","watch","create","patch","delete"]}]},
            {"apiVersion":"rbac.authorization.k8s.io/v1","kind":"RoleBinding","metadata":{"name":"fixture","namespace":"nickeling-api"},
             "subjects":[{"kind":"ServiceAccount","name":"fixture","namespace":"nickeling-api"}],
             "roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"Role","name":"fixture"}}]}
        run(admin + ["create", "-f", "-"], input=json.dumps(resources))
        info = json.loads(run([str(kubectl), "--kubeconfig", str(kubeconfig), "config", "view", "--raw", "-o", "json"]).stdout)
        cluster = info["clusters"][0]["cluster"]
        if not cluster["server"].startswith("https://127.0.0.1:"):
            raise RuntimeError("Refusing non-loopback fixture endpoint")
        ca = output / "ca.pem"
        private(ca, base64.b64decode(cluster["certificate-authority-data"]).decode())
        token = output / "token"
        secrets.append(token)
        private(token, run(admin + ["-n", "nickeling-api", "create", "token", "fixture", "--duration=30m"]).stdout.strip())
        private(output / "target.json", json.dumps({"server":cluster["server"],"namespace":"nickeling-api",
            "caFile":str(ca),"tokenFile":str(token),"allowHttpLoopback":False}))
        summary["server_version"] = json.loads(run(admin + ["get", "--raw=/version"]).stdout)["gitVersion"]
        print(f"Real API ready: {summary['server_version']}; running Rust tests", flush=True)
        env = os.environ.copy()
        env.update(NICKELING_KIND_FIXTURE=str(output), CARGO_BUILD_JOBS="2", RUST_TEST_THREADS="1")
        env["CARGO_TARGET_DIR"] = target_directory
        command = ["cargo", "test", "--locked", "-p", "nickel-plane", "--no-default-features",
                   "--features", "kubernetes", "--test", "kubernetes_real", args.filter,
                   "--", "--ignored", "--nocapture", "--test-threads=1"]
        summary["tests_exit_code"] = test_command(command, env, output)
        with (output / "tests.log").open("rb") as log:
            log.seek(0, os.SEEK_END)
            log.seek(max(0, log.tell() - 16000))
            tail = log.read().decode("utf-8", errors="replace")
        summary["test_outcomes"] = [{"name":name,"outcome":outcome} for name, outcome in
            re.findall(r"^test ([A-Za-z0-9_:]+) \.\.\. (ok|FAILED|ignored)", tail, re.MULTILINE)]
        summary["source_changed_during_run"] = source_fingerprints() != fingerprints
        if summary["tests_exit_code"]:
            raise RuntimeError("Real Kubernetes API tests failed")
    finally:
        try:
            finish(kind, name, node, image_preexisting, summary, secrets, output, attempted)
        finally:
            for sig, handler in old_handlers.items():
                signal.signal(sig, handler)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, subprocess.TimeoutExpired, KeyboardInterrupt) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
