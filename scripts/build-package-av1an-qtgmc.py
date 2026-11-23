"""Build the pinned, source-complete Windows QTGMC dependency delivery.

The output is a small extension to the portable av1an/VapourSynth runtime.
Two C++ plugins are built from pinned source with the static MSVC runtime so
the portable frameserver does not depend on a machine-wide MSVCP140 install.
FFT3DFilter links to a static library built from the pinned FFTW source.
Other executable payloads come from identified upstream release archives.
"""

import argparse
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
from urllib.request import Request, urlopen
import zipfile


sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent
LOCK_PATH = ROOT / "qtgmc-windows-lock.json"
COMPILER_LOCK = ROOT / "package-av1an-windows-lock.json"
MAX_ARCHIVE = 250 * 1024 * 1024

spec = importlib.util.spec_from_file_location("qtgmc_package_support", ROOT / "package-av1an-support.py")
support = importlib.util.module_from_spec(spec)
spec.loader.exec_module(support)


def digest_bytes(data):
    return hashlib.sha256(data).hexdigest()


def fetched(url, expected, cache):
    if not url.startswith("https://") or len(expected) != 64:
        raise ValueError("QTGMC inputs must have HTTPS URLs and SHA256 pins.")
    path = cache / expected
    if path.exists():
        if not path.is_file() or path.is_symlink() or support.digest(path) != expected:
            raise ValueError(f"A cached QTGMC archive differs from its pin: {path}")
        return path
    request = Request(url, headers={"User-Agent": "jesses-qtgmc-source-build"})
    temporary = cache / (expected + ".partial")
    if temporary.exists():
        raise ValueError(f"A previous incomplete QTGMC download needs inspection: {temporary}")
    count = 0
    with urlopen(request, timeout=90) as response, temporary.open("xb") as output:
        while block := response.read(1024 * 1024):
            count += len(block)
            if count > MAX_ARCHIVE:
                raise ValueError(f"A QTGMC archive exceeds its size limit: {url}")
            output.write(block)
    if support.digest(temporary) != expected:
        raise ValueError(f"A QTGMC download differs from its SHA256 pin: {url}")
    temporary.rename(path)
    return path


def archive_member(archive, name, seven_zip):
    support.member_path(name)
    if zipfile.is_zipfile(archive):
        with zipfile.ZipFile(archive) as items:
            info = items.getinfo(name)
            if info.is_dir() or info.file_size > MAX_ARCHIVE:
                raise ValueError(f"Invalid QTGMC archive member: {name}")
            return items.read(info)
    result = subprocess.run(
        [seven_zip, "e", "-so", str(archive), name],
        capture_output=True,
        check=True,
        timeout=60,
    )
    if len(result.stdout) > MAX_ARCHIVE:
        raise ValueError(f"A QTGMC archive member exceeds its size limit: {name}")
    return result.stdout


def source_notice(component, source, destination):
    notices = support.source_notices(source, destination)
    if component == "znedi3":
        with zipfile.ZipFile(source) as archive:
            name = next(name for name in archive.namelist() if name.endswith("/gpl2.txt"))
            (destination / "GPL-2.0-upstream.txt").write_bytes(archive.read(name))
    elif component == "mvtools":
        with zipfile.ZipFile(source) as archive:
            name = next(name for name in archive.namelist() if name.endswith("/readme.rst"))
            (destination / "README-license-notice.rst").write_bytes(archive.read(name))
        # The tagged README identifies GPL 2 but does not carry its full text.
        (destination / "SOURCE-LICENSE-NOTE.txt").write_text(
            "The corresponding tagged source README identifies MVTools as GPL 2. "
            "The complete corresponding source is bundled in sources/mvtools-source.zip.\n",
            encoding="utf-8",
        )
    elif component == "mvtools-fftw":
        with tarfile.open(source) as archive:
            nested = archive.extractfile("mingw-w64-fftw/fftw-3.3.11.tar.gz").read()
        with tarfile.open(fileobj=io.BytesIO(nested)) as fftw:
            (destination / "COPYING").write_bytes(
                fftw.extractfile("fftw-3.3.11/COPYING").read()
            )
    elif component == "removegrainvs":
        with zipfile.ZipFile(source) as archive:
            name = next(name for name in archive.namelist() if name.endswith("/src/clense.cpp"))
            header = archive.read(name).split(b"*/", 1)[0] + b"*/\n"
            if b"Permission is hereby granted" not in header:
                raise ValueError("The pinned RemoveGrain source notice changed.")
            (destination / "LICENSE-from-clense.cpp.txt").write_bytes(header)
    if not any(destination.iterdir()):
        raise ValueError(f"The pinned {component} source has no redistributable notice.")
    return notices


def python_modules(inputs, fetched_inputs):
    havsfunc = inputs["havsfunc"]
    code = archive_member(
        fetched_inputs["havsfunc"][0], havsfunc["member"], None
    )
    if digest_bytes(code) != havsfunc["memberSha256"]:
        raise ValueError("The havsfunc source module differs from its upstream pin.")
    entries = {"havsfunc.py": code}
    wheel = fetched_inputs["vsutil"][0]
    with zipfile.ZipFile(wheel) as archive:
        for name in inputs["vsutil"]["members"]:
            entries[name] = archive.read(name)
    return entries


def deterministic_zip(path, entries):
    with zipfile.ZipFile(path, "w") as archive:
        for name, payload in sorted(entries.items()):
            support.member_path(name)
            member = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            member.compress_type = zipfile.ZIP_DEFLATED
            member.external_attr = 0o644 << 16
            archive.writestr(member, payload)


def build_static_plugins(destination, delivery, fetched_inputs, cmake_archive, ninja):
    workspace = destination / "source-build"
    workspace.mkdir()
    vs = support.unpack(fetched_inputs["vapoursynth"][1], workspace / "vapoursynth")
    fft = support.unpack(fetched_inputs["fft3dfilter"][1], workspace / "fft3dfilter")
    misc = support.unpack(fetched_inputs["miscfilters"][1], workspace / "miscfilters")
    fftw = support.unpack(fetched_inputs["fftw"][1], workspace / "fftw")
    cmake = support.unpack(cmake_archive, workspace / "cmake") / "bin/cmake.exe"
    env, cl, compiler = support.msvc_environment(destination, Path(sys.executable))
    fftw_build = workspace / "fftw-build"
    mapped = f'/experimental:deterministic "/pathmap:{workspace.as_posix()}=."'
    with (delivery / "build/fftw-static.log").open("x", encoding="utf-8") as log:
        support.run(
            [cmake, "-S", fftw, "-B", fftw_build, "-G", "Ninja",
             f"-DCMAKE_MAKE_PROGRAM={ninja.as_posix()}", f"-DCMAKE_C_COMPILER={cl.as_posix()}",
             f"-DCMAKE_C_FLAGS={mapped}",
             "-DCMAKE_BUILD_TYPE=Release", "-DCMAKE_POLICY_VERSION_MINIMUM=3.5",
             "-DCMAKE_POLICY_DEFAULT_CMP0091=NEW", "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded",
             "-DBUILD_TESTS=OFF", "-DBUILD_SHARED_LIBS=OFF", "-DENABLE_FLOAT=ON",
             "-DENABLE_SSE2=ON", "-DENABLE_THREADS=ON", "-DWITH_COMBINED_THREADS=ON"],
            workspace, env, log, 240,
        )
        support.run([cmake, "--build", fftw_build, "--parallel", "4"], workspace, env, log, 600)
    common = [
        cl, "/nologo", "/LD", "/MT", "/O2", "/EHsc", "/std:c++17",
        "/DWIN32", "/DNDEBUG", "/D_WINDOWS", "/D_USRDLL",
        "/I" + str(vs / "include"),
    ]
    configurations = (
        (
            "plugins/fft3dfilter.dll", "fft3dfilter", fft,
            ["/DFFT3DFILTER_EXPORTS", "/I" + str(fftw / "api")],
            [fft / name for name in (
                "FFT3DFilter.cpp", "FFT3DFilterTransform.cpp", "fft3dfilter_c.cpp", "Plugin.cpp"
            )],
            [fftw_build / "fftw3f.lib"],
        ),
        (
            "plugins/MiscFilters.dll", "miscfilters", misc,
            ["/DNOMINMAX", "/DVS_TARGET_CPU_X86", "/DVS_USE_LATEST_API", "/DMISCFILTERS_EXPORTS",
             "/I" + str(vs / "src/core")],
            [misc / "src/miscfilters.cpp"],
            [],
        ),
    )
    records = []
    for relative, component, _source, flags, sources, libraries in configurations:
        output = delivery / relative
        log_path = delivery / "build" / (component + "-static.log")
        with log_path.open("x", encoding="utf-8") as log:
            support.run(
                [*common, *flags, *sources, "/link", "/OUT:" + str(output), *libraries],
                workspace, env, log, 180,
            )
        records.append({
            "component": component,
            "path": relative,
            "sha256": support.digest(output),
            "runtime": "static-msvc-/MT",
            "log": "build/" + log_path.name,
            "logSha256": support.digest(log_path),
        })
    return compiler, records


def build(destination, cache, ninja):
    lock = json.loads(LOCK_PATH.read_text(encoding="utf-8"))
    if lock.get("schemaVersion") != 1 or lock.get("target") != "x86_64-pc-windows-msvc":
        raise ValueError("The QTGMC lock does not target Windows x64.")
    if sys.platform != "win32":
        raise ValueError("The QTGMC native delivery must be built and qualified on Windows.")
    seven_zip = shutil.which("7z")
    if not seven_zip:
        raise ValueError("7z is required to extract the pinned upstream release members.")
    if destination.exists():
        raise ValueError(f"The QTGMC destination already exists: {destination}")
    cache.mkdir(parents=True, exist_ok=True)
    destination.mkdir(parents=True)
    delivery = destination / "delivery"
    delivery.mkdir()
    for subdirectory in ("plugins", "sources", "licenses", "build"):
        (delivery / subdirectory).mkdir()

    inputs = lock["inputs"]
    cmake = json.loads(COMPILER_LOCK.read_text(encoding="utf-8"))["cmakeWindows"]
    cmake_archive = fetched(cmake["url"], cmake["sha256"], cache)
    source_built = set(lock.get("sourceBuiltOutputs", []))
    if source_built != {"plugins/fft3dfilter.dll", "plugins/MiscFilters.dll"}:
        raise ValueError("The QTGMC source-built plugin set changed.")
    fetched_inputs = {}
    for component, pin in inputs.items():
        binary = fetched(pin["url"], pin["sha256"], cache)
        source = fetched(pin["sourceUrl"], pin["sourceSha256"], cache)
        fetched_inputs[component] = (binary, source)

    deterministic_zip(delivery / "qtgmc-deps.zip", python_modules(inputs, fetched_inputs))
    plugins = []
    for component, pin in inputs.items():
        members = pin.get("members", {})
        if not isinstance(members, dict):
            continue
        for relative, member in members.items():
            support.member_path(relative)
            if not relative.startswith("plugins/"):
                raise ValueError(f"A QTGMC native destination is outside its delivery: {relative}")
            payload = archive_member(fetched_inputs[component][0], member["member"], seven_zip)
            if digest_bytes(payload) != member["sha256"]:
                raise ValueError(f"A QTGMC release member differs from its pin: {component}/{relative}")
            if relative in source_built:
                continue
            output = delivery / relative
            output.write_bytes(payload)
            plugins.append({"id": output.stem, "path": relative, "sha256": support.digest(output)})
    compiler, source_builds = build_static_plugins(destination, delivery, fetched_inputs, cmake_archive, ninja)
    plugins.extend({
        "id": Path(record["path"]).stem,
        "path": record["path"],
        "sha256": record["sha256"],
    } for record in source_builds)
    outputs = {"qtgmc-deps.zip": support.digest(delivery / "qtgmc-deps.zip")}
    outputs.update({record["path"]: record["sha256"] for record in plugins})
    if {key: value for key, value in outputs.items() if key not in source_built} != lock["outputs"]:
        raise ValueError("The built QTGMC payload differs from the pinned output set or hashes.")

    additional_sources = []
    for component, pin in inputs.items():
        source = fetched_inputs[component][1]
        if zipfile.is_zipfile(source):
            suffix = ".zip"
        else:
            suffix = ".tar.zst" if pin["sourceUrl"].endswith(".tar.zst") else ".tar.gz"
        relative = f"sources/{component}-source{suffix}"
        shutil.copy2(source, delivery / relative)
        additional_sources.append({
            "component": component,
            "path": relative,
            "sha256": pin["sourceSha256"],
            "url": pin["sourceUrl"],
            "revision": pin["revision"],
        })
        source_notice(component, source, delivery / "licenses" / component)

    for component, pin in inputs.items():
        # A source archive also serving as the exact binary release is already
        # preserved above. Other upstream binary archives remain build inputs.
        if pin["sha256"] == pin["sourceSha256"]:
            continue
        binary = fetched_inputs[component][0]
        suffix = ".zip" if zipfile.is_zipfile(binary) else ".7z"
        shutil.copy2(binary, delivery / "build" / f"{component}-release{suffix}")
    shutil.copy2(Path(__file__), delivery / "build" / Path(__file__).name)
    shutil.copy2(LOCK_PATH, delivery / "build" / LOCK_PATH.name)
    shutil.copy2(COMPILER_LOCK, delivery / "build" / COMPILER_LOCK.name)
    receipt = {
        "schemaVersion": 1,
        "target": "x86_64-pc-windows-msvc",
        "pythonModules": [{"path": "qtgmc-deps.zip", "sha256": outputs["qtgmc-deps.zip"]}],
        "plugins": sorted(plugins, key=lambda record: record["path"]),
        "additionalSources": sorted(additional_sources, key=lambda record: record["component"]),
        "licenses": [
            {**record, "path": "licenses/" + record["path"]}
            for record in support.inventory(delivery / "licenses")
        ],
        "buildInputs": [
            {**record, "path": "build/" + record["path"]}
            for record in support.inventory(delivery / "build")
        ],
        "compiler": compiler,
        "sourceBuild": source_builds,
        "qualification": "Pinned QTGMC Python/native payloads; all six supported presets are frame-tested when merged into the portable Av1an runtime.",
    }
    (delivery / "build-provenance.json").write_text(
        json.dumps(receipt, indent=2) + "\n", encoding="utf-8"
    )
    expected = {"build-provenance.json": support.digest(delivery / "build-provenance.json")}
    for group in ("pythonModules", "plugins", "additionalSources", "licenses", "buildInputs"):
        for record in receipt[group]:
            if record["path"] in expected:
                raise ValueError("The QTGMC receipt repeats a payload.")
            expected[record["path"]] = record["sha256"]
    actual = {record["path"]: record["sha256"] for record in support.inventory(delivery)}
    if expected != actual:
        raise ValueError("The complete QTGMC source delivery differs from its receipt.")
    print(f"Verified source-complete QTGMC delivery: {delivery}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--cache", type=Path, default=Path("target/qtgmc-archive-cache"))
    parser.add_argument("--ninja", type=Path, help="Path to the Ninja build executable")
    args = parser.parse_args()
    ninja = args.ninja or shutil.which("ninja")
    if not ninja:
        parser.error("Ninja is required to build the pinned single-precision FFTW library.")
    build(args.destination.absolute(), args.cache.absolute(), Path(ninja).resolve(strict=True))


if __name__ == "__main__":
    main()
