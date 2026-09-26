"""Merge a pinned QTGMC closure into a fresh portable Windows av1an delivery."""

import argparse
import importlib.util
import json
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import zipfile


sys.dont_write_bytecode = True
SCRIPTS = Path(__file__).resolve().parent
LOCK = SCRIPTS / "qtgmc-windows-lock.json"
FRAMESERVER = Path("python/Lib/site-packages/vapoursynth")
PYTHON_ZIP = Path("python/qtgmc-deps.zip")
PLUGIN_FILES = {
    "eedi3m.dll", "fft3dfilter.dll", "fmtconv.dll", "libtemporalsoften2.dll",
    "miscfilters.dll", "mvtools.dll", "nnedi3_weights.bin",
    "removegrainvs.dll", "znedi3.dll",
}
RUNTIME_FILES = {"libfftw3f-3.dll"}
SOURCE_BUILT_OUTPUTS = {"plugins/fft3dfilter.dll", "plugins/MiscFilters.dll"}
REQUIRED_MODULES = {"havsfunc.py", "vsutil/__init__.py"}
PRESETS = ("Faster", "Fast", "Medium", "Slow", "Slower", "Very Slow")


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


runtime = load("qtgmc_av1an_runtime", "package-av1an-delivery.py")
scorer = load("qtgmc_av1an_scorer", "add-package-av1an-scorers.py")
support = runtime.support


def checked_python_zip(path):
    """Only the reviewed pure-Python modules may enter the isolated interpreter."""
    names = set()
    with zipfile.ZipFile(path) as archive:
        members = archive.infolist()
        if not members or len(members) > 64 or sum(item.file_size for item in members) > 4 * 1024 * 1024:
            raise ValueError("The QTGMC Python module archive exceeds its bounds.")
        for item in members:
            name = support.member_path(item.filename).as_posix()
            folded = name.casefold()
            if (item.is_dir() or stat.S_ISLNK(item.external_attr >> 16) or
                    folded in names or item.file_size > 1024 * 1024):
                raise ValueError("The QTGMC Python module archive has duplicate or invalid entries.")
            if not (name == "havsfunc.py" or
                    (name.startswith("vsutil/") and Path(name).suffix in {".py", ".typed"})):
                raise ValueError(f"Unexpected QTGMC Python module: {name}")
            if "__pycache__" in name or name.endswith((".pyc", ".pyo")):
                raise ValueError("QTGMC Python bytecode cannot be bundled.")
            names.add(folded)
    if not {name.casefold() for name in REQUIRED_MODULES}.issubset(names):
        raise ValueError("The QTGMC Python dependency archive is incomplete.")


def checked_qtgmc_delivery(directory, lock):
    if lock.get("schemaVersion") != 1 or lock.get("target") != "x86_64-pc-windows-msvc":
        raise ValueError("The QTGMC lock has an unsupported identity or platform.")
    receipt = scorer.checked(
        directory, ("pythonModules", "plugins", "additionalSources", "licenses", "buildInputs")
    )
    if len(receipt["pythonModules"]) != 1 or Path(receipt["pythonModules"][0]["path"]).name != "qtgmc-deps.zip":
        raise ValueError("The QTGMC delivery must contain one pinned Python dependency archive.")
    checked_python_zip(directory / receipt["pythonModules"][0]["path"])
    names = {Path(record["path"]).name.casefold() for record in receipt["plugins"]}
    ids = {record["id"] for record in receipt["plugins"]}
    if (names != PLUGIN_FILES | RUNTIME_FILES or len(receipt["plugins"]) != len(names)
            or len(ids) != len(names)):
        raise ValueError("The QTGMC native plugin closure differs from the reviewed set.")
    for record in receipt["plugins"]:
        path = Path(record["path"])
        if path.name.casefold() in PLUGIN_FILES and path.parent.as_posix() != "plugins":
            raise ValueError("A QTGMC plugin is outside its source delivery plugin directory.")
        if path.name.casefold() in RUNTIME_FILES and path.parent.as_posix() != "runtime":
            raise ValueError("A QTGMC runtime library is outside its source delivery runtime directory.")
        payload = (directory / record["path"]).read_bytes().lower()
        # Official release DLLs can retain their upstream builder's source
        # path. Reject this checkout and host's paths without mistaking that
        # upstream provenance for a local workspace leak.
        local_paths = (Path.home(), SCRIPTS.parent, directory)
        if any(encoded.lower() in payload for path in local_paths
               for encoded in (str(path).encode("utf-8"), str(path).encode("utf-16le"))):
            raise ValueError("A QTGMC native dependency embeds a local build path.")
    pins = lock["inputs"]
    sources = {record["component"]: record["sha256"] for record in receipt["additionalSources"]}
    if len(sources) != len(receipt["additionalSources"]) or sources != {
        name: record["sourceSha256"] for name, record in pins.items()
    }:
        raise ValueError("The QTGMC delivery differs from its pinned source closure.")
    if not receipt["licenses"] or not receipt["buildInputs"]:
        raise ValueError("The QTGMC delivery has no source notices or build recipe.")
    notices = [record["path"] for record in receipt["licenses"]]
    if any(not any(path.startswith(f"licenses/{name}/") for path in notices) for name in pins):
        raise ValueError("The QTGMC source notices are incomplete.")
    build_names = {Path(record["path"]).name for record in receipt["buildInputs"]}
    if not {"build-package-av1an-qtgmc.py", LOCK.name}.issubset(build_names):
        raise ValueError("The QTGMC source delivery omitted its pinned build recipe.")
    outputs = {
        record["path"]: record["sha256"]
        for group in ("pythonModules", "plugins") for record in receipt[group]
    }
    dynamic = set(lock.get("sourceBuiltOutputs", []))
    if dynamic != SOURCE_BUILT_OUTPUTS or not dynamic.issubset(outputs):
        raise ValueError("The QTGMC source-built output set differs from the reviewed closure.")
    if (len(outputs) != len(receipt["pythonModules"]) + len(receipt["plugins"])
            or {path: sha for path, sha in outputs.items() if path not in dynamic} != lock["outputs"]):
        raise ValueError("The QTGMC runtime payloads differ from the pinned output hashes.")
    compiler = receipt.get("compiler", {})
    if not all(compiler.get(key) for key in ("compilerSha256", "msvcVersion", "sdkVersion")):
        raise ValueError("The QTGMC source-built DLLs lack compiler and build provenance.")
    builds = receipt.get("sourceBuild", [])
    components = {
        "plugins/fft3dfilter.dll": "fft3dfilter",
        "plugins/MiscFilters.dll": "miscfilters",
    }
    build_inputs = {record["path"]: record["sha256"] for record in receipt["buildInputs"]}
    if len(build_inputs) != len(receipt["buildInputs"]) or len(builds) != len(dynamic):
        raise ValueError("The QTGMC static build receipt is incomplete or ambiguous.")
    seen = set()
    logs = set()
    for build in builds:
        path = build.get("path")
        log = build.get("log")
        if (not isinstance(path, str) or path not in dynamic or path in seen
                or not isinstance(log, str) or log in logs or log not in build_inputs
                or build.get("component") != components[path]
                or build.get("component") not in pins
                or build.get("sha256") != outputs[path]
                or build.get("runtime") != "static-msvc-/MT"
                or not log.startswith("build/")
                or not build.get("logSha256")
                or build.get("logSha256") != build_inputs.get(log)):
            raise ValueError("A QTGMC static DLL is not bound to its source, hash, and build log.")
        seen.add(path)
        logs.add(log)
    if seen != dynamic:
        raise ValueError("The QTGMC source-built DLL receipt omits an output.")
    return receipt


def checked_local_recipe(receipt):
    for name in ("build-package-av1an-qtgmc.py", LOCK.name):
        records = [record for record in receipt["buildInputs"] if Path(record["path"]).name == name]
        if len(records) != 1 or records[0]["sha256"] != support.digest(SCRIPTS / name):
            raise ValueError(f"The QTGMC source delivery has a different build input: {name}")


def replace_support_hash(receipt, path, sha256):
    records = [record for record in receipt["supportFiles"] if record["path"] == path]
    if len(records) != 1:
        raise ValueError(f"The av1an runtime has no unique support file: {path}")
    records[0]["sha256"] = sha256


def install_runtime(base, qtgmc, receipt, extension, result):
    shutil.copytree(base, result)
    destination_zip = result / PYTHON_ZIP
    if destination_zip.exists():
        raise ValueError("The av1an runtime already contains QTGMC Python modules.")
    shutil.copy2(qtgmc / extension["pythonModules"][0]["path"], destination_zip)
    receipt["supportFiles"].append({"path": PYTHON_ZIP.as_posix(), "sha256": support.digest(destination_zip)})
    pth = result / "python/python314._pth"
    if pth.read_text(encoding="utf-8").splitlines() != ["python314.zip", ".", "Lib/site-packages"]:
        raise ValueError("The pinned embedded Python search path changed.")
    pth.write_text("python314.zip\n.\nLib/site-packages\nqtgmc-deps.zip\n", encoding="utf-8")
    replace_support_hash(receipt, "python/python314._pth", support.digest(pth))
    mapping = []
    for record in extension["plugins"]:
        name = Path(record["path"]).name
        # VapourSynth autoloads plugin DLLs with Windows LoadLibrary semantics;
        # FFTW must sit beside FFT3DFilter for its dependent import to resolve.
        relative = FRAMESERVER / "plugins" / name
        output = result / relative
        if output.exists():
            raise ValueError("A QTGMC dependency cannot replace an existing frameserver file.")
        shutil.copy2(qtgmc / record["path"], output)
        item = {"path": relative.as_posix(), "sha256": record["sha256"]}
        receipt["supportFiles"].append(item)
        mapping.append({"id": record["id"], **item})
    for group in ("additionalSources", "licenses", "buildInputs"):
        for record in extension[group]:
            relative = "qtgmc/" + record["path"]
            output = result / relative
            output.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(qtgmc / record["path"], output)
            receipt[group].append({**record, "path": relative})
    for source, relative in (
        (qtgmc / "build-provenance.json", "qtgmc/build-provenance.json"),
        (Path(__file__), "build/package-av1an-qtgmc.py"),
        (LOCK, "build/qtgmc-windows-lock.json"),
    ):
        output = result / relative
        output.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, output)
        receipt["buildInputs"].append({"path": relative, "sha256": support.digest(output)})
    receipt["qtgmc"] = {"pythonModules": PYTHON_ZIP.as_posix(), "nativeDependencies": mapping}


def qualify_qtgmc(result, scratch):
    """Evaluate real frames in the same isolated VSPipe used by the app."""
    scratch.mkdir(parents=True, exist_ok=True)
    before = support.inventory(result / "python")
    vspipe = result / FRAMESERVER / "vspipe.exe"
    environment = runtime.runtime_environment(result)
    with (scratch / "qtgmc-probe.log").open("x", encoding="utf-8") as log:
        for preset in PRESETS:
            label = preset.replace(" ", "-").lower()
            script = scratch / f"qtgmc-{label}.vpy"
            script.write_text(
                "import vapoursynth as vs\nimport havsfunc\n"
                "clip = vs.core.std.BlankClip(width=192, height=128, length=8, "
                "format=vs.YUV420P8, fpsnum=30000, fpsden=1001)\n"
                "clip = vs.core.std.SetFrameProps(clip, _FieldBased=2)\n"
                f"clip = havsfunc.QTGMC(clip, Preset={preset!r}, TFF=True, FPSDivisor=1)\n"
                "assert clip.num_frames == 16 and clip.fps_num == 60000 and clip.fps_den == 1001\n"
                "with clip.get_frame(0) as frame:\n"
                "    assert frame.width == 192 and frame.height == 128\n"
                "clip.set_output()\n",
                encoding="utf-8",
            )
            log.write(f"Probe preset: {preset}\n")
            log.flush()
            subprocess.run(
                [str(vspipe), str(script), "--"], cwd=scratch, env=environment,
                stdout=subprocess.DEVNULL, stderr=log, check=True, timeout=180,
            )
    if before != support.inventory(result / "python"):
        raise ValueError("QTGMC execution changed the bundled Python runtime.")


def qualify_scorers(result, scratch):
    """QTGMC's additional plugins must preserve the original three CPU metrics."""
    code = """import math
import vapoursynth as vs
core = vs.core
reference = core.std.BlankClip(width=192, height=128, format=vs.YUV444P10, length=2, color=[256,512,512])
reference = core.std.SetFrameProps(reference, _Matrix=1, _Transfer=1, _Primaries=1, _ColorRange=1)
distorted = core.std.BlankClip(reference, color=[264,520,520])
rgb_reference = core.resize.Bicubic(reference, format=vs.RGBS, matrix_in_s='709')
rgb_distorted = core.resize.Bicubic(distorted, format=vs.RGBS, matrix_in_s='709')
for clip, props in [
    (core.vszip.SSIMULACRA2(reference, distorted), ['SSIMULACRA2']),
    (core.julek.Butteraugli(rgb_reference, rgb_distorted, distmap=1, intensity_target=203.0), ['_FrameButteraugli']),
    (core.vszip.XPSNR(reference, distorted), ['XPSNR_Y','XPSNR_U','XPSNR_V']),
]:
    with clip.get_frame(0) as frame:
        assert all(math.isfinite(float(frame.props[key])) for key in props)
"""
    before = support.inventory(result / "python")
    with (scratch / "qtgmc-scorer-regression.log").open("x", encoding="utf-8") as log:
        support.run(
            [result / "python/python.exe", "-I", "-B", "-c", code], scratch,
            runtime.runtime_environment(result), log, 60,
        )
    if before != support.inventory(result / "python"):
        raise ValueError("CPU scorer execution changed the bundled Python runtime.")


def merge(base, qtgmc, media, compiler, destination):
    original = scorer.checked(
        base, ("additionalSources", "licenses", "buildInputs", "supportFiles"), ("tool", "source")
    )
    if original["tool"]["id"] != "av1an" or {item["id"] for item in original.get("cpuScorers", [])} != {"vszip", "julek"}:
        raise ValueError("QTGMC requires the reviewed scorer-enabled av1an delivery.")
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    extension = checked_qtgmc_delivery(qtgmc, lock)
    checked_local_recipe(extension)
    media_path = media / "build-provenance.json"
    if support.digest(media_path) != original["sharedMediaProvenanceSha256"]:
        raise ValueError("The QTGMC merger requires av1an's exact media delivery.")
    ffmpeg = next(item for item in json.loads(media_path.read_text(encoding="utf-8"))["tools"] if item["id"] == "ffmpeg")
    if support.digest(media / ffmpeg["path"]) != ffmpeg["sha256"]:
        raise ValueError("The native qualification executable changed.")
    destination.mkdir(parents=True, exist_ok=False)
    result = destination / "delivery"
    install_runtime(base, qtgmc, original, extension, result)
    original["nativeDependencies"] = runtime.dependencies(result, compiler)
    original["tool"]["version"] = runtime.qualify(result, destination / "qualification", media / ffmpeg["path"])
    qualify_qtgmc(result, destination / "qualification")
    qualify_scorers(result, destination / "qualification")
    (result / "build-provenance.json").write_text(json.dumps(original, indent=2) + "\n", encoding="utf-8")
    scorer.checked(result, ("additionalSources", "licenses", "buildInputs", "supportFiles"), ("tool", "source"))
    print(f"Verified portable av1an with QTGMC sources: {result}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--av1an-build", type=Path, required=True)
    parser.add_argument("--qtgmc-build", type=Path, required=True)
    parser.add_argument("--ffmpeg-build", type=Path, required=True)
    parser.add_argument("--msys-root", type=Path, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "win32":
        raise ValueError("This native recipe targets Windows x64.")
    merge(
        args.av1an_build.resolve(strict=True), args.qtgmc_build.resolve(strict=True),
        args.ffmpeg_build.resolve(strict=True), args.msys_root.resolve(strict=True),
        args.destination.absolute(),
    )


if __name__ == "__main__":
    main()
