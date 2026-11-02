"""Structural/package safety checks; synthetic Mach-O fixtures are not native execution evidence."""
import importlib.util
import json
from pathlib import Path
import plistlib
import struct
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("macos_package", Path(__file__).with_name("package-macos.py"))
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)

class MacPackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="jesses-mac-package-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.app = self.root / "jesses.app"
        self.binary = self.app / "Contents/MacOS/jesses"
        self.binary.parent.mkdir(parents=True)
        self.binary.write_bytes(struct.pack("<IiiIIIII", 0xFEEDFACF, 0x0100000C, 0, 2, 0, 0, 0, 0))
        self.binary.chmod(0o755)
        with (self.app / "Contents/Info.plist").open("wb") as output:
            plistlib.dump({"CFBundleIdentifier": "io.github.jkkma.jesses", "CFBundleExecutable": "jesses", "CFBundleShortVersionString": "1.0.1"}, output)
        resource = self.app / "Contents/Resources/resources"
        (resource / "licenses").mkdir(parents=True)
        for name in ["LICENSE", "THIRD_PARTY_NOTICES.md", "licenses/shadcn-svelte-MIT.txt"]:
            (resource / name).write_text("synthetic license fixture\n")
        (resource / "runtime-contract.json").write_text(json.dumps({"platforms": {"aarch64-apple-darwin": {}, "x86_64-apple-darwin": {}}}))

    def test_archive_retains_exact_bytes_and_executable_mode(self):
        output = package.collect(self.app, self.root / "package", "aarch64-apple-darwin", "debug")
        receipt = package.verify_archive(output)
        entry = next(item for item in receipt["files"] if item["path"] == "jesses.app/Contents/MacOS/jesses")
        self.assertEqual(entry["mode"], 0o755)
        self.assertEqual(entry["sha256"], package.digest(self.binary))
        self.assertEqual(receipt["tools"], "external")
        self.assertFalse(receipt["notarized"])
        with self.assertRaises(FileExistsError):
            package.collect(self.app, self.root / "package", "aarch64-apple-darwin", "debug")

    def test_wrong_architecture_and_redirected_payload_are_rejected(self):
        with self.assertRaises(ValueError):
            package.verify_app(self.app, "x86_64-apple-darwin")
        target = self.root / "outside.txt"
        target.write_text("unrelated user file")
        (self.app / "Contents/Resources/link").symlink_to(target)
        with self.assertRaises(ValueError):
            package.verify_app(self.app, "aarch64-apple-darwin")
        self.assertEqual(target.read_text(), "unrelated user file")

    def test_tampered_package_and_unsafe_members_are_rejected(self):
        package.collect(self.app, self.root / "package", "aarch64-apple-darwin", "debug")
        binary = self.root / "package/payload/jesses.app/Contents/MacOS/jesses"
        binary.write_bytes(binary.read_bytes() + b"tamper")
        with self.assertRaises(ValueError):
            package.verify_directory(self.root / "package/payload")
        for name in ["../escape", "/absolute", "a/../../escape", "a\\b", "a\nfile"]:
            with self.assertRaises(ValueError):
                package.safe_name(name)

if __name__ == "__main__":
    unittest.main()
