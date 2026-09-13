"""Portable runtime staging and source extraction must fail before unsafe use."""

import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("delivery", Path(__file__).with_name("package-av1an-delivery.py"))
delivery = importlib.util.module_from_spec(spec)
spec.loader.exec_module(delivery)
support = delivery.support


class ExtractionTests(unittest.TestCase):
    def test_compiler_and_frameserver_environment_are_not_inherited(self):
        with patch.dict(os.environ, {"C_INCLUDE_PATH": "wrong compiler", "PYTHONPATH": "wrong runtime", "BASH_ENV": "wrong script", "CMAKE_TOOLCHAIN_FILE": "wrong linker", "VSSCRIPT_PATH": "wrong DLL", "PATH": "wrong executables", "SYSTEMROOT": "Windows"}, clear=True):
            env = support.isolated_environment()
        self.assertEqual(env["SYSTEMROOT"], "Windows")
        self.assertTrue(all(name not in env for name in ("C_INCLUDE_PATH", "PYTHONPATH", "BASH_ENV", "CMAKE_TOOLCHAIN_FILE", "VSSCRIPT_PATH", "PATH")))

    def test_archive_traversal_and_links_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "bad.zip"
            with zipfile.ZipFile(archive, "w") as stream:
                stream.writestr("../escaped", "not allowed")
            with self.assertRaisesRegex(ValueError, "Unsafe"):
                support.unpack(archive, root / "zip-output")
            archive = root / "bad.tar.gz"
            with tarfile.open(archive, "w:gz") as stream:
                member = tarfile.TarInfo("source/link")
                member.type, member.linkname = tarfile.SYMTYPE, "../../escaped"
                stream.addfile(member)
            with self.assertRaisesRegex(ValueError, "non-regular"):
                support.unpack(archive, root / "tar-output")
            self.assertFalse((root / "escaped").exists())

    def test_notices_are_retained_without_extracting_unrelated_source(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "source.tar.gz"
            with tarfile.open(archive, "w:gz") as stream:
                for name, data in [("source/COPYING", b"complete notice"), ("source/code.c", b"code")]:
                    member = tarfile.TarInfo(name)
                    member.size = len(data)
                    stream.addfile(member, io.BytesIO(data))
            notices = support.source_notices(archive, root / "licenses")
            self.assertEqual([path.read_bytes() for path in notices], [b"complete notice"])


class StagingTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="jesses-av1an-stage-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.directory, self.tools = self.root / "delivery", self.root / "tools"
        self.directory.mkdir()
        (self.tools / "ffmpeg").mkdir(parents=True)
        shared = [{"base": base, "path": f"sources/{base}.tar.zst", "sha256": "a" * 64} for base in sorted(delivery.SHARED_BASES)]
        media = self.tools / "ffmpeg/build-provenance.json"
        media.write_text(json.dumps({"additionalSources": shared, "licenses": [], "buildInputs": []}))
        self.version = "av1an 0.5.2 [ffmpeg9-passthrough-v1] [julek-butteraugli-v1]"
        def record(name):
            path = self.directory / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"synthetic payload")
            return {"path": name, "sha256": support.digest(path)}
        source = record("sources/source.tar.gz")
        # This fixture mocks the reviewed upstream source identity only. Every
        # executable, dependency and build/notice payload uses its real digest.
        source["sha256"] = "7f570da8fe0ba5970cbf04d882b48e442e04d1df5ec180e12d9ff974450dfca2"
        names = ["python/python.exe", "python/python314.dll", "python/python314.zip", "python/python314._pth", "python/Lib/site-packages/vapoursynth/__init__.pyc", "python/Lib/site-packages/vapoursynth/vsscript.dll", "python/Lib/site-packages/vapoursynth/vspipe.exe", "python/Lib/site-packages/vapoursynth/libvapoursynth.dll", "python/Lib/site-packages/vapoursynth/plugins/LSMASHSource.dll"]
        self.receipt = {"schemaVersion": 1, "target": "x86_64-pc-windows-msvc", "tool": {"id": "av1an", "version": self.version, **record("av1an.exe")}, "sourceCommit": "805dad69143fa0a81cfe2fb89c0b9e90a828ea72", "source": source, "sharedMediaProvenanceSha256": support.digest(media), "sharedSources": shared, "additionalSources": [record("sources/decoder.tar.gz")], "licenses": [record("licenses/COPYING")], "buildInputs": [record("build/recipe.py")], "supportFiles": [record(name) for name in names]}
        self.write_receipt()

    def write_receipt(self):
        (self.directory / "build-provenance.json").write_text(json.dumps(self.receipt))

    def inventory(self, root):
        result = support.inventory(root)
        for record in result:
            if record["path"] == "sources/source.tar.gz":
                record["sha256"] = self.receipt["source"]["sha256"]
        return result

    def stage(self):
        return delivery.stage(self.directory, self.tools, "x86_64-pc-windows-msvc", self.inventory)

    def test_complete_runtime_and_shared_sources_are_staged(self):
        output = SimpleNamespace(stdout=self.version + "\nsystems.innocent.lsmas : Found\n", stderr="")
        with patch.object(delivery, "runtime_environment", return_value={}), patch.object(delivery.subprocess, "run", return_value=output):
            result = self.stage()
        self.assertEqual(len(result["supportFiles"]), 9)
        self.assertEqual(len(result["additionalSources"]), 8)
        self.assertTrue((self.tools / "av1an/python/python.exe").is_file())

    def test_changed_dependency_is_rejected_before_native_execution(self):
        (self.directory / "python/python314.dll").write_bytes(b"modified")
        with patch.object(delivery.subprocess, "run") as execute, self.assertRaisesRegex(ValueError, "inventory"):
            self.stage()
        execute.assert_not_called()

    def test_unlisted_plugin_is_rejected_before_native_execution(self):
        (self.directory / "python/Lib/site-packages/vapoursynth/plugins/unlisted.dll").write_bytes(b"injected")
        with patch.object(delivery.subprocess, "run") as execute, self.assertRaisesRegex(ValueError, "inventory"):
            self.stage()
        execute.assert_not_called()

    def test_missing_corresponding_library_source_is_rejected(self):
        self.receipt["sharedSources"].pop()
        self.write_receipt()
        with self.assertRaisesRegex(ValueError, "source closure"):
            self.stage()


if __name__ == "__main__":
    unittest.main()
