"""Integrity and isolation regressions for the source-pinned grain provider."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("grain_builder", Path(__file__).with_name("build-package-grav1synth.py"))
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)

class GrainProviderTests(unittest.TestCase):
    def test_source_matches_fork_pin_and_complete_stock_matrix(self):
        lock = json.loads(BUILD.LOCK.read_text())
        self.assertEqual(lock["source"]["commit"], "1044228cd411672b565e5762a9b3597f4dd163b0")
        expected = ["Super8", "MaxMid"] + [base+suffix for base in ["16mm", "Classic35", "Modern35"] for suffix in ["", "-1", "-2", "-3"]]
        self.assertEqual(lock["presets"], expected)
        fixture = json.loads((BUILD.HERE.parent / "crates/media-runtime/tests/fixtures/grav1synth-presets.json").read_text())
        self.assertEqual(fixture["sourceCommit"], lock["source"]["commit"])
        self.assertEqual([row["name"] for row in fixture["presets"]], expected)
        self.assertIn("--enable-libdav1d", BUILD.CONFIGURE)
        self.assertIn("--disable-autodetect", BUILD.CONFIGURE)
        self.assertIn("--disable-shared", BUILD.CONFIGURE)
        self.assertNotIn("--disable-decoders", BUILD.CONFIGURE)

    def test_environment_removes_compiler_cargo_loader_and_shell_injection(self):
        names = ["CC", "CFLAGS", "LDFLAGS", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_TARGET_DIR", "CARGO_BUILD_RUSTFLAGS", "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_OPT_LEVEL", "PKG_CONFIG_PATH", "LIBCLANG_PATH", "LD_PRELOAD", "BASH_ENV", "ENV", "GIT_CONFIG_PARAMETERS", "CMAKE_PROJECT_INCLUDE"]
        with patch.dict(BUILD.os.environ, {name: "unrelated-host-input" for name in names}):
            env = BUILD.environment(Path("/tmp/owned-home"))
        self.assertFalse(any(value == "unrelated-host-input" for value in env.values()))
        self.assertEqual(env["CARGO_HOME"], str(Path("/tmp/owned-home/cargo")))
        self.assertEqual(env["GIT_CONFIG_GLOBAL"], BUILD.os.devnull)

    def test_receipt_validates_every_payload_and_rejects_tamper_extra_and_wrong_pin(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            def item(name, data, **extra):
                path = root/name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
                return {"path": name, "sha256": BUILD.MEDIA.digest(path), **extra}
            source = item("source.tar", b"original exact source")
            ffmpeg = item("ffmpeg.tar", b"private compatible FFmpeg", id="ffmpeg")
            dav1d = item("dav1d.tar", b"complete AV1 decoder", id="dav1d")
            vendor = item("vendor.tar", b"complete pinned crates", id="cargo-vendor")
            lock = {"source": {"commit": "exact", "sha256": source["sha256"]}, "ffmpeg": ffmpeg, "dav1d": dav1d, "presets": ["one"]}
            lock_path = root/"active-lock.json"
            lock_path.write_text(json.dumps(lock))
            # The active lock is outside the delivery inventory.
            payload = root/"delivery"
            payload.mkdir()
            for entry in [source, ffmpeg, dav1d, vendor]:
                BUILD.shutil.move(root/entry["path"], payload/entry["path"])
            def payload_item(name, data):
                path = payload/name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
                return {"path": name, "sha256": BUILD.MEDIA.digest(path)}
            receipt = {"schemaVersion": 1, "target": "x86_64-unknown-linux-gnu", "sourceCommit": "exact", "source": source, "tool": {**payload_item("grav1synth", b"qualified executable"), "id": "grav1synth"}, "additionalSources": [ffmpeg,dav1d,vendor], "licenses": [payload_item("LICENSE", b"MIT")], "buildInputs": [payload_item("recipe.py", b"exact recipe"), payload_item("build/grav1synth-lock.json", lock_path.read_bytes())], "supportFiles": [], "qualificationFiles": [], "qualification": {"presets": ["one"]}}
            path = payload/"build-provenance.json"
            def write():
                path.write_text(json.dumps(receipt))
            with patch.object(BUILD, "LOCK", lock_path):
                write()
                BUILD.verify_delivery(payload, receipt["target"])
                (payload/"unlisted").write_bytes(b"extra")
                with self.assertRaisesRegex(ValueError, "complete hash"):
                    BUILD.verify_delivery(payload, receipt["target"])
                (payload/"unlisted").unlink()
                (payload/"grav1synth").write_bytes(b"changed")
                with self.assertRaisesRegex(ValueError, "complete hash"):
                    BUILD.verify_delivery(payload, receipt["target"])
                (payload/"grav1synth").write_bytes(b"qualified executable")
                receipt["additionalSources"][0]["sha256"] = "0"*64
                write()
                with self.assertRaisesRegex(ValueError, "source differs"):
                    BUILD.verify_delivery(payload, receipt["target"])
                receipt["sourceCommit"] = "other revision"
                write()
                with self.assertRaisesRegex(ValueError, "source or platform"):
                    BUILD.verify_delivery(payload, receipt["target"])

    def test_failed_build_work_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root/"keep.log").write_bytes(b"prior evidence")
            with self.assertRaises(FileExistsError):
                BUILD.Build(root, root/"cache", 1)
            self.assertEqual((root/"keep.log").read_bytes(), b"prior evidence")

if __name__ == "__main__":
    unittest.main()
