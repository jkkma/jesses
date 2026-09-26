"""Focused hash, closure and runtime-invariance tests for the QTGMC merger."""

import importlib.util
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest
from unittest import mock
import zipfile


sys.dont_write_bytecode = True
SCRIPTS = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("package_av1an_qtgmc", SCRIPTS / "package-av1an-qtgmc.py")
MERGER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MERGER)


def module_zip(path, *, extra=None):
    contents = {
        "havsfunc.py": "import vapoursynth\n",
        "vsutil/__init__.py": "\n",
    }
    if extra:
        contents.update(extra)
    with zipfile.ZipFile(path, "w") as archive:
        for name, content in sorted(contents.items()):
            archive.writestr(name, content)


def qtgmc_fixture(root):
    delivery = root / "qtgmc"
    delivery.mkdir(parents=True)
    module_zip(delivery / "qtgmc-deps.zip")
    plugins = []
    for name in sorted(MERGER.PLUGIN_FILES | MERGER.RUNTIME_FILES):
        filename = "MiscFilters.dll" if name == "miscfilters.dll" else name
        relative = ("runtime" if name in MERGER.RUNTIME_FILES else "plugins") + "/" + filename
        path = delivery / relative
        path.parent.mkdir(exist_ok=True)
        path.write_bytes(name.encode())
        plugins.append({"id": name, "path": relative, "sha256": MERGER.support.digest(path)})
    sources = []
    notices = []
    for component in ("fft3dfilter", "miscfilters"):
        source = delivery / f"sources/{component}-source.tar.gz"
        source.parent.mkdir(exist_ok=True)
        source.write_bytes(f"{component} source".encode())
        sources.append({"component": component, "path": f"sources/{component}-source.tar.gz",
                        "sha256": MERGER.support.digest(source)})
        license_path = delivery / f"licenses/{component}/LICENSE.txt"
        license_path.parent.mkdir(parents=True)
        license_path.write_bytes(f"{component} license".encode())
        notices.append({"path": f"licenses/{component}/LICENSE.txt", "sha256": MERGER.support.digest(license_path)})
    recipe = delivery / "build/build-package-av1an-qtgmc.py"
    recipe.parent.mkdir()
    recipe.write_bytes(b"recipe")
    lock_recipe = delivery / "build/qtgmc-windows-lock.json"
    lock_recipe.write_bytes(b"{}")
    static = []
    logs = []
    for relative, component in (("plugins/fft3dfilter.dll", "fft3dfilter"),
                                ("plugins/MiscFilters.dll", "miscfilters")):
        log = f"build/{component}-static.log"
        log_path = delivery / log
        log_path.write_bytes(b"static CRT build")
        logs.append({"path": log, "sha256": MERGER.support.digest(log_path)})
        plugin = next(item for item in plugins if item["path"] == relative)
        static.append({"component": component, "path": relative, "sha256": plugin["sha256"],
                       "runtime": "static-msvc-/MT", "log": log, "logSha256": MERGER.support.digest(log_path)})
    receipt = {
        "schemaVersion": 1,
        "target": "x86_64-pc-windows-msvc",
        "pythonModules": [{"path": "qtgmc-deps.zip", "sha256": MERGER.support.digest(delivery / "qtgmc-deps.zip")}],
        "plugins": plugins,
        "additionalSources": sources,
        "licenses": notices,
        "buildInputs": [
            {"path": "build/build-package-av1an-qtgmc.py", "sha256": MERGER.support.digest(recipe)},
            {"path": "build/qtgmc-windows-lock.json", "sha256": MERGER.support.digest(lock_recipe)},
            *logs,
        ],
        "compiler": {"compilerSha256": "a" * 64, "msvcVersion": "14.44", "sdkVersion": "10.0"},
        "sourceBuild": static,
    }
    (delivery / "build-provenance.json").write_text(json.dumps(receipt), encoding="utf-8")
    lock = {
        "schemaVersion": 1,
        "target": "x86_64-pc-windows-msvc",
        "inputs": {record["component"]: {"sourceSha256": record["sha256"]} for record in sources},
        "sourceBuiltOutputs": sorted(MERGER.SOURCE_BUILT_OUTPUTS),
        "outputs": {record["path"]: record["sha256"] for group in ("pythonModules", "plugins")
                    for record in receipt[group] if record["path"] not in MERGER.SOURCE_BUILT_OUTPUTS},
    }
    return delivery, receipt, lock


class QtgmcPackageTests(unittest.TestCase):
    def test_python_archive_accepts_only_reviewed_source_modules(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "modules.zip"
            module_zip(path)
            MERGER.checked_python_zip(path)
            module_zip(path, extra={"surprise.py": "print('unexpected')"})
            with self.assertRaisesRegex(ValueError, "Unexpected QTGMC Python module"):
                MERGER.checked_python_zip(path)
            module_zip(path, extra={"vsutil/__pycache__/x.pyc": "compiled"})
            with self.assertRaises(ValueError):
                MERGER.checked_python_zip(path)

    def test_source_and_runtime_hashes_must_match_lock(self):
        with tempfile.TemporaryDirectory() as temporary:
            delivery, receipt, lock = qtgmc_fixture(Path(temporary))
            self.assertEqual(MERGER.checked_qtgmc_delivery(delivery, lock), receipt)
            lock["outputs"]["qtgmc-deps.zip"] = "0" * 64
            with self.assertRaisesRegex(ValueError, "pinned output hashes"):
                MERGER.checked_qtgmc_delivery(delivery, lock)
            lock["outputs"]["qtgmc-deps.zip"] = receipt["pythonModules"][0]["sha256"]
            lock["inputs"]["fft3dfilter"]["sourceSha256"] = "0" * 64
            with self.assertRaisesRegex(ValueError, "pinned source closure"):
                MERGER.checked_qtgmc_delivery(delivery, lock)

    def test_source_notices_and_recipe_are_required(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            delivery, receipt, lock = qtgmc_fixture(root)
            for notice in receipt["licenses"]:
                (delivery / notice["path"]).unlink()
            receipt["licenses"] = []
            (delivery / "build-provenance.json").write_text(json.dumps(receipt), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "no source notices"):
                MERGER.checked_qtgmc_delivery(delivery, lock)

            other, receipt, lock = qtgmc_fixture(root / "second")
            recipe = next(item for item in receipt["buildInputs"] if item["path"].endswith("build-package-av1an-qtgmc.py"))
            (other / recipe["path"]).unlink()
            receipt["buildInputs"].remove(recipe)
            (other / "build-provenance.json").write_text(json.dumps(receipt), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "pinned build recipe"):
                MERGER.checked_qtgmc_delivery(other, lock)

    def test_pinned_build_inputs_match_local_recipe(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            delivery, receipt, _ = qtgmc_fixture(root)
            local = root / "scripts"
            local.mkdir()
            for record in receipt["buildInputs"]:
                source = delivery / record["path"]
                shutil.copy2(source, local / source.name)
            with (mock.patch.object(MERGER, "SCRIPTS", local),
                  mock.patch.object(MERGER, "LOCK", local / "qtgmc-windows-lock.json")):
                MERGER.checked_local_recipe(receipt)
                (local / "build-package-av1an-qtgmc.py").write_bytes(b"different")
                with self.assertRaisesRegex(ValueError, "different build input"):
                    MERGER.checked_local_recipe(receipt)

    def test_static_build_receipt_binds_source_output_crt_and_log(self):
        with tempfile.TemporaryDirectory() as temporary:
            delivery, original, lock = qtgmc_fixture(Path(temporary))
            for field, forged in (
                ("component", "unrelated"),
                ("sha256", "0" * 64),
                ("runtime", "dynamic-msvc-/MD"),
                ("logSha256", "0" * 64),
            ):
                receipt = json.loads(json.dumps(original))
                receipt["sourceBuild"][0][field] = forged
                (delivery / "build-provenance.json").write_text(json.dumps(receipt), encoding="utf-8")
                with self.assertRaisesRegex(ValueError, "not bound"):
                    MERGER.checked_qtgmc_delivery(delivery, lock)
            receipt = json.loads(json.dumps(original))
            receipt["sourceBuild"][0]["log"] = "build/missing.log"
            receipt["sourceBuild"][0].pop("logSha256")
            (delivery / "build-provenance.json").write_text(json.dumps(receipt), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "not bound"):
                MERGER.checked_qtgmc_delivery(delivery, lock)

    def test_install_replaces_search_path_hash_without_loose_python_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            base = root / "base"
            python = base / "python"
            python.mkdir(parents=True)
            pth = python / "python314._pth"
            pth.write_text("python314.zip\n.\nLib/site-packages\n", encoding="utf-8")
            (python / "Lib/site-packages/vapoursynth/plugins").mkdir(parents=True)
            original = {"supportFiles": [{"path": "python/python314._pth", "sha256": MERGER.support.digest(pth)}],
                        "additionalSources": [], "licenses": [], "buildInputs": []}
            delivery, extension, _ = qtgmc_fixture(root)
            lock = root / "qtgmc-windows-lock.json"
            lock.write_bytes(b"{}")
            with mock.patch.object(MERGER, "LOCK", lock):
                MERGER.install_runtime(base, delivery, original, extension, root / "merged")
            merged = root / "merged"
            self.assertTrue((merged / MERGER.PYTHON_ZIP).is_file())
            self.assertTrue((merged / MERGER.FRAMESERVER / "plugins/libfftw3f-3.dll").is_file())
            self.assertTrue((merged / MERGER.FRAMESERVER / "plugins/mvtools.dll").is_file())
            self.assertIn("qtgmc-deps.zip", (merged / "python/python314._pth").read_text())
            self.assertFalse((merged / "python/Lib/site-packages/havsfunc.py").exists())
            hashes = {item["path"]: item["sha256"] for item in original["supportFiles"]}
            self.assertEqual(hashes["python/python314._pth"], MERGER.support.digest(merged / "python/python314._pth"))

    def test_probe_requires_all_presets_and_no_runtime_mutation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            runtime_root = root / "runtime"
            python = runtime_root / "python"
            python.mkdir(parents=True)
            (python / "marker").write_bytes(b"unchanged")
            calls = []

            def run(command, **kwargs):
                calls.append(command)
                return mock.Mock(returncode=0)

            with (mock.patch.object(MERGER.runtime, "runtime_environment", return_value={}),
                  mock.patch.object(MERGER.subprocess, "run", side_effect=run)):
                MERGER.qualify_qtgmc(runtime_root, root / "scratch")
            self.assertEqual(len(calls), len(MERGER.PRESETS))
            self.assertTrue(all(command[2] == "--" for command in calls))

            def mutate(command, **kwargs):
                (python / "unexpected.pyc").write_bytes(b"cache")
                return mock.Mock(returncode=0)

            with (mock.patch.object(MERGER.runtime, "runtime_environment", return_value={}),
                  mock.patch.object(MERGER.subprocess, "run", side_effect=mutate)):
                with self.assertRaisesRegex(ValueError, "changed the bundled Python runtime"):
                    MERGER.qualify_qtgmc(runtime_root, root / "different-scratch")


if __name__ == "__main__":
    unittest.main()
