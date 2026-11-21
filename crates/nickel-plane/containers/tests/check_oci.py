"""Check the delivered image identity and the real Quadlet generator output."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import tarfile
import unittest


class ImageTests(unittest.TestCase):
    def test_image_contains_existing_rootfs(self):
        with tarfile.open(ARGS.archive) as archive:
            manifest = json.load(archive.extractfile("manifest.json"))
            self.assertEqual(len(manifest), 1)
            config_bytes = archive.extractfile(manifest[0]["Config"]).read()
            image_id = "sha256:" + hashlib.sha256(config_bytes).hexdigest()
            self.assertEqual((ARGS.quadlet / "image-id").read_text().strip(), image_id)
            config = json.loads(config_bytes)
            self.assertEqual(config["architecture"], "amd64")
            self.assertEqual(config["config"]["Cmd"], ["/usr/bin/init"])
            self.assertEqual(config["config"]["StopSignal"], "SIGRTMIN+3")
            self.assertEqual(len(manifest[0]["Layers"]), 1)
            with tarfile.open(fileobj=archive.extractfile(manifest[0]["Layers"][0])) as layer:
                entries = {p.name.removeprefix("./"): p for p in layer.getmembers()}
                self.assertEqual(entries["nix"].linkname, "usr/lib/nix")
                for name in ["etc/machine-id", "usr/libexec/nickel-nix-init",
                             "usr/lib/systemd/system/nickel-plane.service",
                             "usr/share/nickel-container/profile.json"]:
                    self.assertEqual(layer.extractfile(entries[name]).read(), (ARGS.rootfs / name).read_bytes())

    def test_quadlet_generates_confined_rootless_service(self):
        result = subprocess.run([ARGS.generator, "-user", "-dryrun"],
                                env={**os.environ, "QUADLET_UNIT_DIRS": str(ARGS.quadlet)},
                                capture_output=True, text=True, timeout=30, check=True)
        lines = [line for line in result.stdout.splitlines() if line.startswith("ExecStart=")]
        self.assertEqual(len(lines), 1, result.stdout + result.stderr)
        command = shlex.split(lines[0].removeprefix("ExecStart="))
        for key, value in [("--userns", "keep-id:uid=1000,gid=1000"),
                           ("--user", "0"), ("--network", "none"), ("--pull", "never")]:
            self.assertEqual(command[command.index(key) + 1], value)
        self.assertIn("--read-only", command)
        self.assertIn("--systemd=always", command)
        self.assertIn("--security-opt=no-new-privileges", command)
        self.assertIn("--replace=false", command)
        self.assertIn("--cidfile=%t/%N/${INVOCATION_ID}.cid", command)
        self.assertIn("ConditionPathExists=%h/.config/systemd/user/%N.service.d/10-container-id.conf", result.stdout)
        self.assertEqual(command[command.index("--cap-add") + 1], "sys_admin")
        self.assertIn((ARGS.quadlet / "image-id").read_text().strip(), command)
        self.assertNotIn("--privileged", command)
        self.assertFalse(any("unconfined" in arg or "unmask" in arg for arg in command))
        volumes = [command[i + 1] for i, arg in enumerate(command) if arg == "-v" or arg == "--volume"]
        self.assertEqual(len(volumes), 2)
        self.assertFalse(any("U" in volume.rsplit(":", 1)[-1].split(",") for volume in volumes))
        self.assertTrue(any(volume.endswith(":/etc/nickel-plane:ro") for volume in volumes))
        self.assertTrue(any(volume.endswith(":/var/lib/nickel-plane:rw") for volume in volumes))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--rootfs", type=Path, required=True)
    parser.add_argument("--quadlet", type=Path, required=True)
    parser.add_argument("--generator", required=True)
    ARGS, rest = parser.parse_known_args()
    unittest.main(argv=[__file__, *rest])
