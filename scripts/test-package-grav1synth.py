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
    def test_captured_cli_preserves_utf8_with_windows_ansi_default(self):
        with tempfile.TemporaryDirectory() as temporary:
            build = object.__new__(BUILD.Build)
            build.root = Path(temporary)
            build.env = dict(BUILD.os.environ)
            with patch.object(BUILD.subprocess, "_text_encoding", return_value="cp1252"):
                output = build.run([BUILD.sys.executable, "-c", "import sys; sys.stdout.buffer.write(b'1\\xe2\\x80\\x936400')"], capture=True)
            self.assertEqual(output, "1–6400")

    def test_bindgen_uses_selected_llvm_and_verified_mingw_headers(self):
        self.assertTrue(hasattr(BUILD, "bindgen_clang_args"), "Bindgen must receive the selected native Clang resource and compiler headers.")
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            clang = root / "LLVM with space/bin"
            compiler = root / "compiler"
            resource = root / "LLVM with space/lib/clang/23"
            with self.assertRaisesRegex(ValueError, "clang.exe"):
                BUILD.bindgen_clang_args(compiler, clang, {})
            clang.mkdir(parents=True)
            (clang / "clang.exe").write_bytes(b"selected clang")
            with patch.object(BUILD.subprocess, "check_output", return_value=str(resource) + "\n"):
                with self.assertRaisesRegex(ValueError, "headers"):
                    BUILD.bindgen_clang_args(compiler, clang, {})
                (resource / "include").mkdir(parents=True)
                (resource / "include/stddef.h").write_bytes(b"selected resource")
                (compiler / "ucrt64/include").mkdir(parents=True)
                arguments = BUILD.bindgen_clang_args(compiler, clang, {"PATH": "controlled"})
            self.assertEqual(BUILD.shlex.split(arguments), ["--target=x86_64-pc-windows-gnu", "-isystem", (resource / "include").as_posix(), "-isystem", (compiler / "ucrt64/include").as_posix()])

    def test_windows_import_policy_accepts_rust_system_primitives_and_rejects_private_dlls(self):
        self.assertTrue(hasattr(BUILD, "verify_windows_imports"), "The system DLL boundary must accept the GNU Rust host's Windows primitives.")
        BUILD.verify_windows_imports("DLL Name: bcryptprimitives.dll\nDLL Name: KERNEL32.dll\n")
        for name in ["avcodec-62.dll", "libwinpthread-1.dll", "libstdc++-6.dll", "std-host.dll"]:
            with self.assertRaisesRegex(ValueError, "unqualified runtime"):
                BUILD.verify_windows_imports(f"DLL Name: {name}\n")
        with self.assertRaisesRegex(ValueError, "unqualified runtime"):
            BUILD.verify_windows_imports("")

    def test_windows_uses_gnu_rust_host_for_build_scripts_and_target(self):
        self.assertTrue(hasattr(BUILD, "rust_toolchain"), "The Windows recipe must select its Rust host explicitly.")
        self.assertEqual(BUILD.rust_toolchain("1.98.1", True), "1.98.1-x86_64-pc-windows-gnu")
        self.assertEqual(BUILD.rust_toolchain("1.98.1", False), "1.98.1")

    def test_windows_compiler_runtime_precedes_rust_host_runtime(self):
        self.assertTrue(hasattr(BUILD, "windows_build_path"), "Compiler children must resolve their own verified runtime first.")
        cargo = Path("rust/bin")
        compiler = Path("compiler")
        system = Path("Windows")
        self.assertEqual(BUILD.windows_build_path(cargo, compiler, system).split(BUILD.os.pathsep),
                         list(map(str, [compiler / "ucrt64/bin", compiler / "usr/bin", cargo, system / "System32"])))

    def test_standalone_manifest_isolated_without_changing_parent_workspace(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            parent = root / "Cargo.toml"
            parent.write_bytes(b'[workspace]\nmembers=[]\n')
            source = root / "target/provider"
            source.mkdir(parents=True)
            manifest = source / "Cargo.toml"
            pristine = b'[package]\nname="standalone"\nversion="0.1.0"\nedition="2024"\n'
            manifest.write_bytes(pristine)
            inputs = root / "inputs"
            inputs.mkdir()
            with patch.object(BUILD, "MANIFEST_SHA256", BUILD.hashlib.sha256(pristine).hexdigest()):
                record = BUILD.isolate_workspace(source, inputs)
            self.assertEqual(parent.read_bytes(), b'[workspace]\nmembers=[]\n')
            self.assertEqual(manifest.read_bytes(), pristine + b'\n[workspace]\n')
            self.assertEqual((inputs / "Cargo.pristine.toml").read_bytes(), pristine)
            self.assertEqual((inputs / "Cargo.workspace.toml").read_bytes(), manifest.read_bytes())
            self.assertEqual(record["pristineSourceSha256"], BUILD.hashlib.sha256(pristine).hexdigest())
            self.assertEqual(record["patchedSourceSha256"], BUILD.MEDIA.digest(manifest))
            self.assertIn("+[workspace]", (inputs / "Cargo-workspace.patch").read_text())

    def test_workspace_isolation_rejects_changed_source_before_writing(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = root / "Cargo.toml"
            manifest.write_bytes(b"unexpected source")
            inputs = root / "inputs"
            inputs.mkdir()
            with self.assertRaisesRegex(ValueError, "manifest differs"):
                BUILD.isolate_workspace(root, inputs)
            self.assertEqual(manifest.read_bytes(), b"unexpected source")
            self.assertEqual(list(inputs.iterdir()), [])

    def test_windows_build_commands_use_explicit_isolated_launchers(self):
        compiler = Path("isolated-compiler").absolute()
        meson, make = BUILD.build_programs(compiler)
        self.assertEqual(meson, [str(compiler / "ucrt64/bin/python.exe"), "-I", "-m", "mesonbuild.mesonmain"])
        self.assertEqual(make, str(compiler / "usr/bin/make.exe"))
        self.assertEqual(BUILD.build_programs(None), (["meson"], "make"))

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
        names = ["CC", "CFLAGS", "LDFLAGS", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_TARGET_DIR", "CARGO_BUILD_RUSTFLAGS", "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_OPT_LEVEL", "PKG_CONFIG_PATH", "LIBCLANG_PATH", "BINDGEN_EXTRA_CLANG_ARGS", "LD_PRELOAD", "BASH_ENV", "ENV", "GIT_CONFIG_PARAMETERS", "CMAKE_PROJECT_INCLUDE"]
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
