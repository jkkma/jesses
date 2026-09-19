"""Exact AppImage tool restoration, path-safety and inventory regressions."""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("restore_appimage_tools", Path(__file__).with_name("restore-appimage-tools.py"))
restore = importlib.util.module_from_spec(spec)
spec.loader.exec_module(restore)


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class AppImageRestoreTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="jesses-appimage-restore-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.tools = self.root / "verified-tools"
        encoder = self.tools / "svt-av1-5fish/SvtAv1EncApp"
        encoder.parent.mkdir(parents=True)
        encoder.write_bytes(b"exact reviewed ELF fixture")
        encoder.chmod(0o755)
        for name, payload in [("source.tar.gz", b"source"), ("LICENSE.md", b"license"), ("build.py", b"recipe")]:
            (encoder.parent / name).write_bytes(payload)
        records = {name: {"path": f"svt-av1-5fish/{name}", "sha256": digest(encoder.parent / name)}
                   for name in ("SvtAv1EncApp", "source.tar.gz", "LICENSE.md", "build.py")}
        manifest = {"schemaVersion": 1, "target": "x86_64-unknown-linux-gnu", "tools": [{
            "id": "svt-av1-5fish", **records["SvtAv1EncApp"], "source": records["source.tar.gz"],
            "licenses": [records["LICENSE.md"]], "buildInputs": [records["build.py"]],
        }]}
        (self.tools / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")

        self.build = self.root / "build"
        self.app_dir = self.build / "bundle/appimage/jesses.AppDir"
        resources = self.app_dir / "usr/lib/jesses"
        destination = resources / "resources/tools"
        destination.parent.mkdir(parents=True)
        restore.shutil.copytree(self.tools, destination, symlinks=True, copy_function=restore.shutil.copy2)
        (destination / "svt-av1-5fish/SvtAv1EncApp").write_bytes(b"linuxdeploy changed this")
        for name in ("runtime-contract.json", "LICENSE", "THIRD_PARTY_NOTICES.md"):
            (resources / "resources" / name).write_text("fixture", encoding="utf-8")
        notice = resources / "resources/licenses/shadcn-svelte-MIT.txt"
        notice.parent.mkdir()
        notice.write_text("fixture", encoding="utf-8")
        self.appimage = self.build / "bundle/appimage/jesses_0.1.0_amd64.AppImage"
        self.appimage.write_bytes(b"pre-repair appimage")
        self.appimage.chmod(0o755)
        self.plugin = self.root / "linuxdeploy-plugin-appimage-x86_64.AppImage"
        self.plugin.write_bytes(b"plugin fixture")
        self.plugin.chmod(0o755)

    def test_restores_exact_tree_runs_full_verifier_and_retains_original_image(self):
        work = self.root / "repair"

        def repack(plugin, app_dir, output, repair):
            self.assertEqual(restore.tree_inventory(app_dir / "usr/lib/jesses/resources/tools"),
                             restore.tree_inventory(self.tools))
            output.write_bytes(b"repacked exact appimage")
            output.chmod(0o755)
            return {
                "pluginSource": restore.OUTPUT_PLUGIN_URL,
                "tauriBundlerSource": restore.TAURI_BUNDLER_SOURCE_URL,
                "pluginSha256": "1" * 64,
                "appimagetoolWrapperSha256": "2" * 64,
                "appimagetoolAppRunSha256": "3" * 64,
                "appimagetoolRuntimeSha256": "4" * 64,
                "mksquashfsSha256": "5" * 64,
            }

        with patch.object(restore.verifier, "verify_resources", wraps=restore.verifier.verify_resources) as verify:
            receipt = restore.restore(self.build, self.tools, self.plugin, work, repack=repack)
        verify.assert_called_once()
        self.assertEqual(self.appimage.read_bytes(), b"repacked exact appimage")
        self.assertEqual((work / "original" / self.appimage.name).read_bytes(), b"pre-repair appimage")
        self.assertEqual(receipt["originalAppImage"]["sha256"], hashlib.sha256(b"pre-repair appimage").hexdigest())
        self.assertEqual(receipt["rebuiltAppImage"]["sha256"], hashlib.sha256(b"repacked exact appimage").hexdigest())
        self.assertEqual(receipt["tools"]["ids"], ["svt-av1-5fish"])
        self.assertEqual(receipt["repackTool"]["pluginSource"], restore.OUTPUT_PLUGIN_URL)
        self.assertEqual(json.loads((work / "restoration.json").read_text(encoding="utf-8")), receipt)

    def test_unrecorded_source_payload_is_rejected_before_the_appdir_changes(self):
        (self.tools / "unexpected").write_bytes(b"unrecorded")
        original = self.appimage.read_bytes()
        with self.assertRaisesRegex(ValueError, "missing or unrecorded"):
            restore.restore(self.build, self.tools, self.plugin, self.root / "repair")
        self.assertEqual(self.appimage.read_bytes(), original)

    def test_symlinked_appdir_manifest_is_rejected_before_repack(self):
        manifest = self.app_dir / "usr/lib/jesses/resources/tools/manifest.json"
        external = self.root / "external-manifest.json"
        external.write_bytes(manifest.read_bytes())
        manifest.unlink()
        try:
            os.symlink(external, manifest)
        except OSError as error:
            self.skipTest(f"Symlinks are unavailable: {error}")
        original = self.appimage.read_bytes()
        with self.assertRaisesRegex(ValueError, "must be a regular file"):
            restore.restore(self.build, self.tools, self.plugin, self.root / "repair")
        self.assertEqual(self.appimage.read_bytes(), original)

    def test_symlinked_bundle_ancestor_cannot_redirect_outside_the_build(self):
        redirected_build = self.root / "redirected-build"
        redirected_build.mkdir()
        external_bundle = self.root / "external-bundle"
        restore.shutil.copytree(self.build / "bundle", external_bundle)
        try:
            os.symlink(external_bundle, redirected_build / "bundle", target_is_directory=True)
        except OSError as error:
            self.skipTest(f"Directory symlinks are unavailable: {error}")
        original = (external_bundle / "appimage" / self.appimage.name).read_bytes()
        with self.assertRaisesRegex(ValueError, "AppImage build directory is outside"):
            restore.restore(redirected_build, self.tools, self.plugin, self.root / "repair")
        self.assertEqual((external_bundle / "appimage" / self.appimage.name).read_bytes(), original)

    def test_exact_tree_copy_preserves_safe_links_and_modes(self):
        link = self.tools / "svt-av1-5fish/SvtAv1EncApp.link"
        try:
            os.symlink("SvtAv1EncApp", link)
        except OSError as error:
            self.skipTest(f"Symlinks are unavailable: {error}")
        copied = self.root / "copied-tools"
        restored_inventory = restore.copy_exact_tree(self.tools, copied)
        restored_link = copied / "svt-av1-5fish/SvtAv1EncApp.link"
        self.assertTrue(restored_link.is_symlink())
        self.assertEqual(os.readlink(restored_link), "SvtAv1EncApp")
        self.assertEqual(restored_inventory, restore.tree_inventory(self.tools))
        self.assertEqual(
            stat_mode := (copied / "svt-av1-5fish/SvtAv1EncApp").stat().st_mode & 0o777,
            (self.tools / "svt-av1-5fish/SvtAv1EncApp").stat().st_mode & 0o777,
        )
        self.assertTrue(stat_mode)

    def test_repack_uses_the_output_plugins_bundled_runtime_and_mksquashfs(self):
        work = self.root / "repack-work"
        work.mkdir()
        output = self.root / "rebuilt.AppImage"
        calls = []

        def run(arguments, **options):
            calls.append((arguments, options))
            if len(calls) == 1:
                plugin_root = Path(options["cwd"]) / "squashfs-root"
                wrapper = plugin_root / "usr/bin/appimagetool"
                app_run = plugin_root / "appimagetool-prefix/AppRun"
                runtime = plugin_root / "appimagetool-prefix/usr/bin/appimagetool"
                mksquashfs = plugin_root / "appimagetool-prefix/usr/bin/mksquashfs"
                for path in (wrapper, app_run, runtime, mksquashfs):
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text("fixture", encoding="utf-8")
                    path.chmod(0o755)
                wrapper.write_text("exec ../../appimagetool-prefix/AppRun", encoding="utf-8")
                app_run.write_text('export PATH="$this_dir"/usr/bin:"$PATH"', encoding="utf-8")
            else:
                Path(arguments[-1]).write_bytes(b"rebuilt by bundled appimagetool")
                Path(arguments[-1]).chmod(0o755)

        with patch.object(restore.subprocess, "run", side_effect=run):
            receipt = restore.repack_appimage(self.plugin, self.app_dir, output, work)

        self.assertEqual(calls[0][0], [str(self.plugin.resolve()), "--appimage-extract"])
        wrapper = work / "output-plugin/squashfs-root/usr/bin/appimagetool"
        self.assertEqual(calls[1][0], [str(wrapper.resolve()), str(self.app_dir), str(output)])
        self.assertEqual(calls[1][1]["env"]["ARCH"], "x86_64")
        self.assertEqual(receipt["appimagetoolWrapperSha256"], digest(wrapper))
        self.assertEqual(receipt["mksquashfsSha256"], digest(
            work / "output-plugin/squashfs-root/appimagetool-prefix/usr/bin/mksquashfs"
        ))


if __name__ == "__main__":
    unittest.main()
