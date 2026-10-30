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

    def test_dependency_cache_reinstalls_only_when_sentinel_missing(self):
        scratch = Path(self.scratch.name)
        self.environment.python = str(scratch / "missing-python")
        calls = []

        def fake_make(target):
            calls.append(target)
            sentinel = {"test:e2e:deps:apps": "apps/node_modules/.bin/ts-node",
                        "test:e2e:deps:node": "sdks/node/node_modules/.bin/napi"}.get(target)
            if sentinel:
                (scratch / sentinel).parent.mkdir(parents=True, exist_ok=True)
                (scratch / sentinel).write_text("")

        with patch.object(manage, "ROOT", scratch), \
                patch.object(self.environment, "fingerprint", side_effect=lambda prefixes: "id"), \
                patch.object(self.environment, "capture", side_effect=lambda argv, **kwargs: ""), \
                patch.object(self.environment, "make", side_effect=fake_make):
            self.environment.prepare_dependencies()
            self.assertEqual(calls, ["test:e2e:deps", "test:e2e:deps:apps", "test:e2e:deps:node"])
            calls.clear()
            self.environment.prepare_dependencies()
            self.assertEqual(calls, ["test:e2e:deps"])
            calls.clear()
            (scratch / "apps/node_modules/.bin/ts-node").unlink()
            (scratch / "sdks/node/node_modules/.bin/napi").unlink()
            self.environment.prepare_dependencies()
            self.assertEqual(calls, ["test:e2e:deps", "test:e2e:deps:apps", "test:e2e:deps:node"])


if __name__ == "__main__":
    unittest.main()
