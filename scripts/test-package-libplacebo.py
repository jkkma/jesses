"""Offline pinned processing closure and qualifier admission regressions."""

import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / filename)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


BUILD = load("processing_build", "build-package-libplacebo.py")
QUALIFY = load("processing_qualify", "qualify-libplacebo.py")


class ProcessingTests(unittest.TestCase):
    def test_both_platforms_bind_the_same_complete_source_and_patch_lock(self):
        for platform in ("linux", "windows"):
            parent = json.loads((ROOT / f"scripts/package-ffmpeg-{platform}-lock.json").read_text())
            lock = BUILD.locked_sources(parent["processingSourceLock"])
            self.assertEqual({item["id"] for item in lock["libraries"]}, {"libplacebo", "glslang", *BUILD.SUBMODULES})
            for item in lock["libraries"]:
                self.assertTrue(item["url"].startswith("https://codeload.github.com/"))
                self.assertEqual(len(item["sha256"]), 64)
                self.assertTrue(item["licenseFiles"])
            for item in lock["compatibilityPatches"]:
                self.assertEqual(hashlib.sha256((ROOT / "scripts" / item["path"]).read_bytes()).hexdigest(), item["sha256"])

    def test_changed_source_lock_and_unexpected_generator_call_reject(self):
        with self.assertRaises(ValueError):
            BUILD.locked_sources({"path": BUILD.LOCK.name, "sha256": "0" * 64})
        with self.assertRaises(ValueError):
            BUILD.compatible_source(BUILD.PYTHON314_FILE, b"registry = changed()")
        original = b"before\n" + BUILD.PYTHON314_BEFORE + b"\nafter\n"
        self.assertEqual(BUILD.compatible_source(BUILD.PYTHON314_FILE, original), original.replace(BUILD.PYTHON314_BEFORE, BUILD.PYTHON314_AFTER))

    def test_compiled_report_does_not_require_a_physical_device(self):
        with tempfile.TemporaryDirectory() as temporary:
            executable = Path(temporary) / "ffmpeg"
            executable.write_bytes(b"pinned-provider")
            def output(args, **kwargs):
                if "-version" in args:
                    text = "ffmpeg version9\nconfiguration: --enable-vulkan --enable-libplacebo"
                elif "-hwaccels" in args:
                    text = "Hardware acceleration methods:\nvulkan\n"
                else:
                    text = "Filterlibplacebo tonemapping spline peak_detect apply_dolbyvision"
                return subprocess.CompletedProcess(args, 0, text.encode(), b"")
            with patch.object(QUALIFY.subprocess, "run", side_effect=output) as run:
                report = QUALIFY.qualify(executable)
            self.assertFalse(report["runtimeExercised"])
            self.assertNotIn("physicalGpu", report)
            self.assertTrue(report["spline"])
            self.assertEqual(run.call_count, 3)

    def test_filter_name_alone_cannot_qualify_missing_spline_or_vulkan(self):
        with tempfile.TemporaryDirectory() as temporary:
            executable = Path(temporary) / "ffmpeg"
            executable.write_bytes(b"wrong-provider")
            with patch.object(QUALIFY.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, b"libplacebo", b"")):
                with self.assertRaisesRegex(ValueError, "explicitly enable"):
                    QUALIFY.qualify(executable)


if __name__ == "__main__":
    unittest.main()
