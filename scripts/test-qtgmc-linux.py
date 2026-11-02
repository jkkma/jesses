"""Integrity and resume guards for the native Linux QTGMC qualification recipe."""

import argparse
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("qtgmc_linux_recipe", Path(__file__).with_name("build-qtgmc-linux.py"))
recipe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(recipe)


class Recipe(unittest.TestCase):
    def test_complete_source_inputs_use_exact_inherited_and_gitlink_pins(self):
        sources = recipe.source_inputs()
        self.assertEqual(sources["vapoursynth"]["version"], "R79")
        self.assertEqual(sources["havsfunc"]["revision"], "5baf9c2a1dac9da4de182cf3a4e12041c3e0d48b")
        self.assertEqual(sources["znedi3-graphengine"]["commit"], "7bb77a5c062c610e7f44df6a59155cc3ab3e452a")
        self.assertEqual(sources["znedi3-vsxx"]["commit"], "11eb026af5517d69e1d3d6258d1b2c2040c508e7")
        for source in sources.values():
            self.assertTrue(source["url"].startswith("https://"))
            self.assertRegex(source["sha256"], r"^[a-f0-9]{64}$")
        self.assertEqual(len(recipe.PLUGINS), 9)

    def test_retained_build_paths_cannot_escape_or_follow_redirected_parents(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            owned = root / "owned"
            outside = root / "outside"
            owned.mkdir()
            outside.mkdir()
            (owned / "redirect").symlink_to(outside, target_is_directory=True)
            for value in ["../outside", str(outside), "redirect", "redirect/file"]:
                with self.assertRaises(ValueError):
                    recipe.safe_child(owned, value)
            self.assertEqual(recipe.safe_child(owned, "new/file"), owned / "new/file")

    def arguments(self, root, resume=False):
        media = root / "media"
        (media / "delivery").mkdir(parents=True, exist_ok=True)
        receipt = media / "delivery/build-provenance.json"
        if not receipt.exists():
            receipt.write_text("exact media receipt")
        libraries = media / "prefix/lib"
        libraries.mkdir(parents=True, exist_ok=True)
        for name in ("libdav1d.a", "libvpx.a", "libz.a"):
            if not (libraries / name).exists():
                (libraries / name).write_text("exact " + name)
        return argparse.Namespace(destination=root / "provider", cache=root / "cache", media_build=media, python=Path(sys.executable), jobs=2, resume=resume)

    def test_build_refuses_existing_destination_and_resume_refuses_tampered_payload(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            args = self.arguments(root)
            build = recipe.Build(args)
            payload = build.root / "proof.bin"
            payload.write_bytes(b"verified owned payload")
            build.state["stages"]["runtime"] = [{"path": "proof.bin", "sha256": recipe.digest(payload)}]
            build.save()
            with self.assertRaises(FileExistsError):
                recipe.Build(args)
            args.resume = True
            recipe.Build(args)
            payload.write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "payload differs"):
                recipe.Build(args)

    def test_resume_refuses_changed_media_and_source_lock_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            args = self.arguments(root)
            build = recipe.Build(args)
            args.resume = True
            (args.media_build / "delivery/build-provenance.json").write_text("changed")
            with self.assertRaisesRegex(ValueError, "inputs differ"):
                recipe.Build(args)
            (args.media_build / "delivery/build-provenance.json").write_text("exact media receipt")
            build.state["configuration"]["locks"]["qtgmc-linux-lock.json"] = "0" * 64
            build.save()
            with self.assertRaisesRegex(ValueError, "inputs differ"):
                recipe.Build(args)

    def test_resume_refuses_changed_linked_library_without_hiding_it_behind_media_receipt(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            args = self.arguments(root)
            recipe.Build(args)
            args.resume = True
            (args.media_build / "prefix/lib/libvpx.a").write_text("changed")
            with self.assertRaisesRegex(ValueError, "inputs differ"):
                recipe.Build(args)


if __name__ == "__main__":
    unittest.main()
