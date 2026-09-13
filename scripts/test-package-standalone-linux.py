"""Cross-platform integrity and environment tests for Linux standalone builds."""

import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location("linux_standalone", Path(__file__).with_name("build-package-standalone-linux.py"))
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)


class StandaloneSourceTests(unittest.TestCase):
    def test_build_environment_excludes_host_compiler_shell_and_dependency_injection(self):
        hostile = {name: "unrelated-host-input" for name in ["CMAKE_PROJECT_INCLUDE", "CMAKE_TOOLCHAIN_FILE", "CMAKE_GENERATOR", "CC", "CXX", "CFLAGS", "LDFLAGS", "LD_PRELOAD", "LD_LIBRARY_PATH", "BASH_ENV", "ENV", "MAKEFLAGS", "MFLAGS", "PKG_CONFIG_PATH", "GIT_CONFIG_PARAMETERS", "GIT_CONFIG_COUNT", "GIT_CONFIG_KEY_0", "GIT_TEMPLATE_DIR", "PYTHONPATH"]}
        with patch.dict(BUILD.os.environ, hostile):
            result = BUILD.environment(Path("/tmp/owned-home"), True)
        self.assertFalse(any(value == "unrelated-host-input" for value in result.values()))
        self.assertEqual(result["PATH"], "/usr/bin:/bin")
        self.assertEqual(result["HOME"], str(Path("/tmp/owned-home")))
        self.assertEqual(result["GIT_CONFIG_GLOBAL"], BUILD.os.devnull)

    def test_complete_shared_receipt_rejects_tamper_extra_duplicate_and_wrong_platform(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            def item(name, value, **extra):
                (root / name).write_bytes(value)
                return {**extra, "path": name, "sha256": BUILD.MEDIA.digest(root / name)}
            source = item("media.tar", b"official source fixture")
            x264 = item("x264.tar", b"exact source fixture")
            receipt = {"schemaVersion": 1, "target": BUILD.TARGET, "source": source, "tools": [item("ffmpeg", b"media program", id="ffmpeg"), item("ffprobe", b"probe program", id="ffprobe")], "additionalSources": [x264], "licenses": [item("COPYING", b"source license")], "buildInputs": [item("recipe.py", b"reproducible recipe")]}
            path = root / "build-provenance.json"
            def write():
                path.write_text(json.dumps(receipt), encoding="utf-8")
            with patch.object(BUILD, "FFMPEG_SHA", source["sha256"]), patch.object(BUILD, "X264_SHA", x264["sha256"]):
                write()
                self.assertEqual(BUILD.verify_media(root)[1], x264)
                (root / "ffmpeg").write_bytes(b"changed program")
                with self.assertRaisesRegex(ValueError, "hash receipt"):
                    BUILD.verify_media(root)
                self.assertEqual((root / "ffmpeg").read_bytes(), b"changed program")
                (root / "ffmpeg").write_bytes(b"media program")
                (root / "unlisted").write_bytes(b"extra payload")
                with self.assertRaisesRegex(ValueError, "hash receipt"):
                    BUILD.verify_media(root)
                (root / "unlisted").unlink()
                receipt["buildInputs"].append(receipt["licenses"][0])
                write()
                with self.assertRaisesRegex(ValueError, "repeats"):
                    BUILD.verify_media(root)
                receipt["buildInputs"].pop()
                receipt["target"] = "x86_64-pc-windows-msvc"
                write()
                with self.assertRaisesRegex(ValueError, "Linux FFmpeg"):
                    BUILD.verify_media(root)

    def test_existing_work_is_not_replaced(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            build = root / "build"
            build.mkdir()
            original = build / "failed-build.log"
            original.write_bytes(b"keep diagnostic evidence")
            with self.assertRaises(FileExistsError):
                BUILD.Build(build, root / "cache", 2, False)
            self.assertEqual(original.read_bytes(), b"keep diagnostic evidence")


if __name__ == "__main__":
    unittest.main()
