"""Add verified source-built CPU scorers to a fresh portable av1an delivery."""

import argparse
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys

sys.dont_write_bytecode = True
SCRIPTS = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("av1an_delivery", SCRIPTS / "package-av1an-delivery.py")
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)
support = runtime.support


def checked(directory, groups, singular=()):
    path = directory / "build-provenance.json"
    receipt = json.loads(path.read_text(encoding="utf-8"))
    if receipt.get("schemaVersion") != 1 or receipt.get("target") != "x86_64-pc-windows-msvc":
        raise ValueError("A native Windows x64 source delivery is required.")
    expected = {"build-provenance.json": support.digest(path)}
    records = [receipt[key] for key in singular]
    records.extend(record for key in groups for record in receipt[key])
    for record in records:
        support.member_path(record["path"])
        if record["path"] in expected:
            raise ValueError("A source delivery repeats a payload.")
        expected[record["path"]] = record["sha256"]
    if expected != {record["path"]: record["sha256"] for record in support.inventory(directory)}:
        raise ValueError("The source delivery differs from its complete hash inventory.")
    return receipt


def merge(base, scorers, media, compiler, destination):
    receipt = checked(base, ("additionalSources", "licenses", "buildInputs", "supportFiles"), ("tool", "source"))
    plugins = checked(scorers, ("plugins", "additionalSources", "licenses", "buildInputs"))
    if receipt["tool"]["id"] != "av1an" or len(plugins["plugins"]) != 2 or {record["id"] for record in plugins["plugins"]} != {"vszip", "julek"}:
        raise ValueError("The scorer delivery must contain exactly the reviewed CPU plugin pair.")
    pins = json.loads((SCRIPTS / "package-av1an-scorers-lock.json").read_text(encoding="utf-8"))["inputs"]
    found = set()
    for record in plugins["additionalSources"]:
        name = record["component"]
        if name in found or name not in pins or record["sha256"] != pins[name]["sha256"]:
            raise ValueError("A scorer source differs from the reviewed source closure.")
        found.add(name)
    if found != set(pins) - {"zigBuild", "cmakeBuild"}:
        raise ValueError("The CPU scorer source closure is incomplete.")
    media_path = media / "build-provenance.json"
    if support.digest(media_path) != receipt["sharedMediaProvenanceSha256"]:
        raise ValueError("The base av1an package requires its exact media delivery.")
    ffmpeg = next(record for record in json.loads(media_path.read_text(encoding="utf-8"))["tools"] if record["id"] == "ffmpeg")
    if support.digest(media / ffmpeg["path"]) != ffmpeg["sha256"]:
        raise ValueError("The native qualification executable changed.")
    destination.mkdir(parents=True, exist_ok=False)
    result = destination / "delivery"
    shutil.copytree(base, result)
    for group in ("additionalSources", "licenses", "buildInputs"):
        for record in plugins[group]:
            relative = "scorers/" + record["path"]
            path = result / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(scorers / record["path"], path)
            receipt[group].append({**record, "path": relative})
    original_receipt = result / "scorers/build-provenance.json"
    shutil.copy2(scorers / "build-provenance.json", original_receipt)
    receipt["buildInputs"].append({"path": "scorers/build-provenance.json", "sha256": support.digest(original_receipt)})
    mapping = []
    names = set()
    for record in plugins["plugins"]:
        name = Path(record["path"]).name
        if not name.lower().endswith(".dll") or name.casefold() in names:
            raise ValueError("Each scorer must have a distinct native DLL name.")
        names.add(name.casefold())
        relative = "python/Lib/site-packages/vapoursynth/plugins/" + name
        path = result / relative
        if path.exists():
            raise ValueError("A scorer cannot replace a base frameserver dependency.")
        shutil.copy2(scorers / record["path"], path)
        receipt["supportFiles"].append({"path": relative, "sha256": record["sha256"]})
        mapping.append({"id": record["id"], "sourcePath": record["path"], "runtimePath": relative, "sha256": record["sha256"]})
    recipe = result / "build" / Path(__file__).name
    shutil.copy2(__file__, recipe)
    receipt["buildInputs"].append({"path": "build/" + recipe.name, "sha256": support.digest(recipe)})
    receipt["cpuScorers"] = mapping
    receipt["nativeDependencies"] = runtime.dependencies(result, compiler)
    receipt["tool"]["version"] = runtime.qualify(result, destination / "qualification", media / ffmpeg["path"])
    before = support.inventory(result / "python")
    environment = runtime.runtime_environment(result)
    code = """import math
import vapoursynth as vs
core = vs.core
reference = core.std.BlankClip(width=192, height=128, format=vs.YUV444P10, length=2, color=[256,512,512])
reference = core.std.SetFrameProps(reference, _Matrix=1, _Transfer=1, _Primaries=1, _ColorRange=1)
distorted = core.std.BlankClip(reference, color=[264,520,520])
rgb_reference = core.resize.Bicubic(reference, format=vs.RGBS, matrix_in_s='709')
rgb_distorted = core.resize.Bicubic(distorted, format=vs.RGBS, matrix_in_s='709')
for name, clip, props in [
    ('SSIMULACRA2', core.vszip.SSIMULACRA2(reference, distorted), ['SSIMULACRA2']),
    ('ButteraugliINF', core.julek.Butteraugli(rgb_reference, rgb_distorted, distmap=1, intensity_target=203.0), ['_FrameButteraugli']),
    ('XPSNR', core.vszip.XPSNR(reference, distorted), ['XPSNR_Y','XPSNR_U','XPSNR_V']),
]:
    with clip.get_frame(0) as frame:
        values = [float(frame.props[key]) for key in props]
        assert all(math.isfinite(value) for value in values), (name, values)
        print(name, values)
"""
    with (destination / "qualification/scorer-load.log").open("x", encoding="utf-8") as log:
        support.run([result / "python/python.exe", "-I", "-B", "-c", code], destination, environment, log, 30)
    if before != support.inventory(result / "python"):
        raise ValueError("Loading the CPU scorers changed the packaged runtime.")
    (result / "build-provenance.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    checked(result, ("additionalSources", "licenses", "buildInputs", "supportFiles"), ("tool", "source"))
    print(f"Verified portable av1an with CPU scorer sources: {result}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--av1an-build", type=Path, required=True)
    parser.add_argument("--scorers-build", type=Path, required=True)
    parser.add_argument("--ffmpeg-build", type=Path, required=True)
    parser.add_argument("--msys-root", type=Path, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    merge(args.av1an_build.resolve(strict=True), args.scorers_build.resolve(strict=True), args.ffmpeg_build.resolve(strict=True), args.msys_root.resolve(strict=True), args.destination.absolute())


if __name__ == "__main__":
    main()
