"""Regression checks for pinned Windows mkvmerge package boundaries."""

import hashlib
import importlib.util
import io
import json
from copy import deepcopy
from pathlib import Path
import struct
import sys
import tarfile
import tempfile
import unittest

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("mkvmerge_builder", ROOT / "scripts/build-package-mkvmerge.py")
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)
stage_spec = importlib.util.spec_from_file_location("mkvmerge_stage", ROOT / "scripts/stage-bundled-tools.py")
stage = importlib.util.module_from_spec(stage_spec)
stage_spec.loader.exec_module(stage)


def fake_pe(imports):
    payload = bytearray(0x600)
    payload[:2] = b"MZ"
    struct.pack_into("<I", payload, 0x3C, 0x80)
    payload[0x80:0x84] = b"PE\0\0"
    struct.pack_into("<H", payload, 0x84, 0x8664)
    struct.pack_into("<H", payload, 0x86, 1)
    struct.pack_into("<H", payload, 0x94, 0xF0)
    opt = 0x98
    struct.pack_into("<H", payload, opt, 0x20B)
    struct.pack_into("<Q", payload, opt + 24, 0x140000000)
    struct.pack_into("<I", payload, opt + 60, 0x200)
    struct.pack_into("<I", payload, opt + 108, 16)
    struct.pack_into("<II", payload, opt + 120, 0x1000, 20 * (len(imports) + 1))
    section = opt + 0xF0
    struct.pack_into("<IIII", payload, section + 8, 0x400, 0x1000, 0x400, 0x200)
    for index, name in enumerate(imports):
        rva = 0x1100 + index * 0x40
        struct.pack_into("<I", payload, 0x200 + index * 20 + 12, rva)
        encoded = name.encode("ascii") + b"\0"
        payload[0x300 + index * 0x40:0x300 + index * 0x40 + len(encoded)] = encoded
    return payload


def tar_with(name, data=b"sample", kind=tarfile.REGTYPE):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w") as archive:
        member = tarfile.TarInfo(name)
        member.type = kind
        member.size = len(data) if kind == tarfile.REGTYPE else 0
        archive.addfile(member, io.BytesIO(data) if kind == tarfile.REGTYPE else None)
    output.seek(0)
    return output


class MkvmergePackagingTests(unittest.TestCase):
    def test_lock_covers_every_binary_package_with_exact_version_sources(self):
        lock = json.loads(builder.LOCK.read_text(encoding="utf-8"))
        packages = lock["packages"]
        sources = lock["sources"]
        files = lock["files"]
        self.assertEqual(lock["schemaVersion"], 2)
        self.assertEqual(lock["source"]["sourceRoot"], "mkvtoolnix-104.0")
        self.assertTrue(packages and sources and files and lock["buildPackages"])
        self.assertEqual({(item["base"], item["version"]) for item in packages},
                         {(item["base"], item["version"]) for item in sources})
        self.assertEqual(len({item["path"].lower() for item in files}), len(files))
        self.assertEqual(len({item["name"] for item in packages}), len(packages))
        self.assertEqual(len({item["base"] for item in sources}), len(sources))
        binary_names = {item["path"].lower() for item in files}
        self.assertNotIn("mkvmerge.exe", binary_names)
        for item in files:
            self.assertIn(item["package"], {package["name"] for package in packages})
            self.assertEqual(item["member"], "ucrt64/bin/" + item["path"])
            for imported in item["imports"]:
                self.assertTrue(imported.lower() in binary_names | builder.SYSTEM_IMPORTS or
                                imported.lower().startswith(("api-ms-win-", "ext-ms-win-")),
                                f"Unpackaged import: {item['path']} -> {imported}")
        for imported in lock["imports"]:
            self.assertTrue(imported.lower() in binary_names | builder.SYSTEM_IMPORTS or
                            imported.lower().startswith(("api-ms-win-", "ext-ms-win-")), imported)
        for item in [*packages, *sources, *lock["buildPackages"]]:
            self.assertEqual(len(item["sha256"]), 64)
            self.assertTrue(item["url"].startswith("https://repo.msys2.org/"))
        self.assertEqual(lock["versionLine"], "mkvmerge v104.0 ('Chimera') 64-bit")
        self.assertEqual(len(lock["source"]["sha256"]), 64)

    def test_cli_icon_keeps_all_reviewed_dimensions_from_the_upstream_icon(self):
        chunks = []
        for size in (32, 64, 128, 256):
            data = b"\x89PNG\r\n\x1a\n" + b"\0\0\0\rIHDR" + struct.pack(">II", size, size)
            chunks.append(struct.pack(">4sI", b"icon", len(data) + 8) + data)
        payload = b"".join(chunks)
        icon = builder.cli_icon(struct.pack(">4sI", b"icns", len(payload) + 8) + payload)
        self.assertEqual(struct.unpack_from("<HHH", icon), (0, 1, 4))
        self.assertEqual([icon[6 + i * 16] for i in range(4)], [32, 64, 128, 0])

    def test_source_extraction_preserves_drake_archive_link(self):
        with tempfile.TemporaryDirectory(prefix="jesses-mkvmerge-source-") as directory:
            scratch = Path(directory)
            archive_path = scratch / "source.tar"
            with tarfile.open(archive_path, "w") as archive:
                body = b"puts 'drake'\n"
                real = tarfile.TarInfo("mkvtoolnix/source/drake")
                real.size = len(body)
                archive.addfile(real, io.BytesIO(body))
                link = tarfile.TarInfo("mkvtoolnix/drake")
                link.type = tarfile.SYMTYPE
                link.linkname = "source/drake"
                archive.addfile(link)
            source = builder.extract_source(archive_path, scratch / "extracted", "mkvtoolnix")
            self.assertEqual((source / "drake").read_bytes(), body)

    def test_pe_parser_and_closure_reject_unknown_or_corrupt_imports(self):
        with tempfile.TemporaryDirectory(prefix="jesses-mkvmerge-pe-") as directory:
            path = Path(directory) / "mkvmerge.exe"
            path.write_bytes(fake_pe(["KERNEL32.dll", "libsample.dll"]))
            self.assertEqual(builder.pe_imports(path), ["KERNEL32.dll", "libsample.dll"])
            with self.assertRaisesRegex(ValueError, "Unpackaged"):
                builder.verify_imports([{"path": "mkvmerge.exe", "imports": ["KERNEL32.dll", "libsample.dll"]}], Path(directory))
            changed = fake_pe(["KERNEL32.dll"])
            struct.pack_into("<I", changed, 0x200 + 12, 0x5000)
            path.write_bytes(changed)
            with self.assertRaisesRegex(ValueError, "Invalid PE import address"):
                builder.pe_imports(path)

    def test_exact_archive_members_reject_redirection_and_unsafe_names(self):
        for name in ("../escape", "C:/escape", "/absolute", "dir\\escape"):
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "Unsafe package member"):
                builder.member_path(name)
        with tarfile.open(fileobj=tar_with("safe/link", kind=tarfile.SYMTYPE)) as archive:
            members = builder.checked_members(archive)
            with self.assertRaisesRegex(ValueError, "ordinary package member"):
                builder.ordinary_member(archive, members, "safe/link")
        with tarfile.open(fileobj=tar_with("safe/file", b"actual source")) as archive:
            members = builder.checked_members(archive)
            self.assertEqual(builder.ordinary_member(archive, members, "safe/file"), b"actual source")

    def test_changed_cached_input_cannot_be_reused(self):
        with tempfile.TemporaryDirectory(prefix="jesses-mkvmerge-cache-") as directory:
            cache = Path(directory)
            (cache / "payload.pkg.tar.zst").write_bytes(b"changed")
            record = {"filename": "payload.pkg.tar.zst", "url": "https://repo.msys2.org/mingw/ucrt64/payload.pkg.tar.zst",
                      "sha256": hashlib.sha256(b"expected").hexdigest()}
            with self.assertRaisesRegex(ValueError, "cached MKVToolNix input changed"):
                builder.download(record, cache)

    def test_removed_or_modified_notice_cannot_be_staged(self):
        locked = json.loads((ROOT / "scripts/package-mkvmerge-notices-lock.json").read_text())
        self.assertEqual(locked["packageLockSha256"], builder.digest(builder.LOCK))
        self.assertTrue(locked["licenses"])
        stage.verify_mkvmerge_notices(locked["licenses"])
        missing = deepcopy(locked["licenses"])
        missing.pop()
        with self.assertRaisesRegex(ValueError, "notice inventory"):
            stage.verify_mkvmerge_notices(missing)
        tampered = deepcopy(locked["licenses"])
        tampered[0]["sha256"] = hashlib.sha256(b"modified notice").hexdigest()
        with self.assertRaisesRegex(ValueError, "notice inventory"):
            stage.verify_mkvmerge_notices(tampered)


if __name__ == "__main__":
    unittest.main()
