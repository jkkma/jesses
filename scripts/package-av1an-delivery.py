"""Assemble and qualify the private portable runtime built by av1an's recipe."""

import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("av1an_support", Path(__file__).with_name("package-av1an-support.py"))
support = importlib.util.module_from_spec(spec)
spec.loader.exec_module(support)

SHARED_BASES = {"mingw-w64-gcc", "mingw-w64-crt", "mingw-w64-headers", "mingw-w64-winpthreads", "mingw-w64-dav1d", "mingw-w64-libvpx", "mingw-w64-zlib"}
SYSTEM_DLLS = {"advapi32.dll", "bcrypt.dll", "bcryptprimitives.dll", "comctl32.dll", "crypt32.dll", "dbghelp.dll", "gdi32.dll", "imagehlp.dll", "imm32.dll", "iphlpapi.dll", "kernel32.dll", "msvcrt.dll", "ncrypt.dll", "netapi32.dll", "normaliz.dll", "ntdll.dll", "ole32.dll", "oleaut32.dll", "powrprof.dll", "psapi.dll", "rpcrt4.dll", "secur32.dll", "setupapi.dll", "shell32.dll", "shlwapi.dll", "user32.dll", "userenv.dll", "ucrtbase.dll", "version.dll", "winhttp.dll", "winmm.dll", "wintrust.dll", "ws2_32.dll", "wtsapi32.dll"}
SYSTEM_DLLS.update(("pdh.dll", "propsys.dll"))


def dependencies(directory, compiler):
    native = [path for path in directory.rglob("*") if path.suffix.lower() in {".exe", ".dll", ".pyd"}]
    bundled = {path.name.lower() for path in native}
    records = []
    for path in sorted(native):
        output = subprocess.check_output([str(compiler / "ucrt64/bin/objdump.exe"), "-p", str(path)], text=True, timeout=30)
        imports = sorted({line.split("DLL Name:", 1)[1].strip() for line in output.splitlines() if "DLL Name:" in line})
        if any(name.lower() not in SYSTEM_DLLS | bundled and not name.lower().startswith(("api-ms-win-", "ext-ms-win-")) for name in imports):
            raise ValueError(f"Unpackaged runtime dependency in {path.name}: {imports}")
        records.append({"path": path.relative_to(directory).as_posix(), "imports": imports})
    return records


def runtime_environment(directory):
    env = support.isolated_environment()
    frameserver = directory / "python/Lib/site-packages/vapoursynth"
    env["PATH"] = str(frameserver) + os.pathsep + str(support.system32())
    env["VSSCRIPT_PATH"] = str(frameserver / "vsscript.dll")
    return env


def stage(directory, tools_root, target, inventory):
    provenance_path = directory / "build-provenance.json"
    receipt = json.loads(provenance_path.read_text(encoding="utf-8"))
    if target != "x86_64-pc-windows-msvc" or receipt.get("schemaVersion") != 1 or receipt.get("target") != target or receipt["tool"]["id"] != "av1an":
        raise ValueError("The portable av1an delivery has an unsupported identity or platform.")
    if (receipt["sourceCommit"], receipt["source"]["sha256"]) != ("805dad69143fa0a81cfe2fb89c0b9e90a828ea72", "7f570da8fe0ba5970cbf04d882b48e442e04d1df5ec180e12d9ff974450dfca2"):
        raise ValueError("The portable av1an source differs from the reviewed revision.")
    media_path = tools_root / "ffmpeg/build-provenance.json"
    if not media_path.is_file() or support.digest(media_path) != receipt["sharedMediaProvenanceSha256"]:
        raise ValueError("Portable av1an requires its exact shared media source delivery.")
    media = json.loads(media_path.read_text(encoding="utf-8"))
    shared = receipt["sharedSources"]
    if len(shared) != len(SHARED_BASES) or {record["base"] for record in shared} != SHARED_BASES or any(record not in media["additionalSources"] for record in shared):
        raise ValueError("The portable decoder's shared source closure is incomplete.")
    actual = {record["path"]: record["sha256"] for record in inventory(directory)}
    expected = {"build-provenance.json": support.digest(provenance_path)}
    for record in [receipt["tool"], receipt["source"], *receipt["additionalSources"], *receipt["licenses"], *receipt["buildInputs"], *receipt["supportFiles"]]:
        support.member_path(record["path"])
        if record["path"] in expected:
            raise ValueError("The portable av1an receipt repeats a payload.")
        expected[record["path"]] = record["sha256"]
    if not receipt["licenses"] or not receipt["buildInputs"] or not receipt["additionalSources"] or actual != expected:
        raise ValueError("The portable av1an inventory differs from its source/build receipt.")
    required = {"python/python.exe", "python/python314.dll", "python/python314.zip", "python/python314._pth", "python/Lib/site-packages/vapoursynth/__init__.pyc", "python/Lib/site-packages/vapoursynth/vsscript.dll", "python/Lib/site-packages/vapoursynth/vspipe.exe", "python/Lib/site-packages/vapoursynth/libvapoursynth.dll", "python/Lib/site-packages/vapoursynth/plugins/LSMASHSource.dll"}
    if not required.issubset({record["path"] for record in receipt["supportFiles"]}):
        raise ValueError("The portable frameserver's required runtime files are missing.")
    result = subprocess.run([str(directory / receipt["tool"]["path"]), "--version"], env=runtime_environment(directory), capture_output=True, text=True, check=True, timeout=30)
    version = result.stdout + result.stderr
    if version.splitlines()[0] != receipt["tool"]["version"] or "[ffmpeg9-passthrough-v1]" not in version or "[target-probe-filter-v1]" not in version or "systems.innocent.lsmas : Found" not in version:
        raise ValueError("The portable av1an identity or decoder availability changed.")
    if actual != {record["path"]: record["sha256"] for record in inventory(directory)}:
        raise ValueError("Av1an execution changed its delivery payload.")
    shutil.copytree(directory, tools_root / "av1an")
    def prefix(record, name="av1an"):
        return {**record, "path": name + "/" + record["path"]}
    notices = [record for record in media["licenses"] if any(record["path"].startswith(f"licenses/{base}/") for base in SHARED_BASES)]
    return {**prefix(receipt["tool"]), "sourceCommit": receipt["sourceCommit"], "source": prefix(receipt["source"]), "additionalSources": [*[prefix(record) for record in receipt["additionalSources"]], *[prefix(record, "ffmpeg") for record in shared]], "licenses": [*[prefix(record) for record in receipt["licenses"]], *[prefix(record, "ffmpeg") for record in notices]], "buildInputs": [*[prefix(record) for record in receipt["buildInputs"]], *[prefix(record, "ffmpeg") for record in media["buildInputs"]]], "supportFiles": [prefix(record) for record in receipt["supportFiles"]], "buildProvenance": {"path": "av1an/build-provenance.json", "sha256": support.digest(provenance_path)}}


def qualify(directory, scratch, ffmpeg):
    scratch.mkdir()
    env = runtime_environment(directory)
    before = support.inventory(directory)
    source = scratch / "decoder-fixture.mkv"
    with (scratch / "runtime-gate.log").open("x", encoding="utf-8") as log:
        support.run([ffmpeg, "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=64x64:rate=3:duration=1", "-c:v", "ffv1", source], scratch, env, log, 30)
        code = "import vapoursynth as vs; clip=vs.core.lsmas.LWLibavSource(source=" + repr(str(source)) + ", cache=0); assert clip.num_frames == 3; assert clip.get_frame(2).width == 64; print(str(vs.core)); print('Portable L-SMASH decoded three frames')"
        support.run([directory / "python/python.exe", "-I", "-B", "-c", code], scratch, env, log, 60)
        result = subprocess.run([str(directory / "av1an.exe"), "--version"], cwd=scratch, env=env, capture_output=True, text=True, check=True, timeout=30)
        version = result.stdout + result.stderr
        log.write(version)
        if "[ffmpeg9-passthrough-v1]" not in version or "[target-probe-filter-v1]" not in version or "systems.innocent.lsmas : Not found" in version or "systems.innocent.lsmas" not in version:
            raise ValueError("The final av1an runtime lacks its compatibility patch or decoder plugin.")
    if before != support.inventory(directory):
        raise ValueError("Running the portable frameserver changed its installed payload.")
    return version.splitlines()[0]


def finish(destination, media, compiler, provenance, lock, archives, python_sources, frameserver, decoder, plugin, engine, rust_version, scripts):
    directory = destination / "delivery"
    directory.mkdir()
    shutil.copy2(engine["binary"], directory / "av1an.exe")
    python = support.unpack(archives["pythonRuntime"], directory / "python", single_root=False)
    (python / "python314._pth").write_text("python314.zip\n.\nLib/site-packages\n", encoding="utf-8")
    package = python / "Lib/site-packages/vapoursynth"
    package.mkdir(parents=True)
    for name in ["libvapoursynth.dll", "libvapoursynthfilters.dll", "libvapoursynthfilters_avx2.dll", "libvapoursynthfilters_zn4.dll", "vsscript.dll", "vspipe.exe", "vapoursynth.pyd"]:
        shutil.copy2(frameserver["build"] / name, package / name)
    python_source = frameserver["source"] / "src/py"
    for path in python_source.iterdir():
        if path.suffix in {".py", ".pyi"} and path.name != "__init__.py":
            shutil.copy2(path, package / path.name)
    (package / "plugins").mkdir()
    shutil.copy2(plugin["binary"], package / "plugins/LSMASHSource.dll")
    build = directory / "build"
    build.mkdir()
    # A sourceless initializer prevents VSScript from writing its own cache before
    # it can set dont_write_bytecode. The exact readable initializer ships too.
    init = build / "vapoursynth-package-init.py"
    init.write_text("import sys\nsys.dont_write_bytecode = True\n" + (python_source / "__init__.py").read_text(encoding="utf-8"), encoding="utf-8")
    command = "import py_compile; py_compile.compile(" + repr(str(init)) + ", cfile=" + repr(str(package / "__init__.pyc")) + ", dfile='vapoursynth/__init__.py', doraise=True, invalidation_mode=py_compile.PycInvalidationMode.CHECKED_HASH)"
    subprocess.run([str(python / "python.exe"), "-I", "-B", "-c", command], env=runtime_environment(directory), check=True, timeout=30)
    imports = dependencies(directory, compiler)
    version = qualify(directory, destination / "qualification", media / "ffmpeg.exe")
    sources_dir = directory / "sources"
    sources_dir.mkdir()
    additional = []
    notices_dir = directory / "licenses"
    notices_dir.mkdir()
    for name, original in archives.items():
        record = lock["inputs"][name]
        if name in {"pythonRuntime", "pythonBuild", "pythonSbom", "diffutilsBuild"}:
            continue
        output = sources_dir / Path(record["path"]).name
        shutil.copy2(original, output)
        additional.append({**record, "component": name, "path": output.relative_to(directory).as_posix()})
        support.source_notices(original, notices_dir / name)
    for index, (record, original) in enumerate(python_sources):
        name = f"python-dependency-{index:02d}-{Path(record['path']).name}"
        shutil.copy2(original, sources_dir / name)
        additional.append({**record, "path": "sources/" + name})
        support.source_notices(original, notices_dir / f"python-dependency-{index:02d}")
    vendor = sources_dir / "av1an-cargo-vendor.tar.gz"
    support.archive_directory(engine["source"] / "vendor", vendor)
    additional.append({"path": "sources/" + vendor.name, "sha256": support.digest(vendor), "description": "Complete Cargo.lock dependencies with Cargo checksum metadata; build resolves this tree offline."})
    support.source_notices(vendor, notices_dir / "cargo")
    for name in ["build-package-av1an.py", "package-av1an-support.py", "package-av1an-components.py", "package-av1an-delivery.py", "patch-package-av1an.py", "package-av1an-windows-lock.json"]:
        shutil.copy2(scripts / name, build / name)
    shutil.copy2(engine["patch"], build / "ffmpeg9-passthrough.patch")
    shutil.copy2(archives["pythonSbom"], build / "python-runtime.spdx.json")
    # Include the exact embedded runtime license in the notice index as well as
    # at its original runtime path.
    shutil.copy2(python / "LICENSE.txt", notices_dir / "Python-runtime-LICENSE.txt")
    shared = [record for record in provenance["additionalSources"] if record["base"] in SHARED_BASES]
    if len(shared) != len(SHARED_BASES):
        raise ValueError("The decoder's shared static library sources are incomplete.")
    source = next(record for record in additional if record.get("component") == "av1an")
    additional.remove(source)
    receipt = {"schemaVersion": 1, "target": "x86_64-pc-windows-msvc", "tool": {"id": "av1an", "path": "av1an.exe", "sha256": support.digest(directory / "av1an.exe"), "version": version}, "sourceCommit": lock["inputs"]["av1an"]["commit"], "source": source, "additionalSources": additional, "sharedSources": shared, "sharedMediaProvenanceSha256": support.digest(media / "build-provenance.json"), "licenses": [{**record, "path": "licenses/" + record["path"]} for record in support.inventory(notices_dir)], "buildInputs": [{**record, "path": "build/" + record["path"]} for record in support.inventory(build)], "supportFiles": [{**record, "path": "python/" + record["path"]} for record in support.inventory(python)], "nativeDependencies": imports, "rustCompiler": rust_version, "frameserverCompiler": frameserver["compiler"], "configure": {"frameserver": frameserver["configure"], "decoder": decoder["configure"], "plugin": plugin["configure"]}, "qualification": "Three actual decoded frames and av1an plugin discovery with only this frameserver and System32 on PATH; complete runtime hash inventory unchanged."}
    (directory / "build-provenance.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    print(f"Verified portable av1an source delivery: {directory}", flush=True)
