"""Build-worker limits must reach native tools without multiplying across components."""

import argparse
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

sys.dont_write_bytecode = True
SCRIPTS = Path(__file__).resolve().parent


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


ffmpeg = load("bounded_ffmpeg", "build-package-ffmpeg.py")
av1an = load("bounded_av1an", "build-package-av1an.py")
vmaf = load("bounded_vmaf", "build-package-vmaf-windows.py")
codecs = load("bounded_codecs", "build-package-av1-codecs.py")
processing = load("bounded_processing", "build-package-libplacebo-windows.py")
x264 = load("bounded_x264", "build-package-x264.py")
svt = load("bounded_svt", "build-package-svt.py")
extra = load("bounded_extra", "build-package-extra-encoders.py")
scorers = load("bounded_scorers", "package-av1an-scorers.py")


class BuildJobTests(unittest.TestCase):
    def test_entry_points_reject_nonpositive_limits(self):
        for module in (ffmpeg, av1an, x264, svt, extra, scorers):
            for value in ("0", "-2", "invalid"):
                with self.subTest(module=module.__name__, value=value):
                    with self.assertRaises(argparse.ArgumentTypeError):
                        module.positive_jobs(value)
            self.assertEqual(module.positive_jobs("2"), 2)

    def test_vmaf_ninja_receives_selected_limit(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            payload = io.BytesIO()
            with tarfile.open(fileobj=payload, mode="w:gz") as archive:
                archive.addfile(tarfile.TarInfo("vmaf-3.2.0"))
            data = payload.getvalue()
            source = root / "source.tar"
            with tarfile.open(source, "w") as archive:
                item = tarfile.TarInfo("mingw-w64-vmaf/vmaf-3.2.0.tar.gz")
                item.size = len(data)
                archive.addfile(item, io.BytesIO(data))
            compiler = root / "compiler"
            (compiler / "usr/bin").mkdir(parents=True)
            (compiler / "usr/bin/xxd.exe").touch()
            (root / "vmaf-build/src").mkdir(parents=True)
            (root / "vmaf-build/src/config.h").write_text("#define VMAF_BUILT_IN_MODELS 1")
            (root / "static-vmaf/lib/pkgconfig").mkdir(parents=True)
            (root / "static-vmaf/lib/libvmaf.a").touch()
            (root / "static-vmaf/lib/pkgconfig/libvmaf.pc").write_text("Libs: -lvmaf\n")
            run = Mock()
            with patch.object(vmaf, "EMBEDDED_SHA256", hashlib.sha256(data).hexdigest()):
                vmaf.build(root, compiler, source, run, jobs=2)
            command = next(call.args[0] for call in run.call_args_list if str(call.args[0][0]).endswith("ninja.exe"))
            self.assertEqual(command[-1], "-j2")

    def test_av1_codec_cmake_receives_selected_limit(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "static-av1/lib/pkgconfig").mkdir(parents=True)
            for name in ("aom.pc", "SvtAv1Enc.pc"):
                (root / "static-av1/lib/pkgconfig" / name).touch()
            record = {"url": "source", "sha256": "pinned"}
            lock = {"linkedSources": [{**record, "id": name} for name in ("aom", "svt-av1")]}
            stager = SimpleNamespace(download=Mock(return_value=root / "archive"))
            run = Mock()
            with patch.object(codecs, "unpack_cmake", return_value=root / "cmake"), patch.object(codecs, "unpack_source", return_value=root / "source"):
                codecs.build(root, root / "compiler", lock, {"cmakeWindows": record}, root, stager, run, jobs=2)
            builds = [call.args[0] for call in run.call_args_list if "--build" in call.args[0]]
            self.assertEqual(len(builds), 2)
            self.assertTrue(all(command[-2:] == ["--parallel", "2"] for command in builds))

    def test_av1an_components_share_one_sequential_budget(self):
        events = []
        def component(name, result):
            def run(*args, jobs):
                self.assertEqual(jobs, 2)
                events.append(name)
                return result
            return run
        components = SimpleNamespace(**{name: component(name, result) for name, result in [
            ("frameserver", "frames"), ("decoder", {"prefix": "decoder"}),
            ("lsmash", "plugin"), ("av1an", "engine"),
        ]})
        result = av1an.build_components(components, Path("work"), {}, Path("compiler"), Path("cmake"), Path("rust"), Path("scripts"), None, jobs=2)
        self.assertEqual(events, ["frameserver", "decoder", "lsmash", "av1an"])
        self.assertEqual(result, ("frames", {"prefix": "decoder"}, "plugin", "engine"))

    def test_processing_install_cannot_start_an_unbounded_rebuild(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            scripts = root / "build-inputs"
            scripts.mkdir()
            (scripts / "package-ffmpeg-windows-lock.json").write_text(json.dumps({"processingSourceLock": {}}))
            (scripts / "standalone-tool-sources.json").write_text(json.dumps({"cmakeWindows": {"root": "cmake"}}))
            def build(builder, *args):
                self.assertEqual(builder.jobs, "1")
                builder.run([*builder.meson, "install", "-C", root / "build"])
                metadata = builder.prefix / "lib/pkgconfig/libplacebo.pc"
                metadata.parent.mkdir(parents=True)
                metadata.write_text("Libs: -lplacebo\n")
            vulkan = SimpleNamespace(sources=lambda *args: {}, locked_sources=lambda *args: {}, build=build)
            run = Mock()
            with patch.object(processing, "load", side_effect=[SimpleNamespace(), vulkan]):
                processing.build(root, root / "compiler", root / "cache", {}, run, jobs=1)
            self.assertEqual(run.call_args.args[0][-1], "--no-rebuild")

    def test_scorers_share_one_sequential_budget(self):
        events = []
        def zip_build(*args, jobs):
            self.assertEqual(jobs, 2)
            events.append("vszip")
            return "zip.dll"
        def julek_build(*args, jobs):
            self.assertEqual(jobs, 2)
            events.append("julek")
            return "julek.dll"
        with patch.object(scorers, "build_zip", side_effect=zip_build), patch.object(scorers, "build_julek", side_effect=julek_build):
            result = scorers.build_scorers(Path("work"), {}, Path("zig"), Path("cmake"), Path("cl"), {}, Path("compiler"), jobs=2)
        self.assertEqual(events, ["vszip", "julek"])
        self.assertEqual(result, {"vszip": "zip.dll", "julek": "julek.dll"})

    def test_vszip_zig_receives_selected_limit(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "vszip"
            source.mkdir()
            (source / "build.zig.zon").write_text(".vapoursynth = .{\n        },\n.zigimg = .{\n        },\n")
            for name in ("vsbindings", "zigimg"):
                (root / name).mkdir()
            binary = root / "vszip-install/bin/vszip.dll"
            binary.parent.mkdir(parents=True)
            binary.touch()
            sources = {name: root / name for name in ("vszip", "vsbindings", "zigimg")}
            with patch.object(scorers.support, "run") as run:
                scorers.build_zip(root, sources, Path("zig"), {}, jobs=2)
            self.assertIn("-j2", run.call_args.args[0])


if __name__ == "__main__":
    unittest.main()
