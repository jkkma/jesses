"""Packaging transaction/resource invariants; no native app or media is launched."""

import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
import warnings
import zipfile

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location("desktop_package", Path(__file__).with_name("package-desktop.py"))
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)

verifier_spec = importlib.util.spec_from_file_location(
    "bundle_resource_verifier", Path(__file__).with_name("verify-bundle-resources.py")
)
verifier = importlib.util.module_from_spec(verifier_spec)
verifier_spec.loader.exec_module(verifier)


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="jesses-package-check-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.executable = self.root / "input.exe"
        self.executable.write_bytes(b"MZ synthetic package fixture\n")
        self.destination = self.root / "portable"
        with patch.object(package, "source_revision", return_value={"revisionAtPackaging": "synthetic"}):
            package.assemble(self.executable, self.destination, "x86_64-pc-windows-msvc", "debug", True)

    def test_staging_retains_source_and_archive_retains_every_verified_resource(self):
        self.assertEqual(self.executable.read_bytes(), b"MZ synthetic package fixture\n")
        self.assertFalse((self.destination / "jesses-data").exists())
        archive = self.root / "output.zip"
        package.archive(self.destination, archive)
        with zipfile.ZipFile(archive) as staged:
            self.assertEqual(staged.read("jesses.portable"), b"1\n")
            manifest = json.loads(staged.read(package.MANIFEST))
            self.assertEqual(set(staged.namelist()), {entry["path"] for entry in manifest["files"]} | {package.MANIFEST})
            self.assertEqual(manifest["tools"], "external")
        with self.assertRaises(FileExistsError):
            package.archive(self.destination, archive)

    def test_existing_destination_is_preserved(self):
        original = (self.destination / package.MANIFEST).read_bytes()
        with self.assertRaises(FileExistsError):
            package.assemble(self.executable, self.destination, "x86_64-pc-windows-msvc", "debug", True)
        self.assertEqual((self.destination / package.MANIFEST).read_bytes(), original)

    def test_scoop_manifest_binds_portable_archive_and_persists_one_data_directory(self):
        archive = self.root / "portable.zip"
        package.archive(self.destination, archive)
        destination = self.root / "jesses.json"
        package.scoop_manifest(archive, "https://example.test/releases/portable.zip", destination)
        manifest = json.loads(destination.read_text())
        self.assertEqual(manifest["architecture"]["64bit"]["hash"], package.digest(archive))
        self.assertEqual(manifest["persist"], "jesses-data")
        self.assertEqual(manifest["bin"], "jesses.exe")
        self.assertNotIn("installer", manifest)
        self.assertNotIn("uninstaller", manifest)
        with self.assertRaises(ValueError):
            package.scoop_manifest(archive, "http://example.test/portable.zip", self.root / "invalid.json")
        with self.assertRaises(FileExistsError):
            package.scoop_manifest(archive, "https://example.test/releases/portable.zip", destination)

    def test_scoop_manifest_rejects_unverified_archive_contents(self):
        valid = self.root / "valid-portable.zip"
        package.archive(self.destination, valid)

        def rewrite(name, transform):
            candidate = self.root / name
            with zipfile.ZipFile(valid) as source, zipfile.ZipFile(candidate, "w") as output:
                for member in source.infolist():
                    output.writestr(member, transform(member.filename, source.read(member)))
            return candidate

        invalid = [
            rewrite(
                "tampered-payload.zip",
                lambda name, content: content + b"tampered" if name == "jesses.exe" else content,
            ),
            rewrite("extra-inventory.zip", lambda _name, content: content),
        ]
        with zipfile.ZipFile(invalid[-1], "a") as output:
            output.writestr("unexpected.bin", b"unexpected")

        user_data = self.root / "case-variant-user-data.zip"
        payload = b"must not ship"
        with zipfile.ZipFile(valid) as source, zipfile.ZipFile(user_data, "w") as output:
            manifest = json.loads(source.read(package.MANIFEST))
            manifest["files"].append(
                {
                    "path": "JESSES-DATA/private.json",
                    "size": len(payload),
                    "sha256": package.hashlib.sha256(payload).hexdigest(),
                }
            )
            for member in source.infolist():
                content = source.read(member)
                if member.filename == package.MANIFEST:
                    content = json.dumps(manifest).encode("utf-8")
                output.writestr(member, content)
            output.writestr("JESSES-DATA/private.json", payload)
        invalid.append(user_data)

        for index, entries in enumerate(
            [
                [("../escaped.exe", b"unsafe")],
                [("same.exe", b"first"), ("same.exe", b"second")],
                [("jesses.exe", b"first"), ("JESSES.exe", b"second")],
            ]
        ):
            candidate = self.root / f"invalid-names-{index}.zip"
            with warnings.catch_warnings():
                warnings.simplefilter("ignore", UserWarning)
                with zipfile.ZipFile(candidate, "w") as output:
                    for name, content in entries:
                        output.writestr(name, content)
            invalid.append(candidate)

        for index, candidate in enumerate(invalid):
            destination = self.root / f"rejected-{index}.json"
            with self.subTest(candidate=candidate.name), self.assertRaises(ValueError):
                package.scoop_manifest(
                    candidate,
                    "https://example.test/releases/portable.zip",
                    destination,
                )
            self.assertFalse(destination.exists())

    def test_windows_collection_requires_only_executable_and_ignores_stale_nsis(self):
        build = self.root / "build"
        build.mkdir()
        (build / "jesses.exe").write_bytes(self.executable.read_bytes())
        stale = build / "bundle/nsis"
        stale.mkdir(parents=True)
        (stale / "old-setup.exe").write_bytes(b"stale installer")
        output = self.root / "collected"
        with patch.object(package, "source_revision", return_value={}):
            package.collect(build, output, "x86_64-pc-windows-msvc", "release")
        manifest = json.loads((output / "artifact-manifest.json").read_text())
        self.assertFalse(any(item["path"].endswith(".exe") for item in manifest["artifacts"]))
        archives = list(output.glob("*.zip"))
        self.assertEqual(len(archives), 1)
        extracted = self.root / "extracted-zip"
        extracted.mkdir()
        verifier.extract_portable(archives[0], extracted)
        self.assertEqual(package.verify(extracted)["storage"], "portable")
        self.assertEqual((extracted / "jesses.exe").read_bytes(), self.executable.read_bytes())

    def test_zip_extraction_rejects_traversal_duplicates_and_redirected_entries(self):
        for index, names in enumerate([["../escaped.exe"], ["same.exe", "same.exe"], ["link.exe"]]):
            archive = self.root / f"invalid-{index}.zip"
            with zipfile.ZipFile(archive, "w") as bundle:
                for name in names:
                    entry = zipfile.ZipInfo(name)
                    if name == "link.exe":
                        entry.external_attr = 0o120777 << 16
                    bundle.writestr(entry, b"invalid")
            destination = self.root / f"invalid-{index}"
            destination.mkdir()
            with self.assertRaises((ValueError, FileExistsError)):
                verifier.extract_portable(archive, destination)
        self.assertFalse((self.root / "escaped.exe").exists())

    def test_modified_missing_and_extra_resources_are_detected(self):
        resource = self.destination / "resources/runtime-contract.json"
        original = resource.read_bytes()
        resource.write_bytes(original + b" ")
        with self.assertRaises(ValueError):
            package.verify(self.destination)
        resource.unlink()
        with self.assertRaises(ValueError):
            package.verify(self.destination)
        resource.write_bytes(original)
        (self.destination / "resources/package-manifest.json").write_text("unexpected", encoding="utf-8")
        with self.assertRaises(ValueError):
            package.verify(self.destination)

    def test_unsafe_archive_names_are_rejected(self):
        for name in ["../escape", "/absolute", "C:/absolute", "resource\\escape", "./resource", ""]:
            with self.subTest(name=name), self.assertRaises(ValueError):
                package.relative_path(name)

    def test_user_data_is_rejected_even_if_added_to_inventory(self):
        (self.destination / "jesses-data").mkdir()
        manifest_path = self.destination / package.MANIFEST
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["files"] = package.inventory(self.destination)
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaises(ValueError):
            package.verify(self.destination)

    def test_invalid_portable_marker_is_rejected_even_with_matching_hash(self):
        (self.destination / "jesses.portable").write_bytes(b"2\n")
        manifest_path = self.destination / package.MANIFEST
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["files"] = package.inventory(self.destination)
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaises(ValueError):
            package.verify(self.destination)

    def test_installed_package_does_not_enable_portable_mode(self):
        installed = self.root / "installed"
        with patch.object(package, "source_revision", return_value={}):
            package.assemble(self.executable, installed, "x86_64-pc-windows-msvc", "debug", False)
        self.assertFalse((installed / "jesses.portable").exists())
        self.assertEqual(package.verify(installed)["storage"], "installed")

    def test_relative_and_empty_path_entries_cannot_supply_a_tool(self):
        self.assertIsNone(package.find_on_path("ffmpeg", "."))
        self.assertIsNone(package.find_on_path("ffmpeg", ""))
        result = package.diagnostics(self.destination, True)
        self.assertTrue(result["packageVerified"])
        self.assertTrue(result["pathSanitized"])
        self.assertEqual(result["tools"], "external")

    def test_bundled_executable_source_and_license_are_bound_to_the_manifest(self):
        tools = self.root / "tools"
        tools.mkdir()
        records = []
        for name, payload in [("encoder.exe", b"binary"), ("source.tar.gz", b"source"), ("LICENSE", b"notice"), ("library-source.tar.gz", b"library source"), ("codec.dll", b"runtime library"), ("build.py", b"build recipe")]:
            (tools / name).write_bytes(payload)
            records.append({"path": name, "sha256": package.digest(tools / name)})
        (tools / "manifest.json").write_text(json.dumps({
            "schemaVersion": 1, "target": "x86_64-pc-windows-msvc",
            "tools": [{"id": "svt-av1-hdr", **records[0], "source": records[1], "licenses": [records[2]], "additionalSources": [records[3]], "supportFiles": [records[4]], "buildInputs": [records[5]]}],
        }), encoding="utf-8")
        bundled = self.root / "bundled"
        with patch.object(package, "source_revision", return_value={}):
            package.assemble(self.executable, bundled, "x86_64-pc-windows-msvc", "debug", True, tools)
        self.assertEqual(package.verify(bundled)["tools"], "bundledWithExternalDependencies")
        (bundled / "resources/tools/source.tar.gz").write_bytes(b"different source")
        manifest_path = bundled / package.MANIFEST
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["files"] = package.inventory(bundled)
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaises(ValueError):
            package.verify(bundled)
        (bundled / "resources/tools/source.tar.gz").write_bytes(b"source")
        for name in ("library-source.tar.gz", "codec.dll", "build.py"):
            with self.subTest(name=name):
                path = bundled / "resources/tools" / name
                original = path.read_bytes()
                path.write_bytes(b"modified")
                manifest["files"] = package.inventory(bundled)
                manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
                with self.assertRaises(ValueError):
                    package.verify(bundled)
                path.write_bytes(original)

    def test_same_stem_appimage_and_deb_use_distinct_extraction_directories(self):
        packages = self.root / "native-packages"
        packages.mkdir()
        artifacts = []
        for extension in (".AppImage", ".deb"):
            artifact = packages / f"jesses_0.1.0_amd64{extension}"
            artifact.write_bytes(f"synthetic {extension} fixture".encode())
            artifacts.append({"path": artifact.name, "sha256": package.digest(artifact)})
        (packages / "artifact-manifest.json").write_text(json.dumps({
            "schemaVersion": 1,
            "product": "jesses",
            "target": "x86_64-unknown-linux-gnu",
            "artifacts": artifacts,
        }), encoding="utf-8")
        destination = self.root / "extracted-native-packages"

        def populate_resources(root):
            tools = root / "resources/tools"
            tools.mkdir(parents=True)
            (tools / "manifest.json").write_text(json.dumps({"schemaVersion": 1, "tools": []}), encoding="utf-8")
            for name in ("runtime-contract.json", "LICENSE", "THIRD_PARTY_NOTICES.md"):
                (root / "resources" / name).write_text("fixture", encoding="utf-8")
            notice = root / "resources/licenses/shadcn-svelte-MIT.txt"
            notice.parent.mkdir()
            notice.write_text("fixture", encoding="utf-8")

        def run(command, **options):
            if "--appimage-extract" in command:
                populate_resources(Path(options["cwd"]) / "squashfs-root/usr/lib/jesses")
            elif command[0] == "dpkg-deb":
                populate_resources(Path(command[-1]) / "usr/lib/jesses")

        arguments = [
            "verify-bundle-resources.py",
            "--packages", str(packages),
            "--probe", str(self.executable),
            "--destination", str(destination),
        ]
        with patch.object(verifier.subprocess, "run", side_effect=run), patch.object(sys, "argv", arguments), patch("builtins.print"):
            verifier.main()

        self.assertTrue((destination / "jesses_0.1.0_amd64.AppImage").is_dir())
        self.assertTrue((destination / "jesses_0.1.0_amd64.deb").is_dir())
        report = json.loads((packages / "bundle-resource-qualification.json").read_text(encoding="utf-8"))
        self.assertEqual([entry["artifact"] for entry in report["artifacts"]], [
            "jesses_0.1.0_amd64.AppImage",
            "jesses_0.1.0_amd64.deb",
        ])


if __name__ == "__main__":
    unittest.main()
