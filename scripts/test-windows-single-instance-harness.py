"""Focused readiness checks; never launches the Jesses executable."""

import ctypes
from ctypes import wintypes
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock


SPEC = importlib.util.spec_from_file_location(
    "single_instance_harness", Path(__file__).with_name("test-windows-single-instance.py")
)
HARNESS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HARNESS)


class Process:
    pid = 1234
    returncode = None

    def poll(self):
        return self.returncode


class WindowReadinessTests(unittest.TestCase):
    def test_history_lock_alone_never_marks_primary_ready(self):
        with tempfile.TemporaryDirectory() as temporary:
            persist = Path(temporary)
            history = persist / "data/jobs/jobs.json"
            history.parent.mkdir(parents=True)
            history.write_bytes(b"preserved")
            process = Process()
            with (
                mock.patch.object(HARNESS, "STARTUP_TIMEOUT", 0.06),
                mock.patch.object(HARNESS, "POLL_SECONDS", 0.005),
                mock.patch.object(HARNESS, "preserved_history", return_value=b"preserved"),
                mock.patch.object(HARNESS, "lock_is_held", return_value=True),
                mock.patch.object(HARNESS, "main_window", return_value=None),
            ):
                with self.assertRaisesRegex(HARNESS.CheckFailed, "responsive main window"):
                    HARNESS.await_primary(process, persist, {}, 0)

    def test_primary_needs_a_stable_responsive_window(self):
        with tempfile.TemporaryDirectory() as temporary:
            persist = Path(temporary)
            history = persist / "data/jobs/jobs.json"
            history.parent.mkdir(parents=True)
            history.write_bytes(b"preserved")
            window = {"handle": 42, "title": "jesses", "responsive": True}
            with (
                mock.patch.object(HARNESS, "STARTUP_TIMEOUT", 0.3),
                mock.patch.object(HARNESS, "WINDOW_STABLE_SECONDS", 0.02),
                mock.patch.object(HARNESS, "POLL_SECONDS", 0.005),
                mock.patch.object(HARNESS, "preserved_history", return_value=b"preserved"),
                mock.patch.object(HARNESS, "lock_is_held", return_value=True),
                mock.patch.object(HARNESS, "main_window", return_value=window),
            ):
                self.assertEqual(
                    HARNESS.await_primary(Process(), persist, {}, 0),
                    (b"preserved", window),
                )

    def test_replaced_window_does_not_satisfy_stability_interval(self):
        with tempfile.TemporaryDirectory() as temporary:
            persist = Path(temporary)
            history = persist / "data/jobs/jobs.json"
            history.parent.mkdir(parents=True)
            history.write_bytes(b"preserved")
            next_handle = iter(range(1000))

            def changing_window(_pid):
                return {"handle": next(next_handle), "responsive": True}

            with (
                mock.patch.object(HARNESS, "STARTUP_TIMEOUT", 0.06),
                mock.patch.object(HARNESS, "WINDOW_STABLE_SECONDS", 0.02),
                mock.patch.object(HARNESS, "POLL_SECONDS", 0.005),
                mock.patch.object(HARNESS, "preserved_history", return_value=b"preserved"),
                mock.patch.object(HARNESS, "lock_is_held", return_value=True),
                mock.patch.object(HARNESS, "main_window", side_effect=changing_window),
            ):
                with self.assertRaisesRegex(HARNESS.CheckFailed, "responsive main window"):
                    HARNESS.await_primary(Process(), persist, {}, 0)

    def test_secondary_cannot_replace_the_primary_window(self):
        with tempfile.TemporaryDirectory() as temporary:
            history = Path(temporary) / "jobs.json"
            history.write_bytes(b"preserved")
            primary = Process()
            secondary = Process()
            secondary.pid = 5678
            secondary.returncode = 0
            with (
                mock.patch.object(HARNESS, "lock_is_held", return_value=True),
                mock.patch.object(HARNESS, "main_window", return_value=None),
            ):
                with self.assertRaisesRegex(HARNESS.CheckFailed, "primary main window disappeared"):
                    HARNESS.secondary_exits(secondary, primary, history, history, b"preserved")

    def test_running_secondary_with_another_main_window_fails(self):
        with tempfile.TemporaryDirectory() as temporary:
            history = Path(temporary) / "jobs.json"
            history.write_bytes(b"preserved")
            primary = Process()
            secondary = Process()
            secondary.pid = 5678
            with (
                mock.patch.object(
                    HARNESS,
                    "main_window",
                    side_effect=lambda pid, **_: {"handle": 42} if pid == secondary.pid else {"handle": 11},
                ),
                mock.patch.object(HARNESS, "lock_is_held", return_value=True),
            ):
                with self.assertRaisesRegex(HARNESS.CheckFailed, "another main window"):
                    HARNESS.secondary_exits(secondary, primary, history, history, b"preserved")

    @unittest.skipUnless(os.name == "nt", "requires Win32")
    def test_hidden_top_level_window_is_found_by_owning_pid(self):
        user32 = ctypes.WinDLL("user32", use_last_error=True)
        user32.CreateWindowExW.argtypes = [
            wintypes.DWORD, wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.DWORD,
            ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
            wintypes.HWND, wintypes.HMENU, wintypes.HINSTANCE, wintypes.LPVOID,
        ]
        user32.CreateWindowExW.restype = wintypes.HWND
        user32.DestroyWindow.argtypes = [wintypes.HWND]
        user32.DestroyWindow.restype = wintypes.BOOL
        hwnd = user32.CreateWindowExW(
            0, "STATIC", "jesses", 0x00CF0000, 0, 0, 320, 240,
            None, None, None, None,
        )
        self.assertTrue(hwnd, ctypes.get_last_error())
        try:
            found = HARNESS.main_window(os.getpid())
            self.assertIsNotNone(found)
            self.assertEqual(found["handle"], hwnd)
            self.assertTrue(found["responsive"])
            self.assertFalse(found["visible"])
            self.assertGreater(found["clientWidth"], 0)
            self.assertGreater(found["clientHeight"], 0)
        finally:
            self.assertTrue(user32.DestroyWindow(hwnd))
        self.assertIsNone(HARNESS.main_window(os.getpid()))


if __name__ == "__main__":
    unittest.main()
