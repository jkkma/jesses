"""Source integrity and environment handoff checks for the CI SVT installer."""

import hashlib
import importlib.util
import io
import os
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location("ci_svt", Path(__file__).with_name("ci-install-svt.py"))
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)


class InstallerTests(unittest.TestCase):
    def archive(self, directory, extra=()):
        pin = BUILD.source_pin()
        archive = directory / "source.tar.gz"
        with tarfile.open(archive, "w:gz") as bundle:
            body = f"project(svt-av1 VERSION {pin['version']} LANGUAGES C CXX)\n".encode()
            item = tarfile.TarInfo(f"{pin['root']}/CMakeLists.txt")
            item.size = len(body)
            bundle.addfile(item, io.BytesIO(body))
            for item, body in extra:
                bundle.addfile(item, io.BytesIO(body) if body is not None else None)
        return archive, {**pin, "sha256": hashlib.sha256(archive.read_bytes()).hexdigest()}

    def test_verified_source_uses_shared_pin_and_never_overwrites_existing_work(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive, pin = self.archive(root)
            before = archive.read_bytes()
            source = BUILD.extract_verified(archive, root / "extracted", pin)
            self.assertIn(pin["version"], (source / "CMakeLists.txt").read_text())
            sentinel = source / "keep.txt"
            sentinel.write_text("existing build evidence")
            with self.assertRaises(FileExistsError):
                BUILD.extract_verified(archive, root / "extracted", pin)
            self.assertEqual(sentinel.read_text(), "existing build evidence")
            self.assertEqual(archive.read_bytes(), before)
            self.assertEqual(pin["commit"], BUILD.source_pin()["commit"])

    def test_tampered_archive_is_rejected_before_extraction(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive, pin = self.archive(root)
            archive.write_bytes(archive.read_bytes() + b"changed")
            with self.assertRaisesRegex(ValueError, "checksum"):
                BUILD.extract_verified(archive, root / "extracted", pin)
            self.assertFalse((root / "extracted").exists())

    def test_traversal_links_duplicates_and_wrong_root_are_rejected_before_writes(self):
        pin = BUILD.source_pin()
        cases = []
        for name in [f"{pin['root']}/../outside", "/absolute", "wrong/file", f"{pin['root']}/CMakeLists.txt", f"{pin['root']}/a\\b"]:
            item = tarfile.TarInfo(name)
            item.size = 1
            cases.append((item, b"x"))
        for kind in [tarfile.SYMTYPE, tarfile.LNKTYPE, tarfile.FIFOTYPE]:
            item = tarfile.TarInfo(f"{pin['root']}/redirected")
            item.type = kind
            item.linkname = "../../outside"
            cases.append((item, None))
        for extra in cases:
            with self.subTest(name=extra[0].name, kind=extra[0].type), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                archive, pin = self.archive(root, [extra])
                with self.assertRaisesRegex(ValueError, "archive member"):
                    BUILD.extract_verified(archive, root / "extracted", pin)
                self.assertFalse((root / "extracted").exists())

    def test_member_and_expansion_limits_are_enforced(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive, pin = self.archive(root)
            for name in ("MAX_MEMBERS", "MAX_EXPANDED"):
                with self.subTest(bound=name), patch.object(BUILD, name, 0):
                    with self.assertRaisesRegex(ValueError, "archive member"):
                        BUILD.extract_verified(archive, root / "extracted", pin)
            self.assertFalse((root / "extracted").exists())

    def test_source_and_executable_versions_must_match_mainline_pin(self):
        pin = BUILD.source_pin()
        for version in [f"SVT-AV1 Encoder Lib v{pin['version']}\n", f"SVT-AV1 v{pin['version']}\n"]:
            BUILD.validate_version(version, pin)
        for version in ["SVT-AV1 Encoder Lib v1.7.0", f"SVT-AV1 Encoder Lib v{pin['version']}0", f"SVT-AV1 Encoder Lib v{pin['version']} [5fish]", f"SVT-AV1-HDR v{pin['version']}"]:
            with self.subTest(version=version), self.assertRaisesRegex(ValueError, "unexpected version"):
                BUILD.validate_version(version, pin)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive, pin = self.archive(root)
            with self.assertRaisesRegex(ValueError, "project version"):
                BUILD.extract_verified(archive, root / "extracted", {**pin, "version": "0.0.0"})
            self.assertFalse((root / "extracted").exists())

    def test_host_environment_is_excluded_and_only_github_handoff_files_change(self):
        hostile = {name: "unrelated-host-input" for name in ["BASH_ENV", "ENV", "CC", "CFLAGS", "LDFLAGS", "CMAKE_TOOLCHAIN_FILE", "CMAKE_PROJECT_INCLUDE", "CMAKE_GENERATOR", "LD_LIBRARY_PATH", "LD_PRELOAD", "MAKEFLAGS", "PYTHONPATH"]}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            executable = root / "bin" / "SvtAv1EncApp"
            executable.parent.mkdir()
            executable.write_bytes(b"verified-tool-fixture")
            handoff = {"GITHUB_ENV": str(root / "github-env"), "GITHUB_PATH": str(root / "github-path")}
            with patch.dict(os.environ, {**hostile, **handoff}):
                before = dict(os.environ)
                result = BUILD.environment(root / "empty-home")
                BUILD.publish_environment(executable)
                self.assertEqual(dict(os.environ), before)
            self.assertFalse(any(value == "unrelated-host-input" for value in result.values()))
            self.assertEqual(result["PATH"], "/usr/bin:/bin")
            self.assertEqual((root / "github-env").read_text(), f"JESSES_SVT_AV1={executable.resolve()}\n")
            self.assertEqual((root / "github-path").read_text(), f"{executable.parent.resolve()}\n")


if __name__ == "__main__":
    unittest.main()
