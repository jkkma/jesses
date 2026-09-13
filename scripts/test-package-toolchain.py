"""Regression gates for compiler isolation and pinned payload boundaries."""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


builder = load("ffmpeg_builder", "build-package-ffmpeg.py")
bootstrap = load("bootstrap", "bootstrap-package-toolchain.py")
probe = load("probe", "qualify-package-tools.py")
vmaf = load("vmaf", "build-package-vmaf-windows.py")


class PackageCompilerTests(unittest.TestCase):
    def test_vmaf_static_c_link_resolves_its_cpp_parser_runtime(self):
        with tempfile.TemporaryDirectory(prefix="jesses-vmaf-metadata-test-") as directory:
            metadata = Path(directory) / "libvmaf.pc"
            for private in ("", "Libs.private: -pthread\n", "Libs.private: -lstdc++\n"):
                with self.subTest(private=private):
                    metadata.write_text("Libs: -lvmaf\n" + private, encoding="utf-8")
                    vmaf.complete_static_metadata(metadata)
                    vmaf.complete_static_metadata(metadata)
                    value = metadata.read_text(encoding="utf-8")
                    self.assertEqual(value.count("-lstdc++"), 1)
                    self.assertEqual(value.count("Libs.private:"), 1)
                    self.assertIn("Libs: -lvmaf\n", value)

    @unittest.skipUnless(sys.platform == "win32", "Windows canonical path prefix")
    def test_windows_native_canonical_paths_match_the_package_root(self):
        with tempfile.TemporaryDirectory(prefix="jesses-probe-path-test-") as directory:
            root = Path(directory).resolve()
            executable = root / "tool.exe"
            executable.write_bytes(b"tool")
            extended = Path("\\\\?\\" + str(executable))
            self.assertTrue(probe.comparable_path(extended).is_relative_to(probe.comparable_path(root)))

    def test_host_compiler_paths_and_shell_hooks_cannot_enter_the_build(self):
        names = ["C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH", "CPATH", "LIBRARY_PATH", "CC", "CXX", "LD", "BASH_ENV", "ENV", "MAKEFLAGS", "PKG_CONFIG", "LD_PRELOAD"]
        with patch.dict(os.environ, {name: "unrelated-host-input" for name in names}):
            result = builder.build_environment("controlled-pkgconfig")
        for name in names:
            self.assertNotIn(name, result)
        self.assertEqual(result["PKG_CONFIG_PATH"], "controlled-pkgconfig")

    def test_changed_compiler_payload_is_rejected_before_compilation(self):
        with tempfile.TemporaryDirectory(prefix="jesses-compiler-test-") as directory:
            root = Path(directory).resolve()
            payload = root / "compiler.exe"
            payload.write_bytes(b"pinned compiler")
            receipt = {"schemaVersion": 1, "lockSha256": builder.digest(builder.LOCK), "files": {"compiler.exe": hashlib.sha256(payload.read_bytes()).hexdigest()}}
            (root / "jesses-toolchain.json").write_text(json.dumps(receipt), encoding="utf-8")
            builder.verify_compiler(root)
            payload.write_bytes(b"changed compiler")
            with self.assertRaisesRegex(ValueError, "Changed compiler"):
                builder.verify_compiler(root)

    def test_package_payloads_cannot_escape_the_isolated_compiler(self):
        with tempfile.TemporaryDirectory(prefix="jesses-compiler-path-test-") as directory:
            root = Path(directory).resolve()
            for name in ("../escape", "usr/../../escape", "C:/absolute", "usr\\escape", "etc/profile", "usr/./bin/tool"):
                with self.subTest(name=name), self.assertRaises(ValueError):
                    bootstrap.native_file(root, name)
            self.assertEqual(bootstrap.native_file(root, "ucrt64/bin/gcc.exe"), root / "ucrt64/bin/gcc.exe")


if __name__ == "__main__":
    unittest.main()
