"""Cross-platform extraction and dependency checks for the Linux source build."""

import hashlib
import importlib.util
from contextlib import redirect_stderr
import io
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
import sys
from unittest.mock import patch

sys.dont_write_bytecode = True

SPEC = importlib.util.spec_from_file_location("linux_ffmpeg", Path(__file__).with_name("build-package-ffmpeg-linux.py"))
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)


def archive(path, files, links=()):
    with tarfile.open(path, "w:gz") as bundle:
        for name, content in files:
            member = tarfile.TarInfo(name)
            member.size = len(content)
            member.mode = 0o644
            bundle.addfile(member, io.BytesIO(content))
        for name, target in links:
            member = tarfile.TarInfo(name)
            member.type = tarfile.SYMTYPE
            member.linkname = target
            bundle.addfile(member)


class SourceBuildTests(unittest.TestCase):
    def test_failed_command_surfaces_bounded_build_log_tail(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            builder = BUILD.Builder(root / "build", root / "cache", 1)
            stderr = io.StringIO()
            with self.assertRaises(subprocess.CalledProcessError), redirect_stderr(stderr):
                builder.run([sys.executable, "-c", "import sys; print('native compiler failure'); sys.exit(3)"])
            diagnostic = stderr.getvalue()
            self.assertIn("Build command failed", diagnostic)
            self.assertIn("native compiler failure", diagnostic)
            self.assertTrue((root / "build/build.log").is_file())

    def test_failed_captured_command_surfaces_captured_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            builder = BUILD.Builder(root / "build", root / "cache", 1)
            stderr = io.StringIO()
            with self.assertRaises(subprocess.CalledProcessError), redirect_stderr(stderr):
                builder.run([sys.executable, "-c", "import sys; print('captured probe failure'); sys.exit(4)"], capture=True)
            diagnostic = stderr.getvalue()
            self.assertIn("Last 1 captured output lines", diagnostic)
            self.assertIn("captured probe failure", diagnostic)

    def test_build_environment_excludes_host_shell_and_compiler_overrides(self):
        overrides = {name: "unexpected-host-input" for name in ["BASH_ENV", "ENV", "MFLAGS", "MAKEFLAGS", "CC", "CFLAGS", "LDFLAGS", "PKG_CONFIG", "PKG_CONFIG_PATH", "PKG_CONFIG_SYSROOT_DIR", "LD_PRELOAD", "CMAKE_TOOLCHAIN_FILE", "GIT_CONFIG_PARAMETERS", "GIT_TEMPLATE_DIR"]}
        with tempfile.TemporaryDirectory() as temporary, patch.dict(BUILD.os.environ, overrides):
            root = Path(temporary)
            builder = BUILD.Builder(root / "build", root / "cache", 1)
            self.assertFalse(any(value == "unexpected-host-input" for value in builder.env.values()))
            self.assertEqual(builder.env["PKG_CONFIG_LIBDIR"], str(builder.prefix) + "/lib/pkgconfig")
            self.assertEqual(builder.env["CC"], "gcc")

    def test_empty_directories_required_by_bare_git_are_retained(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "git.tar"
            with tarfile.open(source, "w") as bundle:
                member = tarfile.TarInfo("source/refs")
                member.type = tarfile.DIRTYPE
                bundle.addfile(member)
            extracted = BUILD.extract(source, root / "extracted", "source")
            self.assertTrue((extracted / "refs").is_dir())

    def test_extraction_materializes_internal_source_links(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source.tar.gz"
            archive(source, [("source/COPYING", b"License text"), ("source/src/util.h", b"header")], [("source/doc/util.h", "../src/util.h")])
            extracted = BUILD.extract(source, root / "extracted", "source")
            self.assertEqual((extracted / "doc/util.h").read_bytes(), b"header")
            self.assertFalse((extracted / "doc/util.h").is_symlink())
            self.assertEqual((extracted / "COPYING").read_bytes(), b"License text")

    def test_extraction_preserves_generated_file_mtimes_independent_of_member_order(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source.tar.gz"
            with tarfile.open(source, "w:gz") as bundle:
                for name, content, mtime in [("source/configure", b"generated", 1_700_000_200), ("source/configure.ac", b"input", 1_700_000_100)]:
                    member = tarfile.TarInfo(name)
                    member.size = len(content)
                    member.mtime = mtime
                    bundle.addfile(member, io.BytesIO(content))
            extracted = BUILD.extract(source, root / "extracted", "source")
            self.assertEqual(int((extracted / "configure").stat().st_mtime), 1_700_000_200)
            self.assertEqual(int((extracted / "configure.ac").stat().st_mtime), 1_700_000_100)
            self.assertGreater((extracted / "configure").stat().st_mtime, (extracted / "configure.ac").stat().st_mtime)

    def test_archive_traversal_duplicate_members_and_escaping_links_fail(self):
        cases = [
            ([("source/../../outside", b"no")], []),
            ([("source/file", b"first"), ("source/file", b"second")], []),
            ([("other/file", b"no")], []),
            ([("source/file", b"data")], [("source/link", "../../outside")]),
            ([("source/file", b"data")], [("source/link", "missing")]),
        ]
        for files, links in cases:
            with self.subTest(files=files, links=links), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                source = root / "source.tar.gz"
                archive(source, files, links)
                with self.assertRaises(ValueError):
                    BUILD.extract(source, root / "extracted", "source")
                self.assertFalse((root / "outside").exists())

    def test_tampered_cached_source_is_retained_and_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            cached = root / "source.tar.gz"
            cached.write_bytes(b"changed source")
            item = {"filename": cached.name, "sha256": hashlib.sha256(b"expected source").hexdigest(), "url": "https://example.invalid/source.tar.gz"}
            with self.assertRaises(ValueError):
                BUILD.download(item, root)
            self.assertEqual(cached.read_bytes(), b"changed source")

    def test_receipt_keeps_source_and_license_hashes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            cache = root / "cache"
            cache.mkdir()
            upstream = cache / "source.tar.gz"
            archive(upstream, [("source/COPYING", b"Exact source license")])
            item = {"filename": upstream.name, "sha256": BUILD.digest(upstream), "url": "https://example.invalid/source.tar.gz", "sourceRoot": "source", "licenseFiles": ["COPYING"]}
            builder = BUILD.Builder(root / "new-build", cache, 1)
            builder.source(item)
            self.assertEqual(builder.sources[0]["sha256"], item["sha256"])
            for record in [*builder.sources, *builder.licenses]:
                self.assertEqual(BUILD.digest(builder.delivery / record["path"]), record["sha256"])
            with self.assertRaises(FileExistsError):
                BUILD.Builder(root / "new-build", cache, 1)

    def test_dynamic_codec_or_temporary_dependencies_fail(self):
        prefix = Path("/tmp/jesses-build/prefix")
        system = "linux-vdso.so.1 (0x0001)\nlibc.so.6 => /lib/libc.so.6 (0x0002)\n/lib64/ld-linux-x86-64.so.2 (0x0003)\n"
        BUILD.validate_dependencies(system + "libmvec.so.1 => /lib/libmvec.so.1 (0x0004)\nlibstdc++.so.6 => /lib/libstdc++.so.6 (0x0005)\n", prefix)
        for unexpected in ["", "not a dynamic executable\n", system + "libx265.so.217 => /lib/libx265.so.217\n", system + "libz.so.1 => not found\n", system.replace("/lib/libc", str(prefix) + "/lib/libc")]:
            with self.subTest(text=unexpected), self.assertRaises(ValueError):
                BUILD.validate_dependencies(unexpected, prefix)


if __name__ == "__main__":
    unittest.main()
