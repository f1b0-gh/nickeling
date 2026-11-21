"""Launch an isolated, networkless KVM host and record container qualification."""
import argparse
import base64
import hashlib
import io
import json
from pathlib import Path
import socket
import subprocess
import tarfile
import time

import vm_agent as VM


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--rootfs", type=Path, required=True)
    parser.add_argument("--qemu", required=True)
    parser.add_argument("--mkfs", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runtime", choices=["nspawn", "podman"], default="nspawn")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    disk = args.output / "root.ext4"
    with disk.open("wb") as handle:
        handle.truncate(8 * 1024**3)
    with (args.output / "mkfs.log").open("w") as log:
        subprocess.run([args.mkfs, "-q", "-F", "-t", "ext4", "-d", str(args.rootfs), str(disk)], check=True, stdout=log, stderr=log, timeout=300)
    kernel = next((args.rootfs / "usr/lib/modules").glob("*/vmlinuz"))
    agent = args.output / "agent.sock"
    command = [args.qemu, "-machine", "q35,accel=kvm", "-cpu", "host", "-smp", "2", "-m", "4096", "-nographic", "-no-reboot", "-monitor", "none", "-kernel", str(kernel), "-initrd", str(kernel.parent / "initramfs.img"), "-append", "root=/dev/sda console=ttyS0 panic=1", "-device", "virtio-scsi-pci,id=scsi0", "-drive", f"file={disk},format=raw,if=none,id=rootdisk", "-device", "scsi-hd,drive=rootdisk,bus=scsi0.0", "-nic", "none", "-device", "virtio-rng-pci", "-device", "virtio-serial-pci", "-chardev", f"socket,path={agent},server=on,wait=off,id=agent", "-device", "virtserialport,chardev=agent,name=org.qemu.guest_agent.0"]
    (args.output / "command.json").write_text(json.dumps(command, indent=2) + "\n")
    with (args.output / "serial.log").open("w") as serial, (args.output / "events.jsonl").open("w") as events:
        process = subprocess.Popen(command, stdout=serial, stderr=serial)
        connection = socket.socket(socket.AF_UNIX)
        try:
            deadline = time.monotonic() + 1200
            connection.close()
            guest = VM.connect_agent(agent, process, deadline, events)
            connection = guest.connection
            archive = io.BytesIO()
            with tarfile.open(fileobj=archive, mode="w:gz") as tar:
                for path in sorted(Path(__file__).parent.rglob("*")):
                    if path.is_file() and "__pycache__" not in path.parts:
                        tar.add(path, arcname=str(path.relative_to(Path(__file__).parent)))
            data = archive.getvalue()
            (args.output / "test-sources.tar.gz").write_bytes(data)
            (args.output / "test-sources.sha256").write_text(hashlib.sha256(data).hexdigest() + "\n")
            handle = guest.rpc("guest-file-open", {"path": "/run/qualification-tests.tar.gz", "mode": "w"})
            guest.rpc("guest-file-write", {"handle": handle, "buf-b64": base64.b64encode(data).decode()})
            guest.rpc("guest-file-close", {"handle": handle})
            guest.run("tar -xzf /run/qualification-tests.tar.gz -C /opt/qualification")
            script = "guest.py" if args.runtime == "nspawn" else "podman_guest.py"
            pid = guest.start(f"python3 /opt/qualification/{script} > /var/log/qualification.log 2>&1")
            last_log = ""
            while True:
                status = guest.rpc("guest-exec-status", {"pid": pid})
                output = guest.run("tail -c 60000 /var/log/qualification.log", check=False)
                if output["stdout"] != last_log:
                    last_log = output["stdout"]
                    (args.output / "qualification.log").write_text(last_log)
                if status.get("exited"):
                    result = {"exitcode": status.get("exitcode", -1)}
                    break
                time.sleep(2)
            output = guest.run("cat /var/log/qualification.log", check=False)
            (args.output / "qualification.log").write_text(output["stdout"] + output["stderr"])
            (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(output["stdout"][-14000:])
            if result["exitcode"]:
                raise RuntimeError("Container qualification failed; see qualification.log")
        finally:
            connection.close()
            process.terminate()
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=10)


if __name__ == "__main__":
    main()
