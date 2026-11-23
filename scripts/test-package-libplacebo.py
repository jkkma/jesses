"""Offline pinned processing closure and qualifier admission regressions."""

import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
from types import SimpleNamespace
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
RESTORE = load("processing_restore", "restore-libplacebo-ffmpeg-linux.py")


class ProcessingTests(unittest.TestCase):
    def test_both_platforms_bind_the_same_complete_source_lock(self):
        for platform in ("linux", "windows"):
            parent = json.loads((ROOT / f"scripts/package-ffmpeg-{platform}-lock.json").read_text())
            lock = BUILD.locked_sources(parent["processingSourceLock"])
            self.assertEqual({item["id"] for item in lock["libraries"]}, {"libplacebo", "glslang", *BUILD.SUBMODULES})
            for item in lock["libraries"]:
                self.assertTrue(item["url"].startswith("https://codeload.github.com/"))
                self.assertEqual(len(item["sha256"]), 64)
                self.assertTrue(item["licenseFiles"])

    def test_changed_source_lock_rejects(self):
        with self.assertRaises(ValueError):
            BUILD.locked_sources({"path": BUILD.LOCK.name, "sha256": "0" * 64})

    def test_retained_ffmpeg_restores_current_sources_and_resumes(self):
        recipe = load("test_ffmpeg_recipe", "build-package-ffmpeg-linux.py")
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            previous = root / "prior"
            retained = previous / "delivery"
            ffmpeg_source = previous / "work/ffmpeg/ffmpeg-9.1"
            ffmpeg_source.mkdir(parents=True)
            (ffmpeg_source / "RELEASE").write_text("9.1\n")
            (previous / "prefix").mkdir()
            meson = previous / "work/meson/meson-1"
            meson.mkdir(parents=True)
            (meson / "meson.py").touch()

            def retain(relative, content):
                path = retained / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(content)
                return recipe.record(path, retained, url="https://example.invalid/" + path.name)

            ffmpeg = retain("sources/ffmpeg.tar.xz", b"FFmpeg archive")
            meson_source = retain("sources/meson.tar.gz", b"Meson archive")
            old_glslang = retain("sources/glslang-old.tar.gz", b"old shader compiler")
            provenance = {"target": recipe.TARGET, "source": ffmpeg, "tools": [], "buildInputs": [],
                          "additionalSources": [meson_source, old_glslang],
                          "licenses": [retain("licenses/ffmpeg/LICENSE", b"FFmpeg license"),
                                       retain("licenses/meson/COPYING", b"Meson license"),
                                       retain("licenses/glslang/LICENSE", b"old shader compiler license")]}
            (retained / "build-provenance.json").write_text(json.dumps(provenance))
            lock_file = root / "ffmpeg-lock.json"
            lock = {"ffmpeg": {**ffmpeg, "sourceRoot": "ffmpeg-9.1", "version": "9.1", "licenseFiles": ["LICENSE"]},
                    "libraries": [], "buildTools": [{**meson_source, "id": "meson", "licenseFiles": ["COPYING"]}]}
            lock_file.write_text(json.dumps(lock))

            cache = root / "cache"
            cache.mkdir()
            shader_archive = cache / "glslang.tar.gz"
            contents = b"new shader compiler license"
            with tarfile.open(shader_archive, "w:gz") as archive:
                notice = tarfile.TarInfo("glslang-current/LICENSE")
                notice.size = len(contents)
                archive.addfile(notice, io.BytesIO(contents))
            shader = {"id": "glslang", "filename": shader_archive.name, "sha256": recipe.digest(shader_archive),
                      "url": "https://example.invalid/glslang.tar.gz", "sourceRoot": "glslang-current", "licenseFiles": ["LICENSE"]}
            processing_lock = root / "processing.json"
            processing_lock.write_text(json.dumps({"libraries": [shader]}))
            processing = SimpleNamespace(LOCK=processing_lock, sources=lambda builder: {"glslang": builder.source(shader)},
                                         build=lambda builder, records: (builder.directory / "build.log").touch())
            output = root / "restored"
            arguments = ["restore", "--previous-build", str(previous), "--build-directory", str(output), "--cache", str(cache)]
            with (patch.object(recipe, "LOCK", lock_file), patch.object(sys, "platform", "linux"),
                  patch.object(RESTORE, "module", side_effect=[recipe, processing] * 3),
                  patch.object(recipe.Builder, "ffmpeg", return_value=({}, [])) as compile_ffmpeg,
                  patch.object(recipe.Builder, "finish")):
                for options in ([], ["--resume"]):
                    with patch.object(sys, "argv", arguments + options):
                        RESTORE.main()
                    compile_ffmpeg.assert_called_with(ffmpeg_source, "9.1")
                lock["buildTools"][0]["sha256"] = "0" * 64
                lock_file.write_text(json.dumps(lock))
                with patch.object(sys, "argv", arguments + ["--resume"]):
                    with self.assertRaisesRegex(ValueError, "lacks locked source provenance for meson"):
                        RESTORE.main()
            self.assertEqual(compile_ffmpeg.call_count, 2)
            self.assertEqual((output / "delivery/licenses/glslang/LICENSE").read_bytes(), contents)
            self.assertFalse((output / "delivery/sources/glslang-old.tar.gz").exists())

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
