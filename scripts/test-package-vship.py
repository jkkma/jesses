"""Pinned Vship source and optional scorer merge regressions."""

import importlib.util
import io
import json
from pathlib import Path
import shutil
import sys
import tarfile
import tempfile
import unittest
from unittest import mock

sys.dont_write_bytecode = True
SCRIPTS = Path(__file__).resolve().parent


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


builder = load("test_vship_builder", "build-package-vship.py")
merger = load("test_vship_merger", "package-av1an-vship.py")


def record(root, name, data):
    path = root / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return {"path": name, "sha256": merger.support.digest(path)}


class VshipPackageTests(unittest.TestCase):
    def test_cached_input_must_match_hash_and_length(self):
        with tempfile.TemporaryDirectory() as temporary:
            cache = Path(temporary)
            contents = b"pinned Vship payload"
            item = record(cache, "vship.dll", contents)
            pin = {**item, "url": "https://example.invalid/vship.dll", "size": len(contents)}
            self.assertEqual(builder.checked_input(pin, cache), cache / "vship.dll")
            (cache / "vship.dll").write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "differs from its pin"):
                builder.checked_input(pin, cache)

    def test_pinned_source_must_retain_license(self):
        with tempfile.TemporaryDirectory() as temporary:
            archive = Path(temporary) / "source.tar.gz"
            with tarfile.open(archive, "w:gz") as source:
                payload = b"license text"
                member = tarfile.TarInfo("vship/LICENSE")
                member.size = len(payload)
                source.addfile(member, io.BytesIO(payload))
            self.assertEqual(builder.license_from_source(archive), b"license text")
            with tarfile.open(archive, "w:gz") as source:
                member = tarfile.TarInfo("vship/code.rs")
                member.size = 1
                source.addfile(member, io.BytesIO(b"x"))
            with self.assertRaisesRegex(ValueError, "lacks its license"):
                builder.license_from_source(archive)

    def test_merge_preserves_cpu_autoload_and_hashes_optional_plugin(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            base = root / "base"
            base.mkdir()
            tool = record(base, "av1an.exe", b"existing CPU av1an")
            source = record(base, "sources/av1an.tar.gz", b"upstream source")
            cpu = record(base, "python/Lib/site-packages/vapoursynth/plugins/vszip.dll", b"CPU")
            original = {
                "schemaVersion": 1,
                "target": "x86_64-pc-windows-msvc",
                "tool": {**tool, "id": "av1an", "version": "reviewed version"},
                "source": source,
                "additionalSources": [],
                "licenses": [],
                "buildInputs": [],
                "supportFiles": [cpu],
                "cpuScorers": [{"id": "vszip"}, {"id": "julek"}],
                "qtgmc": {"pythonModules": "python/qtgmc-deps.zip"},
            }
            (base / "build-provenance.json").write_text(json.dumps(original), encoding="utf-8")
            extension = root / "vship"
            extension.mkdir()
            plugin = record(extension, "plugins/libvship_VULKAN.dll", b"optional GPU")
            source = record(extension, "sources/vship.tar.gz", b"Vship source")
            license_record = record(extension, "licenses/Vship/LICENSE", b"Vship license")
            recipe = record(extension, "build/build-package-vship.py", b"reviewed recipe")
            (extension / "build-provenance.json").write_bytes(b"receipt")
            receipt = {
                "version": "5.1.1", "sourceRevision": "revision", "plugin": {"id": "com.lumen.vship", **plugin},
                "source": source, "licenses": [license_record], "buildInputs": [recipe],
            }
            destination = root / "merged"
            with (
                mock.patch.object(merger, "checked_vship", return_value=receipt),
                mock.patch.object(merger, "optional_imports", return_value=["kernel32.dll", "vulkan-1.dll"]),
                mock.patch.object(merger, "qualify_cpu"),
                mock.patch.object(merger, "qualify_optional_gpu", return_value=False),
            ):
                merger.merge(base, extension, root, destination)
            merged = destination / "delivery"
            self.assertTrue((merged / merger.OPTIONAL).is_file())
            self.assertFalse((merged / merger.FRAMESERVER / "plugins/libvship_VULKAN.dll").exists())
            receipt = merger.scorer.checked(
                merged, ("additionalSources", "licenses", "buildInputs", "supportFiles"),
                ("tool", "source"),
            )
            self.assertEqual(receipt["vship"]["runtimePath"], merger.OPTIONAL.as_posix())
            self.assertEqual(receipt["vship"]["sha256"], merger.support.digest(merged / merger.OPTIONAL))
            (merged / merger.OPTIONAL).write_bytes(b"tampered")
            with self.assertRaisesRegex(ValueError, "hash inventory"):
                merger.scorer.checked(
                    merged, ("additionalSources", "licenses", "buildInputs", "supportFiles"),
                    ("tool", "source"),
                )


if __name__ == "__main__":
    unittest.main()
