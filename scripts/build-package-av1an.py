"""Build the pinned Windows av1an engine, portable frameserver and decoder.

Uses a verified media compiler/source delivery and a native MSVC installation.
All build outputs stay in a fresh explicitly named destination. No registration,
global Python installation or system plugin directory is modified.
"""

import argparse
import concurrent.futures
import json
from pathlib import Path
import shutil
import subprocess
import sys

sys.dont_write_bytecode = True
SCRIPTS = Path(__file__).resolve().parent
LOCK = SCRIPTS / "package-av1an-windows-lock.json"


def load(name, path):
    import importlib.util
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--ffmpeg-build", type=Path, required=True)
    parser.add_argument("--msys-root", type=Path, required=True)
    parser.add_argument("--rust-directory", type=Path)
    parser.add_argument("--cache", type=Path, default=Path("target/tool-download-cache"))
    args = parser.parse_args()
    if sys.platform != "win32":
        raise SystemExit("The portable av1an recipe currently targets Windows x64.")
    components = load("av1an_components", SCRIPTS / "package-av1an-components.py")
    delivery = load("av1an_delivery", SCRIPTS / "package-av1an-delivery.py")
    support = components.support
    media = args.ffmpeg_build.resolve(strict=True)
    compiler = args.msys_root.resolve(strict=True)
    provenance = json.loads((media / "build-provenance.json").read_text(encoding="utf-8"))
    if provenance.get("schemaVersion") != 1 or provenance.get("target") != "x86_64-pc-windows-msvc":
        raise ValueError("A verified Windows media source delivery is required.")
    for record in provenance["buildInputs"]:
        if support.digest(media / record["path"]) != record["sha256"]:
            raise ValueError("The shared media build inputs changed.")
    helper = load("media_builder", media / "build/scripts/build-package-ffmpeg.py")
    stager = load("media_stager", media / "build/scripts/stage-bundled-tools.py")
    helper.verify_compiler(compiler)
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    if args.rust_directory:
        rust = args.rust_directory.resolve(strict=True)
    else:
        executable = subprocess.check_output(["rustup", "which", "--toolchain", lock["rustVersion"], "cargo"], text=True, timeout=30).strip()
        rust = Path(executable).resolve(strict=True).parent
    rust_version = subprocess.check_output([str(rust / "rustc.exe"), "--version", "--verbose"], text=True, timeout=20)
    if not rust_version.startswith("rustc " + lock["rustVersion"] + " ") or "host: x86_64-pc-windows-msvc" not in rust_version:
        raise ValueError("The locked native Rust x64 toolchain is required.")
    destination = args.destination.absolute()
    destination.mkdir(parents=True, exist_ok=False)
    cache = args.cache.absolute()
    cache.mkdir(parents=True, exist_ok=True)
    archives = {name: stager.download(record["url"], record["sha256"], cache) for name, record in lock["inputs"].items()}
    python_sources = [(record, stager.download(record["url"], record["sha256"], cache)) for record in lock["pythonSources"]]
    cmake_archive = stager.download(lock["cmakeWindows"]["url"], lock["cmakeWindows"]["sha256"], cache)
    cmake = support.unpack(cmake_archive, destination / "cmake") / "bin/cmake.exe"
    # Independent native builds use four workers apiece and never share output.
    with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
        frameserver = pool.submit(components.frameserver, destination / "frameserver", archives, compiler)
        decoder = pool.submit(components.decoder, destination / "decoder", archives, compiler, helper)
        engine = pool.submit(components.av1an, destination / "engine", archives, rust, compiler, SCRIPTS)
        decoder_result = decoder.result()
        plugin = components.lsmash(destination / "plugin", archives, compiler, cmake, decoder_result["prefix"], helper)
        frameserver_result, engine_result = frameserver.result(), engine.result()
    delivery.finish(destination, media, compiler, provenance, lock, archives, python_sources, frameserver_result, decoder_result, plugin, engine_result, rust_version, SCRIPTS)


if __name__ == "__main__":
    main()
