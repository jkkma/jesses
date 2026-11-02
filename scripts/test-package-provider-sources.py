"""Shared package sources coexist with upstream Vulkan source archives."""

import importlib.util
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
SCRIPTS = Path(__file__).resolve().parent


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


extra = load("mixed_source_extra", "build-package-extra-encoders.py")
av1an = load("mixed_source_av1an", "package-av1an-delivery.py")


class SharedSourceTests(unittest.TestCase):
    def test_package_selection_skips_upstream_source_records(self):
        package = {"base": "mingw-w64-libvpx", "path": "sources/libvpx.tar.zst", "sha256": "pinned"}
        provenance = {"additionalSources": [{"path": "sources/libplacebo.tar.gz", "sha256": "upstream"}, package]}
        self.assertEqual(extra.source_record(provenance, "mingw-w64-libvpx"), package)
        with self.assertRaisesRegex(ValueError, "Missing pinned source package"):
            extra.source_record(provenance, "mingw-w64-x265")

    def test_extra_runtime_selection_retains_complete_package_closure(self):
        packages = [{"base": "mingw-w64-" + name, "path": "sources/" + name, "sha256": name}
                    for name in ("gcc", "crt", "headers", "winpthreads")]
        with tempfile.TemporaryDirectory() as temporary:
            media = Path(temporary)
            recipe = media / "recipe.py"
            recipe.write_bytes(b"retained recipe")
            provenance = {"schemaVersion": 1, "target": "x86_64-pc-windows-msvc",
                          "additionalSources": [{"path": "sources/glslang.tar.gz", "sha256": "upstream"}, *packages],
                          "buildInputs": [{"path": recipe.name, "sha256": extra.digest(recipe)}]}
            receipt = media / "build-provenance.json"
            receipt.write_text(json.dumps(provenance))
            helper = SimpleNamespace(verify_compiler=lambda path: None)
            with patch.object(extra, "module", return_value=helper):
                self.assertEqual(extra.pinned_media(media, media)[2], packages)
                provenance["additionalSources"].pop()
                receipt.write_text(json.dumps(provenance))
                with self.assertRaisesRegex(ValueError, "runtime sources are incomplete"):
                    extra.pinned_media(media, media)

    def test_av1an_shared_sources_require_every_package_among_upstream_sources(self):
        packages = [{"base": base, "path": "sources/" + base, "sha256": base}
                    for base in sorted(av1an.SHARED_BASES)]
        provenance = {"additionalSources": [{"path": "sources/Vulkan-Headers.tar.gz", "sha256": "upstream"}, *packages]}
        self.assertEqual(av1an.shared_media_sources(provenance), packages)
        provenance["additionalSources"].pop()
        with self.assertRaisesRegex(ValueError, "shared static library sources are incomplete"):
            av1an.shared_media_sources(provenance)


if __name__ == "__main__":
    unittest.main()
