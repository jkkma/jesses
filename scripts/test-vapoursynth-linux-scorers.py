"""Input boundaries for the source-pinned Linux reader/scorer builder."""

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("linux_vs_builder", Path(__file__).with_name("build-vapoursynth-linux-scorers.py"))
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)


class PluginBuilderTests(unittest.TestCase):
    def fixture(self, root):
        plugins = root / "plugins"
        plugins.mkdir()
        records = []
        for name, filename in BUILD.PLUGIN_FILES.items():
            path = plugins / filename
            path.write_bytes(name.encode())
            records.append({"name": name, "path": str(path), "sha256": BUILD.digest(path)})
        return {"plugins": records}

    def test_inventory_checks_exact_bytes_and_all_three_identities(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            receipt = self.fixture(root)
            self.assertEqual(set(BUILD.plugin_inventory(root, receipt)), set(BUILD.PLUGIN_FILES))
            (root / "plugins/libjulek.so").write_bytes(b"different plugin")
            with self.assertRaisesRegex(ValueError, "differs"):
                BUILD.plugin_inventory(root, receipt)
            with self.assertRaises(ValueError):
                BUILD.plugin_inventory(root, {"plugins": receipt["plugins"][:2]})

    def test_redirected_and_foreign_plugin_paths_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            receipt = self.fixture(root)
            path = root / "plugins/libvszip.so"
            target = root / "other.so"
            path.rename(target)
            path.symlink_to(target)
            with self.assertRaisesRegex(ValueError, "redirected"):
                BUILD.plugin_inventory(root, receipt)
            receipt["plugins"][1]["path"] = str(target)
            with self.assertRaises(ValueError):
                BUILD.plugin_inventory(root, receipt)

    def test_changed_retained_source_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            receipt = self.fixture(root)
            source = root / "source.tar"
            source.write_bytes(b"pinned complete source")
            receipt["files"] = [{"path": "source.tar", "sha256": BUILD.digest(source)}]
            BUILD.plugin_inventory(root, receipt)
            source.write_bytes(b"changed source")
            with self.assertRaisesRegex(ValueError, "provenance"):
                BUILD.plugin_inventory(root, receipt)

    def test_existing_output_is_preserved_before_any_build(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            output = root / "existing"
            output.mkdir()
            keep = output / "keep"
            keep.write_bytes(b"unrelated")
            with self.assertRaises(FileExistsError):
                BUILD.Build(argparse.Namespace(destination=output))
            self.assertEqual(keep.read_bytes(), b"unrelated")

    def test_shared_codec_sources_must_match_exact_pins_once(self):
        lock = json.loads((BUILD.SCRIPTS / "package-ffmpeg-linux-lock.json").read_text())
        records = [{"path": "sources/" + row["filename"], "sha256": row["sha256"]} for row in lock["libraries"] if row["id"] in {"dav1d", "libvpx", "zlib"}]
        self.assertEqual(len(BUILD.linked_codec_sources({"additionalSources": records})), 3)
        for changed in (records[:2], records + [records[0]], [{**row, "sha256": "0" * 64} for row in records]):
            with self.assertRaises(ValueError):
                BUILD.linked_codec_sources({"additionalSources": changed})

    def test_runtime_uses_task_config_without_inherited_loader_hooks(self):
        with patch.dict(BUILD.os.environ, {"HOME": "/user/home", "PYTHONHOME": "injected", "LD_PRELOAD": "injected"}):
            env = BUILD.runtime_environment(Path("/tools/venv/bin/python"), Path("/tools/vapoursynth"), Path("/task/config"), Path("/task/plugins"))
        self.assertEqual(env["HOME"], "/user/home")
        self.assertEqual(env["XDG_CONFIG_HOME"], "/task/config")
        self.assertNotIn("PYTHONHOME", env)
        self.assertNotIn("LD_PRELOAD", env)
        self.assertTrue(env["PATH"].startswith("/tools/venv/bin:"))


if __name__ == "__main__":
    unittest.main()
