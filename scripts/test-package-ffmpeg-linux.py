"""Cross-platform extraction and dependency checks for the Linux source build."""

import hashlib
import importlib.util
import io
from pathlib import Path
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
        BUILD.validate_dependencies(system + "libstdc++.so.6 => /lib/libstdc++.so.6 (0x0004)\n", prefix)
        for unexpected in ["", "not a dynamic executable\n", system + "libx265.so.217 => /lib/libx265.so.217\n", system + "libz.so.1 => not found\n", system.replace("/lib/libc", str(prefix) + "/lib/libc")]:
            with self.subTest(text=unexpected), self.assertRaises(ValueError):
                BUILD.validate_dependencies(unexpected, prefix)


if __name__ == "__main__":
    unittest.main()
