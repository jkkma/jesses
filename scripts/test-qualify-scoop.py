"""Focused tests for the isolated Scoop qualification safety helpers."""

import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
import zipfile


sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("qualify_scoop", Path(__file__).with_name("qualify-scoop.py"))
qualify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qualify)


class QualificationSafetyTests(unittest.TestCase):
    def write_archive(self, archive_path: Path, extra: dict[str, bytes] | None = None) -> None:
        files = {"jesses.exe": b"MZ fixture", "jesses.portable": b"1\n", **(extra or {})}
        package = {
            "schemaVersion": 1,
            "product": "jesses",
            "version": "0.1.0",
            "target": "x86_64-pc-windows-msvc",
            "profile": "release",
            "storage": "portable",
            "files": [
                {"path": name, "size": len(value), "sha256": qualify.hashlib.sha256(value).hexdigest()}
                for name, value in files.items()
            ],
        }
        with zipfile.ZipFile(archive_path, "w") as archive:
            for name, value in files.items():
                archive.writestr(name, value)
            archive.writestr("package-manifest.json", json.dumps(package))

    def test_require_under_rejects_sibling_and_accepts_child(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "root"
            self.assertEqual(qualify.require_under(root / "child", root), (root / "child").absolute())
            with self.assertRaises(qualify.QualificationError):
                qualify.require_under(root.parent / "other", root)

    def test_archive_requires_bound_manifest_and_no_data_directory(self):
        with tempfile.TemporaryDirectory() as temporary:
            archive_path = Path(temporary) / "candidate.zip"
            self.write_archive(archive_path)
            qualify.validate_archive(archive_path)

            data_archive = Path(temporary) / "data.zip"
            self.write_archive(data_archive, {"JESSES-DATA/history.json": b"{}"})
            with self.assertRaises(qualify.QualificationError):
                qualify.validate_archive(data_archive)

    def test_scoop_adapter_hash_is_rechecked_between_phases(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "scoop"
            qualify.configure_root(root)
            system_script = root / "apps/scoop/current/lib/system.ps1"
            system_script.parent.mkdir(parents=True)
            system_script.write_text("original", encoding="utf-8")
            state = {"isolated_system_sha256": qualify.digest(system_script)}
            qualify.assert_scoop_adapter_unchanged(state)
            system_script.write_text("changed", encoding="utf-8")
            with self.assertRaises(qualify.QualificationError):
                qualify.assert_scoop_adapter_unchanged(state)

    def test_uninstall_rejects_leftover_shim_metadata(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "scoop"
            qualify.configure_root(root)
            shim = root / "shims/jesses.shim"
            shim.parent.mkdir(parents=True)
            shim.write_text("path = fixture", encoding="utf-8")
            with self.assertRaises(qualify.QualificationError):
                qualify.assert_uninstalled()
            shim.unlink()
            qualify.assert_uninstalled()

    def test_tree_summary_detects_content_change_without_exposing_names(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary)
            secret_name = "private-job-name.json"
            (path / secret_name).write_text("first", encoding="utf-8")
            before = qualify.tree_summary(path)
            self.assertNotIn(secret_name, json.dumps(before))
            (path / secret_name).write_text("second", encoding="utf-8")
            self.assertNotEqual(before, qualify.tree_summary(path))


if __name__ == "__main__":
    unittest.main()
