"""Build the pinned FFmpeg pair in an isolated, explicitly supplied toolchain.

The Windows build uses a repository-local MSYS2 UCRT64 directory; it does not
install anything into the user's environment. Build output is never reused.
"""

import argparse
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tarfile
from urllib.request import Request, urlopen

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
LOCK = ROOT / "scripts/package-ffmpeg-windows-lock.json"

VERSION = "9.0.1"
URL = f"https://ffmpeg.org/releases/ffmpeg-{VERSION}.tar.xz"
SHA256 = "cf38e0e28c7e5605942c4a77755349b0145804a397af37eb1fb4c77cb237f635"
CONFIGURE = [
    "--disable-autodetect", "--disable-doc", "--disable-ffplay", "--disable-debug",
    "--disable-shared", "--enable-static", "--enable-gpl", "--enable-version3",
    "--disable-pthreads", "--enable-w32threads", "--enable-libx264", "--enable-libx265", "--enable-libvpx",
    "--enable-libopus", "--enable-libmp3lame", "--enable-libvorbis",
    "--enable-libdav1d", "--enable-libzimg", "--enable-zlib", "--enable-bzlib",
    "--enable-libass", "--enable-libvmaf",
    "--enable-lzma", "--pkg-config-flags=--static --dont-define-prefix",
    "--extra-cflags=-I../static-vmaf/include",
    "--extra-ldflags=-L../static-vmaf/lib -static -static-libgcc -static-libstdc++",
]


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def verify_compiler(root):
    receipt = json.loads((root / "jesses-toolchain.json").read_text(encoding="utf-8"))
    if receipt.get("schemaVersion") != 1 or receipt.get("lockSha256") != digest(LOCK):
        raise ValueError("The isolated compiler was not assembled from the current input lock.")
    for name, expected in receipt["files"].items():
        path = root / name
        if not path.resolve().is_relative_to(root) or not path.is_file() or digest(path) != expected:
            raise ValueError(f"Changed compiler input: {name}")


def build_environment(pkgconfig):
    rejected = {"CFLAGS", "CPPFLAGS", "CXXFLAGS", "LDFLAGS", "LIBS", "PKG_CONFIG", "PKG_CONFIG_PATH", "PKG_CONFIG_LIBDIR", "PKG_CONFIG_SYSROOT_DIR", "PKG_CONFIG_SYSTEM_INCLUDE_PATH", "PKG_CONFIG_SYSTEM_LIBRARY_PATH", "CC", "CXX", "LD", "AR", "AS", "NM", "STRIP", "RANLIB", "MAKE", "MAKEFLAGS", "MFLAGS", "CPATH", "C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH", "LIBRARY_PATH", "LD_LIBRARY_PATH", "LD_PRELOAD", "LD_RUN_PATH", "COMPILER_PATH", "GCC_EXEC_PREFIX", "INCLUDE", "LIB", "BASH_ENV", "ENV", "CONFIG_SITE", "CROSS_COMPILE", "HOSTCC", "CC_FOR_BUILD"}
    env = {name: value for name, value in os.environ.items() if name.upper() not in rejected}
    env.update(MSYSTEM="UCRT64", CHERE_INVOKING="1", MSYS2_PATH_TYPE="strict", SOURCE_DATE_EPOCH="1789257600", LC_ALL="C", TZ="UTC")
    env["PKG_CONFIG_PATH"] = str(pkgconfig)
    return env


def write_notices(bundle, directory, prefix=""):
    count = 0
    for member in bundle.getmembers():
        name = Path(member.name).name
        if member.isfile() and member.size <= 1024 * 1024 and len(Path(member.name).parts) <= 4 and name.upper().startswith(("COPYING", "LICENSE", "PATENTS", "COPYRIGHT")):
            target = directory / (prefix + member.name.replace("/", "__"))
            with bundle.extractfile(member) as source, target.open("xb") as output:
                shutil.copyfileobj(source, output)
            count += 1
    return count


def verify_vmaf_model(executable):
    # Exercise the model from the final native binary with external tools and
    # model directories unavailable. A successful feature probe is insufficient.
    env = {name: value for name, value in os.environ.items() if name.upper() not in {"VMAF_MODEL_PATH", "VMAF_MODEL_DIR"}}
    env["PATH"] = str(Path(os.environ["SystemRoot"]) / "System32")
    subprocess.run([str(executable), "-v", "error", "-nostdin", "-f", "lavfi", "-i", "testsrc2=size=64x64:rate=2:duration=1", "-filter_complex", "[0:v]split[reference][distorted];[distorted][reference]libvmaf=model=version=vmaf_v0.6.1", "-f", "null", "-"], env=env, capture_output=True, text=True, check=True, timeout=60)


def deliver(destination, source_dir, root, versions, imports, input_lock):
    verify_vmaf_model(destination / "bin/ffmpeg.exe")
    spec = importlib.util.spec_from_file_location("stager", ROOT / "scripts/stage-bundled-tools.py")
    stager = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(stager)
    delivery = destination / "delivery"
    delivery.mkdir()
    sources_dir = delivery / "sources"
    sources_dir.mkdir()
    notices_dir = delivery / "licenses"
    notices_dir.mkdir()
    source_archive = destination / f"ffmpeg-{VERSION}.tar.xz"
    shutil.copy2(source_archive, sources_dir / source_archive.name)
    for name in ("COPYING.GPLv2", "COPYING.GPLv3", "COPYING.LGPLv2.1", "COPYING.LGPLv3", "LICENSE.md"):
        shutil.copy2(source_dir / name, notices_dir / name)
    cache = ROOT / "target/tool-download-cache"
    cache.mkdir(parents=True, exist_ok=True)
    lock = input_lock
    additional_sources = []
    for record in lock["linkedSources"]:
        archive_path = stager.download(record["url"], record["sha256"], cache)
        shutil.copy2(archive_path, sources_dir / record["filename"])
        additional_sources.append({**record, "path": f"sources/{record['filename']}"})
        library_notices = notices_dir / record["base"]
        library_notices.mkdir()
        # Include original package notices plus original upstream tar notices.
        # Git-based source packages contain their complete object repository;
        # notices for the linked MinGW runtime are also in its binary package.
        suffix = record["package"].removeprefix("mingw-w64-ucrt-x86_64-")
        installed = root / "ucrt64/share/licenses" / suffix
        if installed.is_dir():
            for item in installed.rglob("*"):
                if item.is_file():
                    shutil.copy2(item, library_notices / item.relative_to(installed).as_posix().replace("/", "__"))
        with tarfile.open(archive_path) as outer:
            write_notices(outer, library_notices)
            for member in outer.getmembers():
                # Python's tar reader supports these compression formats. A
                # retained lzip source (gettext) uses its installed notices;
                # the original complete archive is still shipped unchanged.
                if member.isfile() and 0 < member.size < 160 * 1024 * 1024 and member.name.endswith((".tar", ".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst")):
                    data = outer.extractfile(member).read()
                    with tarfile.open(fileobj=io.BytesIO(data)) as inner:
                        write_notices(inner, library_notices, "upstream__")
        if not any(library_notices.iterdir()):
            # x264 uses the GNU GPL v2 text, also preserved in its complete Git
            # sources. Keep a directly readable copy alongside the SPDX record.
            if suffix != "libx264":
                raise ValueError(f"No directly readable notices found for {record['base']}.")
            shutil.copy2(source_dir / "COPYING.GPLv2", library_notices / "COPYING.GPLv2")
    recipes = delivery / "build/scripts"
    recipes.mkdir(parents=True)
    for original in (destination / "build-inputs").iterdir():
        shutil.copy2(original, recipes / original.name)
    tool_records = []
    for name, version in versions.items():
        executable = delivery / f"{name}.exe"
        shutil.copy2(destination / "bin" / executable.name, executable)
        tool_records.append({"id": name, "path": executable.name, "sha256": digest(executable), "version": version, "systemImports": imports[name]})
    provenance = {"schemaVersion": 1, "target": "x86_64-pc-windows-msvc", "source": {"url": URL, "sha256": SHA256, "path": f"sources/{source_archive.name}"}, "configure": CONFIGURE, "compilerInputLockSha256": digest(destination / "build-inputs/package-ffmpeg-windows-lock.json"), "tools": tool_records, "additionalSources": additional_sources,
                  "licenses": [{"path": path.relative_to(delivery).as_posix(), "sha256": digest(path)} for path in sorted(notices_dir.rglob("*")) if path.is_file()],
                  "buildInputs": [{"path": path.relative_to(delivery).as_posix(), "sha256": digest(path)} for path in sorted(recipes.iterdir())]}
    (delivery / "build-provenance.json").write_text(json.dumps(provenance, indent=2) + "\n", encoding="utf-8")
    print(f"Source-complete delivery: {delivery}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--msys-root", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "win32":
        raise SystemExit("This build driver currently qualifies Windows UCRT64 only.")
    root = args.msys_root.resolve(strict=True)
    bash = root / "usr/bin/bash.exe"
    if not bash.is_file() or not (root / "ucrt64/bin/gcc.exe").is_file():
        raise SystemExit("An initialized isolated MSYS2 UCRT64 compiler is required.")
    verify_compiler(root)
    destination = args.destination.absolute()
    destination.mkdir(parents=True, exist_ok=False)
    recipe_inputs = destination / "build-inputs"
    recipe_inputs.mkdir()
    for name in ("build-package-ffmpeg.py", "build-package-vmaf-windows.py", "bootstrap-package-toolchain.py", "stage-bundled-tools.py", "package-ffmpeg-windows-lock.json"):
        shutil.copy2(ROOT / "scripts" / name, recipe_inputs / name)
    input_lock = json.loads((recipe_inputs / "package-ffmpeg-windows-lock.json").read_text(encoding="utf-8"))
    archive = destination / f"ffmpeg-{VERSION}.tar.xz"
    with urlopen(Request(URL, headers={"User-Agent": "jesses-package-builder"}), timeout=90) as source, archive.open("xb") as output:
        count = 0
        while block := source.read(1024 * 1024):
            count += len(block)
            if count > 64 * 1024 * 1024:
                raise ValueError("The source archive exceeds its size limit.")
            output.write(block)
    if digest(archive) != SHA256:
        raise ValueError("The official FFmpeg source checksum did not match.")
    with tarfile.open(archive) as source:
        members = source.getmembers()
        if len(members) > 20_000 or sum(member.size for member in members) > 512 * 1024 * 1024:
            raise ValueError("The source archive exceeds its extraction limits.")
        for member in members:
            parts = member.name.split("/")
            if parts[0] != f"ffmpeg-{VERSION}" or ".." in parts or not (member.isfile() or member.isdir()):
                raise ValueError(f"Unexpected source archive entry: {member.name}")
        source.extractall(destination, filter="data")
    source_dir = destination / f"ffmpeg-{VERSION}"
    pkgconfig = destination / "pkgconfig"
    pkgconfig.mkdir()
    # The upstream Windows x265 .pc unconditionally lists the shared GCC
    # unwinder even for a static link. Select GCC's static runtime consistently;
    # no library or source payload is changed.
    pc = (root / "ucrt64/lib/pkgconfig/x265.pc").read_text(encoding="utf-8")
    pc = pc.replace(" -lgcc_s", "").replace("prefix=/ucrt64", "prefix=" + (root / "ucrt64").as_posix())
    (pkgconfig / "x265.pc").write_text(pc, encoding="utf-8", newline="\n")
    env = build_environment(pkgconfig)
    env["PATH"] = os.pathsep.join(str(path) for path in (root / "ucrt64/bin", root / "usr/bin", Path(os.environ["SystemRoot"]) / "System32"))
    commands = [
        "./configure " + shlex.join(CONFIGURE),
        f"make -j{min(os.cpu_count() or 2, 4)} ffmpeg.exe ffprobe.exe",
    ]
    with (destination / "build.log").open("x", encoding="utf-8") as log:
        def run_native(arguments, cwd):
            print(shlex.join(arguments), flush=True)
            log.write(shlex.join(arguments) + "\n")
            log.flush()
            subprocess.run(arguments, cwd=cwd, env=env, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=1800)
        download_spec = importlib.util.spec_from_file_location("stager", recipe_inputs / "stage-bundled-tools.py")
        stager = importlib.util.module_from_spec(download_spec)
        download_spec.loader.exec_module(stager)
        cache = ROOT / "target/tool-download-cache"
        cache.mkdir(parents=True, exist_ok=True)
        vmaf_source = next(record for record in input_lock["linkedSources"] if record["base"] == "mingw-w64-vmaf")
        vmaf_archive = stager.download(vmaf_source["url"], vmaf_source["sha256"], cache)
        vmaf_spec = importlib.util.spec_from_file_location("vmaf_builder", recipe_inputs / "build-package-vmaf-windows.py")
        vmaf_builder = importlib.util.module_from_spec(vmaf_spec)
        vmaf_spec.loader.exec_module(vmaf_builder)
        vmaf_prefix = vmaf_builder.build(destination, root, vmaf_archive, run_native)
        env["PKG_CONFIG_PATH"] = str(vmaf_prefix / "lib/pkgconfig") + os.pathsep + str(pkgconfig)
        for command in commands:
            print(command, flush=True)
            subprocess.run([str(bash), "--noprofile", "--norc", "-c", "export PATH=/ucrt64/bin:/usr/bin; " + command], cwd=source_dir, env=env, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=3600)
    output_dir = destination / "bin"
    output_dir.mkdir()
    versions = {}
    imports = {}
    for name in ("ffmpeg", "ffprobe"):
        executable = output_dir / f"{name}.exe"
        shutil.copy2(source_dir / executable.name, executable)
        result = subprocess.run([str(executable), "-version"], capture_output=True, text=True, check=True, timeout=20)
        versions[name] = result.stdout.splitlines()[0]
        if not versions[name].startswith(f"{name} version {VERSION} "):
            raise ValueError(f"Unexpected built tool identity: {versions[name]}")
        dependencies = subprocess.run([str(root / "ucrt64/bin/objdump.exe"), "-p", str(executable)], capture_output=True, text=True, check=True, timeout=20)
        imports[name] = sorted({line.split("DLL Name:", 1)[1].strip() for line in dependencies.stdout.splitlines() if "DLL Name:" in line})
        system = {"advapi32.dll", "avicap32.dll", "bcrypt.dll", "comdlg32.dll", "crypt32.dll", "dwrite.dll", "gdi32.dll", "kernel32.dll", "msvcrt.dll", "ntdll.dll", "ole32.dll", "oleaut32.dll", "psapi.dll", "rpcrt4.dll", "secur32.dll", "shell32.dll", "shlwapi.dll", "user32.dll", "usp10.dll", "vfw32.dll", "winmm.dll", "ws2_32.dll"}
        if not imports[name] or any(item.lower() not in system and not item.lower().startswith("api-ms-win-") for item in imports[name]):
            raise ValueError(f"The static FFmpeg build imported an unqualified runtime DLL: {imports[name]}")
    provenance = {"schemaVersion": 1, "target": "x86_64-pc-windows-msvc", "source": {"url": URL, "sha256": SHA256, "path": archive.name}, "configure": CONFIGURE, "tools": [{"id": name, "path": f"bin/{name}.exe", "sha256": digest(output_dir / f"{name}.exe"), "version": version} for name, version in versions.items()]}
    (destination / "build-provenance.json").write_text(json.dumps(provenance, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(provenance, indent=2), flush=True)
    deliver(destination, source_dir, root, versions, imports, input_lock)


if __name__ == "__main__":
    main()
