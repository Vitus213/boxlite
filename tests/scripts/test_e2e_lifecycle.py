"""Exercise actual child processes and environment ownership boundaries."""
import importlib.util
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("e2e_manage", ROOT / "apps/e2e/manage.py")
manage = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(manage)


class LifecycleTest(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.settings = patch.dict(os.environ, {"BOXLITE_E2E_STATE_DIR": self.scratch.name})
        self.settings.start()
        self.addCleanup(self.settings.stop)
        self.environment = manage.Environment()
        self.environment.initialize("check")

    def test_failed_child_preserves_its_exit_and_diagnostic(self):
        with self.assertRaises(manage.Failure) as failure:
            self.environment.command([sys.executable, "-c", "import sys; print('child failure'); sys.exit(7)"])
        self.assertEqual(failure.exception.code, 7)
        self.assertIn("child failure", (self.environment.run_dir / "commands.log").read_text())

    def test_timeout_stops_real_child_and_preserves_evidence(self):
        marker = Path(self.scratch.name) / "child.pid"
        program = "import os,pathlib,time; pathlib.Path(__import__('sys').argv[1]).write_text(str(os.getpid())); time.sleep(30)"
        with self.assertRaises(manage.Failure) as failure:
            self.environment.command([sys.executable, "-c", program, str(marker)], timeout=0.5)
        self.assertEqual(failure.exception.code, 124)
        pid = int(marker.read_text())
        result = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True)
        self.assertTrue(result.returncode != 0 or result.stdout.strip().startswith("Z"))

    def test_live_state_lock_refuses_second_owner(self):
        other = manage.Environment()
        with self.environment.locked():
            with self.assertRaises(manage.Failure) as failure:
                with other.locked():
                    self.fail("second environment acquired the lock")
        self.assertEqual(failure.exception.code, 2)
        with other.locked():
            pass

    def test_foreign_container_is_refused(self):
        with self.assertRaisesRegex(manage.Failure, "ownership conflict"):
            self.environment.assert_owner({"Name": "unrelated", "Config": {"Labels": {"boxlite.e2e.state": "/someone/else"}}})


if __name__ == "__main__":
    unittest.main()
