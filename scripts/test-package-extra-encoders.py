"""Failure-injection checks for additional Windows standalone deliveries."""

import importlib.util
import io
import json
from pathlib import Path
import shutil
import sys
import tempfile
import tarfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("stager", HERE / "stage-bundled-tools.py")
stager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(stager)
builder_spec = importlib.util.spec_from_file_location("builder", HERE / "build-package-extra-encoders.py")
builder = importlib.util.module_from_spec(builder_spec)
builder_spec.loader.exec_module(builder)


class AomSourceTreeTests(unittest.TestCase):
    def test_archive_timestamps_do_not_change_pinned_file_tree(self):
        with tempfile.TemporaryDirectory(prefix="jesses-aom-tree-test-") as root:
            paths = [Path(root) / f"archive-{index}.tar.gz" for index in range(3)]
            for index, path in enumerate(paths):
                with tarfile.open(path, "w:gz") as archive:
                    data = b"same pinned source" if index < 2 else b"modified source"
                    member = tarfile.TarInfo("common/y4minput.c")
                    member.size = len(data)
                    member.mtime = 100 + index
                    archive.addfile(member, io.BytesIO(data))
            self.assertEqual(builder.aom_tree_sha256(paths[0]), builder.aom_tree_sha256(paths[1]))
            self.assertNotEqual(builder.aom_tree_sha256(paths[0]), builder.aom_tree_sha256(paths[2]))


class ExtraEncoderStageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="jesses-extra-encoder-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.tools = self.root / "tools"
        media = self.tools / "ffmpeg"
        media.mkdir(parents=True)
        (media / "build/scripts").mkdir(parents=True)
        self.delivery = self.root / "delivery"
        self.delivery.mkdir()
        runtime = [{"base": "mingw-w64-" + base, "path": f"sources/{base}.tar.zst", "sha256": "a" * 64}
                   for base in ("gcc", "crt", "headers", "winpthreads")]
        source = {"base": "mingw-w64-x265", "version": "4.3-1", "path": "sources/x265.src.tar.zst", "sha256": "b" * 64}
        license_record = {"path": "licenses/mingw-w64-x265/COPYING", "sha256": "c" * 64}
        self.media_receipt = media / "build-provenance.json"
        self.media_receipt.write_text(json.dumps({"additionalSources": [*runtime, source],
                                                  "licenses": [license_record], "buildInputs": []}), encoding="utf-8")
        for name, contents in (("x265.exe", b"encoder"), ("libstdc++-6.dll", b"runtime"),
                               ("binary-package.pkg.tar.zst", b"public package"),
                               ("build-package-extra-encoders.py", b"recipe")):
            (self.delivery / name).write_bytes(contents)
        shutil.copy2(HERE / "standalone-tool-sources.json", self.delivery / "standalone-tool-sources.json")
        def record(name):
            return {"path": name, "sha256": stager.digest(self.delivery / name)}
        binary = {"name": "mingw-w64-ucrt-x86_64-x265", "version": "4.3-1",
                  "sha256": stager.digest(self.delivery / "binary-package.pkg.tar.zst")}
        (media / "build/scripts/package-ffmpeg-windows-lock.json").write_text(
            json.dumps({"packages": [binary]}), encoding="utf-8")
        self.receipt = {"schemaVersion": 1, "target": "x86_64-pc-windows-msvc", "id": "x265",
                        "tool": {"id": "x265", "version": "x265 [info]: HEVC encoder version 4.3", **record("x265.exe")},
                        "sharedMediaProvenanceSha256": stager.digest(self.media_receipt),
                        "source": source, "runtimeSources": runtime, "licenses": [license_record],
                        "supportFiles": [record("libstdc++-6.dll")],
                        "binaryPackage": {**binary, "path": "binary-package.pkg.tar.zst"},
                        "buildInputs": [record("build-package-extra-encoders.py"),
                                        record("standalone-tool-sources.json")]}
        self.write_receipt()

    def write_receipt(self):
        (self.delivery / "build-provenance.json").write_text(json.dumps(self.receipt), encoding="utf-8")

    def stage(self):
        with patch.object(stager.subprocess, "run") as run:
            run.return_value.stdout = self.receipt["tool"]["version"]
            run.return_value.stderr = ""
            return stager.stage_extra_encoder(self.delivery, self.tools, "x86_64-pc-windows-msvc", "x265")

    def test_retains_public_binary_package_and_shared_source(self):
        result = self.stage()
        self.assertEqual(result["path"], "x265/x265.exe")
        self.assertEqual(result["source"]["path"], "ffmpeg/sources/x265.src.tar.zst")
        self.assertEqual(result["supportFiles"][0]["path"], "x265/libstdc++-6.dll")

    def test_rejects_changed_runtime_dll_before_execution(self):
        (self.delivery / "libstdc++-6.dll").write_bytes(b"changed")
        with patch.object(stager.subprocess, "run") as run, self.assertRaisesRegex(ValueError, "payload"):
            stager.stage_extra_encoder(self.delivery, self.tools, "x86_64-pc-windows-msvc", "x265")
        run.assert_not_called()

    def test_rejects_changed_shared_media_receipt(self):
        self.media_receipt.write_text("{}", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "exact shared"):
            self.stage()

    def test_rejects_missing_runtime_source(self):
        self.receipt["runtimeSources"].pop()
        self.write_receipt()
        with self.assertRaisesRegex(ValueError, "incomplete"):
            self.stage()

    def test_rejects_wrong_target(self):
        with self.assertRaisesRegex(ValueError, "Windows x64"):
            stager.stage_extra_encoder(self.delivery, self.tools, "x86_64-unknown-linux-gnu", "x265")


if __name__ == "__main__":
    unittest.main()
