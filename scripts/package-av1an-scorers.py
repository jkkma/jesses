"""Build pinned CPU VapourSynth scorers without changing an installed runtime."""

import argparse
import concurrent.futures
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile

sys.dont_write_bytecode = True
SCRIPTS = Path(__file__).resolve().parent
LOCK = SCRIPTS / "package-av1an-scorers-lock.json"


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


support = load("scorer_support", "package-av1an-support.py")


def unpack_jxl(archive, destination):
    # The pinned archive contains benchmark-only symlinks. Retain the complete
    # original source for delivery, but do not create uncompiled filesystem links.
    destination.mkdir()
    with tarfile.open(archive) as source:
        members = source.getmembers()
        if len(members) > 100_000 or sum(item.size for item in members) > 1024**3:
            raise ValueError("JPEG XL archive exceeds source bounds.")
        names, roots, regular = set(), set(), []
        for member in members:
            path = support.member_path(member.name)
            key = path.as_posix().casefold()
            if key in names:
                raise ValueError("JPEG XL archive repeats a path.")
            names.add(key)
            roots.add(path.parts[0])
            if member.isfile() or member.isdir():
                regular.append(member)
            elif not (member.issym() and path.parts[1:4] == ("tools", "benchmark", "metrics") and len(path.parts) == 5 and member.linkname == "iqa_wrapper.sh"):
                raise ValueError(f"Unexpected JPEG XL link: {member.name}")
        if len(roots) != 1:
            raise ValueError("JPEG XL source root is ambiguous.")
        source.extractall(destination, members=regular, filter="data")
    return destination / roots.pop()


def unpack_sources(archives, destination):
    return {
        key: unpack_jxl(path, destination / (key + "-source")) if key == "libjxl" else support.unpack(path, destination / (key + "-source"))
        for key, path in archives.items()
        if key not in {"zigSource", "zigBuild", "cmakeBuild"}
    }


def build_zip(destination, sources, zig, env):
    source = sources["vszip"]
    manifest = source / "build.zig.zon"
    text = manifest.read_text(encoding="utf-8")
    # Replace only the two pinned network dependencies with retained local trees.
    for key, directory in [("vapoursynth", sources["vsbindings"]), ("zigimg", sources["zigimg"])]:
        shutil.copytree(directory, source / "deps" / key)
        pattern = rf'(\.{key}\s*=\s*\.\{{).*?(\n        \}},)'
        replacement = rf'\1\n            .path = "deps/{key}",\2'
        text, count = re.subn(pattern, replacement, text, flags=re.S)
        if count != 1:
            raise ValueError(f"Pinned vszip dependency layout changed: {key}")
    manifest.write_text(text, encoding="utf-8")
    (destination / "vszip-local-dependencies.zon").write_text(text, encoding="utf-8")
    env = {**env, "ZIG_GLOBAL_CACHE_DIR": str(destination / "zig-global-cache"), "ZIG_LOCAL_CACHE_DIR": str(destination / "zig-local-cache")}
    with (destination / "vszip-build.log").open("x", encoding="utf-8") as log:
        support.run([zig, "build", "-Doptimize=ReleaseFast", "-Dtarget=x86_64-windows-gnu", "-Dcpu=x86_64", "-j4", "--prefix", destination / "vszip-install"], source, env, log)
    binary = destination / "vszip-install/bin/vszip.dll"
    if not binary.is_file():
        raise ValueError("The pinned vszip build did not produce its DLL.")
    return binary


def build_julek(destination, sources, cmake, cl, env, compiler):
    source, jxl = sources["julek"], sources["libjxl"]
    for name in ["brotli", "highway", "skcms"]:
        shutil.copytree(sources[name], jxl / "third_party" / name, dirs_exist_ok=True)
    shutil.copytree(jxl, source / "thirdparty/libjxl", dirs_exist_ok=True)
    jxl = source / "thirdparty/libjxl"
    build = destination / "jxl-build"
    installed = source / "thirdparty/libjxl_build/install"
    common = ["-G", "Ninja", f"-DCMAKE_C_COMPILER={cl}", f"-DCMAKE_CXX_COMPILER={cl}", f"-DCMAKE_MAKE_PROGRAM={compiler / 'ucrt64/bin/ninja.exe'}", "-DCMAKE_BUILD_TYPE=Release", "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded", "-DCMAKE_POLICY_DEFAULT_CMP0091=NEW", "-DCMAKE_C_FLAGS_RELEASE=/O2 /Ob2 /DNDEBUG", "-DCMAKE_CXX_FLAGS_RELEASE=/O2 /Ob2 /DNDEBUG", "-DCMAKE_POLICY_VERSION_MINIMUM=3.5", "-DCMAKE_DISABLE_FIND_PACKAGE_Git=TRUE", "-DCMAKE_DISABLE_FIND_PACKAGE_PkgConfig=TRUE"]
    flags = ["-DBUILD_SHARED_LIBS=OFF", "-DBUILD_TESTING=OFF", "-DJPEGXL_ENABLE_BENCHMARK=OFF", "-DJPEGXL_ENABLE_EXAMPLES=OFF", "-DJPEGXL_ENABLE_FUZZERS=OFF", "-DJPEGXL_ENABLE_JNI=OFF", "-DJPEGXL_ENABLE_MANPAGES=OFF", "-DJPEGXL_ENABLE_OPENEXR=OFF", "-DJPEGXL_ENABLE_SJPEG=OFF", "-DJPEGXL_ENABLE_TOOLS=OFF", "-DJPEGXL_ENABLE_JPEGLI=OFF", "-DJPEGXL_ENABLE_PLUGINS=OFF", "-DJPEGXL_ENABLE_VIEWERS=OFF", "-DJPEGXL_ENABLE_SKCMS=ON", "-DJPEGXL_BUNDLE_SKCMS=ON", "-DCMAKE_DISABLE_FIND_PACKAGE_PNG=TRUE", "-DCMAKE_DISABLE_FIND_PACKAGE_JPEG=TRUE", "-DCMAKE_DISABLE_FIND_PACKAGE_OpenEXR=TRUE", f"-DCMAKE_INSTALL_PREFIX={installed}"]
    with (destination / "julek-build.log").open("x", encoding="utf-8") as log:
        support.run([cmake, "-S", jxl, "-B", build, *common, *flags], destination, env, log)
        support.run([cmake, "--build", build, "--parallel", "4"], destination, env, log)
        support.run([cmake, "--install", build], destination, env, log)
        support.run([cmake, "-S", source, "-B", destination / "julek-build", *common, f"-DVS_INCLUDE_DIR={sources['vapoursynth'] / 'include'}", "-DVCS_TAG=r3-3f2780d"], destination, env, log)
        support.run([cmake, "--build", destination / "julek-build", "--parallel", "4"], destination, env, log)
    return destination / "julek-build/julek.dll"


def finish(destination, archives, lock, binaries, compiler, compiler_receipt):
    delivery = destination / "delivery"
    delivery.mkdir()
    (delivery / "plugins").mkdir()
    plugins = []
    imports = {}
    allowed = {"kernel32.dll", "user32.dll", "advapi32.dll", "bcrypt.dll", "crypt32.dll", "msvcrt.dll", "ucrtbase.dll", "ntdll.dll", "shell32.dll", "ole32.dll", "ws2_32.dll"}
    for name, binary in binaries.items():
        output = delivery / "plugins" / binary.name
        shutil.copy2(binary, output)
        dump = subprocess.check_output([str(compiler / "ucrt64/bin/objdump.exe"), "-p", str(output)], text=True)
        dependencies = re.findall(r"DLL Name:\s*(\S+)", dump)
        if not dependencies or any(name.lower() not in allowed and not name.lower().startswith("api-ms-win-") for name in dependencies):
            raise ValueError(f"Unclosed native scorer dependencies for {name}: {dependencies}")
        imports[name] = dependencies
        plugins.append({"id": name, "path": output.relative_to(delivery).as_posix(), "sha256": support.digest(output)})
    (delivery / "sources").mkdir()
    (delivery / "licenses").mkdir()
    (delivery / "build").mkdir()
    additional = []
    for name, archive in archives.items():
        record = lock["inputs"][name]
        is_tool = name in {"zigBuild", "cmakeBuild"}
        output = delivery / ("build" if is_tool else "sources") / record["path"]
        shutil.copy2(archive, output)
        if not is_tool:
            additional.append({**record, "component": name, "path": output.relative_to(delivery).as_posix()})
        support.source_notices(archive, delivery / "licenses" / name)
    for filename in [Path(__file__).name, LOCK.name, "package-av1an-support.py", "stage-bundled-tools.py", "build-package-ffmpeg.py", "package-ffmpeg-windows-lock.json"]:
        shutil.copy2(SCRIPTS / filename, delivery / "build" / filename)
    shutil.copy2(compiler / "jesses-toolchain.json", delivery / "build/jesses-toolchain.json")
    shutil.copy2(destination / "vszip-local-dependencies.zon", delivery / "build/vszip-local-dependencies.zon")
    receipt = {"schemaVersion": 1, "target": "x86_64-pc-windows-msvc", "plugins": plugins, "additionalSources": additional, "licenses": [{**r, "path": "licenses/" + r["path"]} for r in support.inventory(delivery / "licenses")], "buildInputs": [{**r, "path": "build/" + r["path"]} for r in support.inventory(delivery / "build")], "nativeDependencies": imports, "compiler": compiler_receipt}
    (delivery / "build-provenance.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    print(f"CPU scorer source delivery: {delivery}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--msys-root", type=Path, required=True)
    parser.add_argument("--vapoursynth-source", type=Path, required=True)
    parser.add_argument("--python", type=Path, default=Path(sys.executable))
    parser.add_argument("--cache", type=Path, default=Path("target/tool-download-cache"))
    args = parser.parse_args()
    if sys.platform != "win32":
        raise ValueError("This native recipe targets Windows x64.")
    compiler = args.msys_root.resolve(strict=True)
    media = load("scorer_media", "build-package-ffmpeg.py")
    media.verify_compiler(compiler)
    stager = load("scorer_stager", "stage-bundled-tools.py")
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    destination = args.destination.absolute()
    destination.mkdir(parents=True, exist_ok=False)
    cache = args.cache.absolute()
    cache.mkdir(parents=True, exist_ok=True)
    archives = {key: stager.download(r["url"], r["sha256"], cache) for key, r in lock["inputs"].items()}
    sources = unpack_sources(archives, destination)
    supplied_headers = args.vapoursynth_source.resolve(strict=True) / "include"
    if support.inventory(supplied_headers) != support.inventory(sources["vapoursynth"] / "include"):
        raise ValueError("The supplied VapourSynth headers differ from the pinned R79 source.")
    zig = support.unpack(archives["zigBuild"], destination / "zig") / "zig.exe"
    cmake = support.unpack(archives["cmakeBuild"], destination / "cmake") / "bin/cmake.exe"
    env, cl, compiler_receipt = support.msvc_environment(compiler, args.python.resolve(strict=True))
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        zip_future = pool.submit(build_zip, destination, sources, zig, env)
        julek_future = pool.submit(build_julek, destination, sources, cmake, cl, env, compiler)
        binaries = {"vszip": zip_future.result(), "julek": julek_future.result()}
    finish(destination, archives, lock, binaries, compiler, compiler_receipt)


if __name__ == "__main__":
    main()
