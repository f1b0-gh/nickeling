"""Lifecycle refusals use temporary files and a fake external process boundary."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("kind_driver", Path(__file__).parents[1] / "test-kind.py")
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)


class CleanupTests(unittest.TestCase):
    def test_failed_cluster_cleanup_still_removes_credentials_and_records_owner(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            secrets = [root / "token", root / "kubeconfig"]
            for path in secrets:
                driver.private(path, "private fixture credential")
            with patch.object(driver, "cleanup", side_effect=RuntimeError("Docker unavailable")):
                with self.assertRaisesRegex(RuntimeError, "cluster cleanup failed"):
                    driver.finish("kind", "owned-cluster", "owned-node", True, {}, secrets, root, True)
            self.assertTrue(all(not path.exists() for path in secrets))
            result = json.loads((root / "result.json").read_text())
            self.assertEqual(result["remaining_owned_cluster"], "owned-cluster")
            self.assertNotIn("private fixture credential", (root / "result.json").read_text())

    def test_report_failure_cannot_skip_cluster_or_credential_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            token = root / "token"
            driver.private(token, "private fixture token")
            with patch.object(driver, "cleanup") as cleanup, patch.object(driver, "report", side_effect=OSError("full disk")):
                with self.assertRaisesRegex(RuntimeError, "PDF report"):
                    driver.finish("kind", "owned", "owned-control-plane", True, {}, [token], root, True)
            cleanup.assert_called_once()
            self.assertFalse(token.exists())
            self.assertEqual(json.loads((root / "result.json").read_text())["finalization_errors"], ["could not write PDF report"])

    def test_wrong_cluster_label_refuses_all_deletion(self):
        def external(command, **_kwargs):
            self.assertEqual(command, ["docker", "inspect", "candidate-node"])
            return subprocess.CompletedProcess(command, 0, json.dumps([{
                "Config":{"Labels":{"io.x-k8s.kind.cluster":"someone-elses-cluster"}}
            }]), "")
        with patch.object(driver, "run", side_effect=external) as calls:
            with self.assertRaisesRegex(RuntimeError, "Refusing cleanup"):
                driver.cleanup("kind", "owned-cluster", "candidate-node", True, {})
            self.assertEqual(calls.call_count, 1)

    def test_preexisting_image_is_preserved(self):
        def external(command, **_kwargs):
            if command[:2] == ["docker", "inspect"]:
                return subprocess.CompletedProcess(command, 1, "", "")
            self.assertNotIn("image", command)
            return subprocess.CompletedProcess(command, 0, "", "")
        result = {}
        with patch.object(driver, "run", side_effect=external):
            driver.cleanup("kind", "owned", "owned-control-plane", True, result)
        self.assertTrue(result["cluster_removed"])
        self.assertEqual(result["image_retained"], "preexisting cache")

    def test_new_image_shared_by_another_container_is_preserved(self):
        def external(command, **_kwargs):
            if command[:2] == ["docker", "inspect"]:
                return subprocess.CompletedProcess(command, 1, "", "")
            self.assertNotEqual(command[:3], ["docker", "image", "rm"])
            stdout = "another-container\n" if f"ancestor={driver.IMAGE}" in command else ""
            return subprocess.CompletedProcess(command, 0, stdout, "")
        result = {}
        with patch.object(driver, "run", side_effect=external):
            driver.cleanup("kind", "owned", "owned-control-plane", False, result)
        self.assertTrue(result["cluster_removed"])
        self.assertEqual(result["image_retained"], "in use by another container")


class IsolationTests(unittest.TestCase):
    def test_remote_context_overrides_local_docker_host_and_is_refused(self):
        result = subprocess.CompletedProcess([], 0, json.dumps([{
            "Endpoints":{"docker":{"Host":"ssh://external-host"}}
        }]), "")
        with patch.object(driver, "run", return_value=result) as calls:
            with self.assertRaisesRegex(RuntimeError, "remote Docker"):
                driver.local_docker_endpoint({"DOCKER_CONTEXT":"remote", "DOCKER_HOST":"unix:///var/run/docker.sock"})
        calls.assert_called_once_with(["docker", "context", "inspect", "remote"])

    def test_local_context_overrides_remote_docker_host(self):
        result = subprocess.CompletedProcess([], 0, json.dumps([{
            "Endpoints":{"docker":{"Host":"unix:///var/run/docker.sock"}}
        }]), "")
        with patch.object(driver, "run", return_value=result) as calls:
            self.assertEqual(driver.local_docker_endpoint({"DOCKER_CONTEXT":"local", "DOCKER_HOST":"tcp://external-host:2376"}), "unix:///var/run/docker.sock")
        calls.assert_called_once_with(["docker", "context", "inspect", "local"])

    def test_remote_host_without_context_is_refused_without_contact(self):
        with patch.object(driver, "run") as calls:
            with self.assertRaisesRegex(RuntimeError, "remote Docker"):
                driver.local_docker_endpoint({"DOCKER_HOST":"tcp://external-host:2376"})
        calls.assert_not_called()

    def test_target_path_rejects_crate_tree_and_symlink_parent(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "crates" / "nickel-plane").mkdir(parents=True)
            (root / "build").mkdir()
            (root / "link").symlink_to(root / "build", target_is_directory=True)
            with patch.object(driver, "ROOT", root):
                self.assertEqual(driver.cargo_target_directory("build/kind"), str(root / "build/kind"))
                with self.assertRaisesRegex(RuntimeError, "Nix crate"):
                    driver.cargo_target_directory("crates/nickel-plane/target")
                with self.assertRaisesRegex(RuntimeError, "symlinks"):
                    driver.cargo_target_directory("link/target")


class ProcessTests(unittest.TestCase):
    def test_zero_matching_real_tests_cannot_report_success(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(RuntimeError, "ran no tests"):
                driver.test_command([sys.executable, "-c", "print('test result: ok. 0 passed; 0 failed;')"],
                                    os.environ.copy(), Path(directory))

    def test_timeout_terminates_the_child_process_group(self):
        with tempfile.TemporaryDirectory() as directory:
            pid_file = Path(directory) / "child.pid"
            code = "import pathlib,subprocess,sys,time; child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(30)']); pathlib.Path(sys.argv[1]).write_text(str(child.pid)); time.sleep(30)"
            with self.assertRaises(subprocess.TimeoutExpired):
                driver.run([sys.executable, "-c", code, str(pid_file)], timeout=1)
            self.assertTrue(pid_file.exists(), "fixture child did not start before the timeout")
            pid = int(pid_file.read_text())
            try:
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline:
                    status = Path(f"/proc/{pid}/stat")
                    if not status.exists() or status.read_text().split()[2] == "Z":
                        break
                    time.sleep(0.01)
                else:
                    self.fail("timed-out command left a running descendant")
            finally:
                try:
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass


if __name__ == "__main__":
    unittest.main()
