"""Build pinned mainline SVT-AV1 with the verified Windows media compiler."""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import stat
import subprocess
import sys
import tarfile
import zipfile

sys.dont_write_bytecode = True
LOCK = Path(__file__).with_name("standalone-tool-sources.json")


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--ffmpeg-build", type=Path, required=True)
    parser.add_argument("--msys-root", type=Path, required=True)
    parser.add_argument("--cache", type=Path, default=Path("target/tool-download-cache"))
    args = parser.parse_args()
    if sys.platform != "win32":
        raise SystemExit("This driver currently builds Windows x64 SVT-AV1.")
    media = args.ffmpeg_build.resolve(strict=True)
    compiler = args.msys_root.resolve(strict=True)
    provenance = json.loads((media / "build-provenance.json").read_text(encoding="utf-8"))
    if provenance.get("schemaVersion") != 1 or provenance.get("target") != "x86_64-pc-windows-msvc":
        raise ValueError("A verified Windows media source delivery is required.")
    for record in provenance["buildInputs"]:
        if digest(media / record["path"]) != record["sha256"]:
            raise ValueError("The shared media build inputs changed.")
    helper = load("media_builder", media / "build/scripts/build-package-ffmpeg.py")
    stager = load("media_stager", media / "build/scripts/stage-bundled-tools.py")
    helper.verify_compiler(compiler)
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    source_record, cmake_record = lock["svtAv1"], lock["cmakeWindows"]
    destination = args.destination.absolute()
    destination.mkdir(parents=True, exist_ok=False)
    cache = args.cache.absolute()
    cache.mkdir(parents=True, exist_ok=True)
    source_archive = stager.download(source_record["url"], source_record["sha256"], cache)
    cmake_archive = stager.download(cmake_record["url"], cmake_record["sha256"], cache)
    with tarfile.open(source_archive) as bundle:
        members = bundle.getmembers()
        if len(members) > 20_000 or sum(member.size for member in members) > 512 * 1024 * 1024:
            raise ValueError("The SVT source exceeds its extraction bounds.")
        names = set()
        for member in members:
            parts = PurePosixPath(member.name).parts
            if not parts or parts[0] != source_record["root"] or ".." in parts or "\\" in member.name or not (member.isfile() or member.isdir()):
                raise ValueError(f"Unexpected source member: {member.name}")
            if member.isfile() and member.name in names:
                raise ValueError("The source archive repeats a file.")
            names.add(member.name)
        bundle.extractall(destination, filter="data")
    with zipfile.ZipFile(cmake_archive) as bundle:
        members = bundle.infolist()
        if len(members) > 20_000 or sum(member.file_size for member in members) > 512 * 1024 * 1024:
            raise ValueError("The CMake archive exceeds its extraction bounds.")
        names = set()
        for member in members:
            parts = PurePosixPath(member.filename).parts
            if not parts or parts[0] != cmake_record["root"] or ".." in parts or "\\" in member.filename or stat.S_ISLNK(member.external_attr >> 16):
                raise ValueError(f"Unexpected CMake member: {member.filename}")
            if member.filename in names:
                raise ValueError("The CMake archive repeats a file.")
            names.add(member.filename)
        bundle.extractall(destination)
    source = destination / source_record["root"]
    cmake = destination / cmake_record["root"] / "bin/cmake.exe"
    build = destination / "build"
    env = {name: value for name, value in helper.build_environment("").items() if not name.upper().startswith(("CMAKE_", "GIT_"))}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_SYSTEM=os.devnull)
    env["PATH"] = os.pathsep.join(map(str, [compiler / "ucrt64/bin", compiler / "usr/bin", Path(os.environ["SystemRoot"]) / "System32"]))
    flags = ["-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release", "-DBUILD_SHARED_LIBS=OFF", "-DBUILD_TESTING=OFF", "-DBUILD_APPS=ON", "-DNATIVE=OFF", "-DSVT_AV1_PGO=OFF", "-DCMAKE_EXE_LINKER_FLAGS=-static -static-libgcc -static-libstdc++", f'-DCMAKE_C_FLAGS=-ffile-prefix-map="{destination.as_posix()}"=.', f'-DCMAKE_CXX_FLAGS=-ffile-prefix-map="{destination.as_posix()}"=.']
    local_flags = [f"-DCMAKE_C_COMPILER={compiler / 'ucrt64/bin/gcc.exe'}", f"-DCMAKE_CXX_COMPILER={compiler / 'ucrt64/bin/g++.exe'}", f"-DCMAKE_MAKE_PROGRAM={compiler / 'ucrt64/bin/ninja.exe'}", f"-DCMAKE_ASM_NASM_COMPILER={compiler / 'usr/bin/nasm.exe'}"]
    with (destination / "build.log").open("x", encoding="utf-8") as output:
        for command in [[str(cmake), "-S", str(source), "-B", str(build), *flags, *local_flags], [str(cmake), "--build", str(build), "--target", "SvtAv1EncApp", "--parallel", "4"]]:
            print("Building pinned mainline SVT-AV1", flush=True)
            subprocess.run(command, env=env, cwd=destination, stdout=output, stderr=subprocess.STDOUT, check=True, timeout=1800)
    finish(destination, source, media, compiler, provenance, source_archive, source_record, cmake_record, flags)


def finish(destination, source, media, compiler, provenance, source_archive, source_record, cmake_record, flags):
    delivery = destination / "delivery"
    delivery.mkdir()
    executable = delivery / "SvtAv1EncApp.exe"
    shutil.copy2(source / "Bin/Release/SvtAv1EncApp.exe", executable)
    subprocess.run([str(compiler / "ucrt64/bin/strip.exe"), str(executable)], check=True, timeout=30)
    version_result = subprocess.run([str(executable), "--version"], capture_output=True, text=True, check=True, timeout=20)
    version = (version_result.stdout + version_result.stderr).strip()
    if f"v{source_record['version']}" not in version or "HDR" in version or "5fish" in version:
        raise ValueError(f"Unexpected mainline SVT identity: {version}")
    dependencies = subprocess.check_output([str(compiler / "ucrt64/bin/objdump.exe"), "-p", str(executable)], text=True, timeout=20)
    imports = sorted({line.split("DLL Name:", 1)[1].strip() for line in dependencies.splitlines() if "DLL Name:" in line})
    if not imports or any(name.lower() not in {"kernel32.dll", "msvcrt.dll", "shell32.dll", "user32.dll", "winmm.dll"} and not name.lower().startswith("api-ms-win-") for name in imports):
        raise ValueError(f"Mainline SVT imports a non-system library: {imports}")
    shutil.copy2(source_archive, delivery / "source.tar.gz")
    for name in ("LICENSE.md", "LICENSE-BSD2.md", "PATENTS.md"):
        shutil.copy2(source / name, delivery / name)
    shutil.copy2(__file__, delivery / Path(__file__).name)
    shutil.copy2(LOCK, delivery / LOCK.name)
    shutil.copy2(destination / cmake_record["root"] / "doc/cmake/LICENSE.rst", delivery / "CMake-LICENSE.rst")
    runtime_sources = [record for record in provenance["additionalSources"] if record["base"] in {"mingw-w64-gcc", "mingw-w64-crt", "mingw-w64-headers", "mingw-w64-winpthreads"}]
    if len(runtime_sources) != 4:
        raise ValueError("The shared runtime source closure is incomplete.")
    receipt = {"schemaVersion": 1, "target": provenance["target"], "tool": {"id": "svt-av1", "path": executable.name, "version": version, "sha256": digest(executable), "systemImports": imports}, "sharedMediaProvenanceSha256": digest(media / "build-provenance.json"), "source": {**source_record, "path": "source.tar.gz"}, "sourceCommit": source_record["commit"], "runtimeSources": runtime_sources, "configure": [flag for flag in flags if not flag.startswith(("-DCMAKE_C_FLAGS=", "-DCMAKE_CXX_FLAGS="))], "prefixMapping": "Build-directory paths are mapped to relative paths in compiler macros.", "cmakeInput": cmake_record, "licenses": [{"path": name, "sha256": digest(delivery / name)} for name in ("LICENSE.md", "LICENSE-BSD2.md", "PATENTS.md", "CMake-LICENSE.rst")], "buildInputs": [{"path": name, "sha256": digest(delivery / name)} for name in (Path(__file__).name, LOCK.name)]}
    (delivery / "build-provenance.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    print(f"Verified mainline SVT build: {delivery}", flush=True)


if __name__ == "__main__":
    main()
