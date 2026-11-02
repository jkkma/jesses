"""AV1 dependency extraction gates run without executing untrusted build scripts."""

import hashlib
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location("av1_dependencies", Path(__file__).with_name("build-package-av1-codecs.py"))
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)


def source_archive(directory, members):
    payload = io.BytesIO()
    with tarfile.open(fileobj=payload, mode="w:gz") as source:
        for name, content in members:
            member = tarfile.TarInfo(name)
            member.size = len(content)
            source.addfile(member, io.BytesIO(content))
    data = payload.getvalue()
    archive = directory / "complete.tar.gz"
    with tarfile.open(archive, "w:gz") as outer:
        member = tarfile.TarInfo("distribution/upstream.tar.gz")
        member.size = len(data)
        outer.addfile(member, io.BytesIO(data))
    return archive, {"sha256": BUILD.digest(archive), "embeddedArchive": "distribution/upstream.tar.gz", "embeddedSha256": hashlib.sha256(data).hexdigest(), "sourceRoot": "upstream"}


class Av1DependencyTests(unittest.TestCase):
    def test_exact_complete_and_embedded_sources_are_required(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive, record = source_archive(root, [("upstream/LICENSE", b"original license"), ("upstream/source.c", b"source")])
            source = BUILD.unpack_source(archive, record, root / "extracted")
            self.assertEqual((source / "LICENSE").read_bytes(), b"original license")
            for key in ("sha256", "embeddedSha256"):
                with self.subTest(key=key), self.assertRaises(ValueError):
                    BUILD.unpack_source(archive, {**record, key: "0" * 64}, root / key)

    def test_foreign_paths_and_duplicate_files_are_rejected(self):
        for members in [[("upstream/../../escape", b"bad")], [("other/source.c", b"bad")], [("upstream/source.c", b"first"), ("upstream/source.c", b"second")]]:
            with self.subTest(members=members), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                archive, record = source_archive(root, members)
                with self.assertRaises(ValueError):
                    BUILD.unpack_source(archive, record, root / "extracted")
                self.assertFalse((root / "escape").exists())

    def test_existing_extraction_directory_is_not_replaced(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive, record = source_archive(root, [("upstream/source.c", b"source")])
            output = root / "extracted"
            output.mkdir()
            (output / "keep").write_bytes(b"unrelated")
            with self.assertRaises(FileExistsError):
                BUILD.unpack_source(archive, record, output)
            self.assertEqual((output / "keep").read_bytes(), b"unrelated")


if __name__ == "__main__":
    unittest.main()
