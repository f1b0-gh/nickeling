"""Check delivered container contents with root-relative symlink resolution."""
import argparse
import json
import re
from pathlib import Path
import unittest


def inside(path):
    parts = list(Path(path).parts[1:])
    resolved = []
    for _ in range(200):
        if not parts:
            return ROOT.joinpath(*resolved)
        part = parts.pop(0)
        if part == "..":
            if not resolved:
                raise AssertionError("symlink escapes container root")
            resolved.pop()
            continue
        candidate = ROOT.joinpath(*resolved, part)
        if candidate.is_symlink():
            target = candidate.readlink()
            if target.is_absolute():
                resolved = []
                parts = list(target.parts[1:]) + parts
            else:
                parts = list(target.parts) + parts
        else:
            resolved.append(part)
    raise AssertionError("symlink depth exceeded")


class RootfsTests(unittest.TestCase):
    def test_selected_commands_resolve_inside_root(self):
        self.assertTrue(ROOT.is_dir(), "container-rootfs output is missing")
        for name in ["nickel-plane", "systemd", "nix", "nix-store", "nickel", "bwrap", "prlimit", "sh"]:
            with self.subTest(name=name):
                self.assertTrue(inside("/usr/bin/" + name).is_file())
        self.assertEqual(inside("/usr/bin/tofu").is_file(), "opentofu" in ADAPTERS)
        if "opentofu" in ADAPTERS:
            self.assertFalse((ROOT / "usr/bin/tofu").is_symlink(), "Execution target must satisfy nickel-util's no-symlink file policy")
        self.assertFalse((ROOT / "usr/bin/kubectl").exists())

    def test_container_excludes_boot_components(self):
        for path in ["usr/lib/modules", "usr/lib/ostree", "usr/bin/bootc", "usr/bin/qemu-ga", "etc/nftables.conf"]:
            self.assertFalse((ROOT / path).exists() or (ROOT / path).is_symlink(), path)
        closure = json.loads((ROOT / "usr/share/nickel-container/closure.json").read_text())
        forbidden = ("bootc-", "ostree-", "qemu-", "gcc-wrapper-", "rustc-", "cargo-")
        self.assertFalse([p for p in closure if Path(p).name[33:].startswith(forbidden) or re.match(r"linux-[0-9]", Path(p).name[33:])])

    def test_profile_and_isolated_layout(self):
        profile = json.loads((ROOT / "usr/share/nickel-container/profile.json").read_text())
        self.assertEqual(profile["adapters"], ADAPTERS)
        self.assertEqual((ROOT / "nix").readlink(), Path("usr/lib/nix"))
        self.assertEqual((ROOT / "usr/lib/nix/var").readlink(), Path("/var/lib/nickel-nix"))
        self.assertEqual((ROOT / "etc/machine-id").read_text(), "")
        self.assertIn("nickel-plane:x:1000:1000:", (ROOT / "etc/passwd").read_text())
        self.assertIn("systemd-journal:x:190:", (ROOT / "etc/group").read_text())
        for adapter, directory in [("opentofu", "tf"), ("kubernetes", "k8s")]:
            self.assertEqual((ROOT / "opt/nickel" / directory).exists(), adapter in ADAPTERS)

    def test_no_host_services_or_daemon_enabled(self):
        enabled = [p.name for p in (ROOT / "etc/systemd/system/multi-user.target.wants").iterdir()]
        self.assertEqual(sorted(enabled), ["nickel-plane.service"])
        self.assertFalse(inside("/nix/var/nix/daemon-socket/socket").exists())
        self.assertTrue((ROOT / "usr/share/nickel-container/registration").is_file())


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--rootfs", type=Path, required=True)
    parser.add_argument("--adapters", default="opentofu,kubernetes")
    args, rest = parser.parse_known_args()
    ROOT, ADAPTERS = args.rootfs, args.adapters.split(",")
    unittest.main(argv=[__file__, *rest])
