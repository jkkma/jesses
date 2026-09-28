"""Merge optional Vship into a verified QTGMC av1an source delivery.

Vship lives outside VapourSynth's autoload directory. The app admits it for an
individual job only after a contained, finite-frame Vulkan scorer probe.
"""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

sys.dont_write_bytecode = True
SCRIPTS = Path(__file__).resolve().parent
LOCK = SCRIPTS / "vship-windows-lock.json"
FRAMESERVER = Path("python/Lib/site-packages/vapoursynth")
OPTIONAL = FRAMESERVER / "optional-plugins/libvship_VULKAN.dll"


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


runtime = load("vship_av1an_runtime", "package-av1an-delivery.py")
scorer = load("vship_av1an_scorer", "add-package-av1an-scorers.py")
qtgmc = load("vship_av1an_qtgmc", "package-av1an-qtgmc.py")
support = runtime.support


def checked_vship(directory):
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    receipt = scorer.checked(directory, ("licenses", "buildInputs"), ("plugin", "source"))
    if (receipt.get("version"), receipt.get("sourceRevision")) != (lock["version"], lock["revision"]):
        raise ValueError("The Vship delivery differs from its pinned source revision.")
    if (receipt["plugin"]["id"] != "com.lumen.vship"
            or receipt["plugin"]["path"] != "plugins/" + lock["binary"]["path"]
            or receipt["plugin"]["sha256"] != lock["binary"]["sha256"]
            or receipt["source"]["path"] != "sources/" + lock["source"]["path"]
            or receipt["source"]["sha256"] != lock["source"]["sha256"]
            or [item["path"] for item in receipt["licenses"]] != ["licenses/Vship/LICENSE"]):
        raise ValueError("The Vship payloads differ from the reviewed binary and source pins.")
    expected_recipes = {"build/build-package-vship.py", "build/vship-windows-lock.json"}
    recipes = {record["path"] for record in receipt["buildInputs"]}
    if recipes != expected_recipes or any(
        support.digest(SCRIPTS / Path(path).name) != record["sha256"]
        for record in receipt["buildInputs"] for path in [record["path"]]
    ):
        raise ValueError("The Vship delivery has a different local build recipe.")
    return receipt


def optional_imports(binary, compiler):
    objdump = compiler / "ucrt64/bin/objdump.exe"
    output = subprocess.check_output([str(objdump), "-p", str(binary)], text=True, timeout=30)
    imports = sorted({line.split("DLL Name:", 1)[1].strip().lower()
                      for line in output.splitlines() if "DLL Name:" in line})
    if imports != ["kernel32.dll", "vulkan-1.dll"]:
        raise ValueError(f"Vship acquired an unreviewed native dependency: {imports}")
    return imports


def qualify_cpu(result, scratch, original_version):
    environment = runtime.runtime_environment(result)
    environment.pop("VAPOURSYNTH_EXTRA_PLUGIN_PATH", None)
    version = subprocess.run(
        [str(result / "av1an.exe"), "--version"], env=environment,
        cwd=scratch, capture_output=True, text=True, check=True, timeout=30,
    )
    text = version.stdout + version.stderr
    if (text.splitlines()[0] != original_version or "com.julek.vszip : Found" not in text
            or "com.julek.plugin : Found" not in text or "com.lumen.vship : Not found" not in text):
        raise ValueError("Optional Vship changed the default CPU av1an scorer path.")
    (scratch / "cpu-av1an-version.log").write_text(text, encoding="utf-8")
    qtgmc.qualify_scorers(result, scratch)


def qualify_optional_gpu(result, scratch):
    environment = runtime.runtime_environment(result)
    environment.pop("VAPOURSYNTH_EXTRA_PLUGIN_PATH", None)
    binary = result / OPTIONAL
    script = scratch / "vship-capability.vpy"
    script.write_text(
        "import math\nimport vapoursynth as vs\ncore = vs.core\n"
        f"core.std.LoadPlugin(path={str(binary)!r})\n"
        "reference = core.std.BlankClip(width=192, height=128, format=vs.YUV444P10, length=2, color=[256,512,512])\n"
        "distorted = core.std.BlankClip(reference, color=[264,520,520])\n"
        "for name, clip, prop in [\n"
        "    ('SSIMULACRA2', core.vship.SSIMULACRA2(reference, distorted, numStream=1), '_SSIMULACRA2'),\n"
        "    ('BUTTERAUGLI', core.vship.BUTTERAUGLI(reference, distorted, distmap=1, intensity_multiplier=203.0, numStream=1), '_BUTTERAUGLI_INFNorm'),\n"
        "]:\n"
        "    with clip.get_frame(0) as frame:\n"
        "        value = float(frame.props[prop])\n"
        "        assert math.isfinite(value), (name, value)\n"
        "        print('JESSES_VSHIP_OK', name, value)\n"
        "reference.set_output()\n",
        encoding="utf-8",
    )
    try:
        probe = subprocess.run(
            [str(result / FRAMESERVER / "vspipe.exe"), "--info", str(script), "-"],
            env=environment, cwd=scratch, capture_output=True, text=True, timeout=45,
        )
        diagnostic = probe.stdout + probe.stderr
        usable = (probe.returncode == 0 and "JESSES_VSHIP_OK SSIMULACRA2" in diagnostic
                  and "JESSES_VSHIP_OK BUTTERAUGLI" in diagnostic)
    except (OSError, subprocess.TimeoutExpired) as error:
        diagnostic, usable = str(error), False
    (scratch / "vship-capability.log").write_text(diagnostic, encoding="utf-8")
    if not usable:
        return False
    environment["VAPOURSYNTH_EXTRA_PLUGIN_PATH"] = str(binary.parent)
    version = subprocess.run(
        [str(result / "av1an.exe"), "--version"], env=environment,
        cwd=scratch, capture_output=True, text=True, check=True, timeout=30,
    )
    text = version.stdout + version.stderr
    (scratch / "vship-av1an-version.log").write_text(text, encoding="utf-8")
    if "com.lumen.vship : Found" not in text:
        raise ValueError("Vship passed GPU frames but av1an did not select it through the child-only path.")
    return True


def merge(base, vship, compiler, destination):
    original = scorer.checked(
        base, ("additionalSources", "licenses", "buildInputs", "supportFiles"), ("tool", "source")
    )
    if (original["tool"]["id"] != "av1an"
            or {item["id"] for item in original.get("cpuScorers", [])} != {"vszip", "julek"}
            or "qtgmc" not in original or "vship" in original):
        raise ValueError("Optional Vship requires the reviewed QTGMC and CPU scorer delivery.")
    extension = checked_vship(vship)
    imports = optional_imports(vship / extension["plugin"]["path"], compiler)
    destination.mkdir(parents=True, exist_ok=False)
    result = destination / "delivery"
    shutil.copytree(base, result)
    optional = result / OPTIONAL
    optional.parent.mkdir()
    shutil.copy2(vship / extension["plugin"]["path"], optional)
    original["supportFiles"].append({"path": OPTIONAL.as_posix(), "sha256": extension["plugin"]["sha256"]})
    for group in ("licenses", "buildInputs"):
        for record in extension[group]:
            relative = "vship/" + record["path"]
            output = result / relative
            output.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(vship / record["path"], output)
            original[group].append({**record, "path": relative})
    source = extension["source"]
    source_relative = "vship/" + source["path"]
    output = result / source_relative
    output.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(vship / source["path"], output)
    original["additionalSources"].append({**source, "path": source_relative})
    for file, relative in (
        (vship / "build-provenance.json", "vship/build-provenance.json"),
        (Path(__file__), "build/package-av1an-vship.py"),
    ):
        output = result / relative
        output.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(file, output)
        original["buildInputs"].append({"path": relative, "sha256": support.digest(output)})
    original["vship"] = {
        "version": extension["version"],
        "sourceRevision": extension["sourceRevision"],
        "runtimePath": OPTIONAL.as_posix(),
        "sha256": extension["plugin"]["sha256"],
        "activation": "VAPOURSYNTH_EXTRA_PLUGIN_PATH after a contained finite-frame probe",
        "optionalNativeImports": imports,
    }
    scratch = destination / "qualification"
    scratch.mkdir()
    before = support.inventory(result / "python")
    qualify_cpu(result, scratch, original["tool"]["version"])
    gpu = qualify_optional_gpu(result, scratch)
    if before != support.inventory(result / "python"):
        raise ValueError("Scorer qualification changed the packaged Python runtime.")
    (scratch / "summary.json").write_text(json.dumps({"cpu": "finite scorers passed", "gpu": "finite scorers passed and av1an selected Vship" if gpu else "unavailable on this host; CPU retained"}, indent=2) + "\n", encoding="utf-8")
    (result / "build-provenance.json").write_text(json.dumps(original, indent=2) + "\n", encoding="utf-8")
    scorer.checked(result, ("additionalSources", "licenses", "buildInputs", "supportFiles"), ("tool", "source"))
    print(f"Verified optional Vship and CPU fallback delivery: {result}; GPU here: {gpu}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--av1an-build", type=Path, required=True)
    parser.add_argument("--vship-build", type=Path, required=True)
    parser.add_argument("--msys-root", type=Path, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "win32":
        raise SystemExit("This native recipe targets Windows x64.")
    merge(args.av1an_build.resolve(strict=True), args.vship_build.resolve(strict=True),
          args.msys_root.resolve(strict=True), args.destination.absolute())


if __name__ == "__main__":
    main()
