"""Standalone encoder staging must retain its verified shared source closure."""

import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("stager", Path(__file__).with_name("stage-bundled-tools.py"))
stager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(stager)


class SharedSourceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="jesses-standalone-stage-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.tools = self.root / "tools"
        (self.tools / "ffmpeg").mkdir(parents=True)
        self.delivery = self.root / "delivery"
        self.delivery.mkdir()
        source = {"base": "mingw-w64-x264", "path": "sources/x264.tar.zst", "sha256": "6a4d3620201074edec84681aeb25ffa1eceaa9451f7347ca1448f532634eed55"}
        runtime = [{"base": "mingw-w64-" + name, "path": f"sources/{name}.tar.zst", "sha256": "a" * 64} for name in ("gcc", "crt", "headers", "winpthreads")]
        self.media_receipt = self.tools / "ffmpeg/build-provenance.json"
        self.media_receipt.write_text(json.dumps({"additionalSources": [source, *runtime], "licenses": [], "buildInputs": []}), encoding="utf-8")
        for name in ("x264.exe", "COPYING", "build-package-x264.py"):
            (self.delivery / name).write_bytes(b"synthetic test payload")
        def record(name):
            return {"path": name, "sha256": stager.digest(self.delivery / name)}
        self.receipt = {"schemaVersion": 1, "target": "x86_64-pc-windows-msvc", "tool": {"id": "x264", "version": "x264 0.165.3222 b35605a", **record("x264.exe")}, "sharedMediaProvenanceSha256": stager.digest(self.media_receipt), "sourceCommit": "b35605ace3ddf7c1a5d67a2eb553f034aef41d55", "source": source, "runtimeSources": runtime, "licenses": [record("COPYING")], "buildInputs": [record("build-package-x264.py")]}
        self.write_receipt()

    def write_receipt(self):
        (self.delivery / "build-provenance.json").write_text(json.dumps(self.receipt), encoding="utf-8")

    def stage(self):
        return stager.stage_x264(self.delivery, self.tools, "x86_64-pc-windows-msvc")

    def test_shared_sources_are_referenced_without_duplicate_archives(self):
        with patch.object(stager.subprocess, "check_output", return_value="x264 0.165.3222 b35605a\n"):
            result = self.stage()
        self.assertEqual(result["source"]["path"], "ffmpeg/sources/x264.tar.zst")
        self.assertEqual(len(result["additionalSources"]), 4)
        self.assertTrue(all(item["path"].startswith("ffmpeg/sources/") for item in result["additionalSources"]))
        self.assertFalse((self.tools / "x264/sources").exists())

    def test_changed_binary_is_rejected_before_execution(self):
        (self.delivery / "x264.exe").write_bytes(b"changed executable")
        with patch.object(stager.subprocess, "check_output") as execute, self.assertRaisesRegex(ValueError, "inventory"):
            self.stage()
        execute.assert_not_called()

    def test_different_shared_delivery_is_rejected(self):
        self.media_receipt.write_text("{}", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "exact shared"):
            self.stage()

    def test_missing_runtime_source_is_rejected(self):
        self.receipt["runtimeSources"].pop()
        self.write_receipt()
        with self.assertRaisesRegex(ValueError, "incomplete"):
            self.stage()

    def test_linux_uses_shared_codec_source_and_explicit_system_runtime(self):
        self.receipt.update(target="x86_64-unknown-linux-gnu", runtimeSources=[], systemDependencies=["libc.so.6", "libm.so.6", "ld-linux-x86-64.so.2"])
        self.write_receipt()
        with patch.object(stager.subprocess, "check_output", return_value="x264 0.165.3222 b35605a\n"):
            result = stager.stage_x264(self.delivery, self.tools, "x86_64-unknown-linux-gnu")
        self.assertEqual(result["source"]["path"], "ffmpeg/sources/x264.tar.zst")
        self.assertEqual(result["additionalSources"], [])

    def test_linux_rejects_an_unpackaged_codec_dependency(self):
        self.receipt.update(target="x86_64-unknown-linux-gnu", runtimeSources=[], systemDependencies=["libc.so.6", "libunpackaged-codec.so.1"])
        self.write_receipt()
        with patch.object(stager.subprocess, "check_output") as execute, self.assertRaisesRegex(ValueError, "system dependency"):
            stager.stage_x264(self.delivery, self.tools, "x86_64-unknown-linux-gnu")
        execute.assert_not_called()


if __name__ == "__main__":
    unittest.main()
