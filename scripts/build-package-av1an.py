"""Build the pinned Windows av1an engine, portable frameserver and decoder.

Uses a verified media compiler/source delivery and a native MSVC installation.
All build outputs stay in a fresh explicitly named destination. No registration,
global Python installation or system plugin directory is modified.
"""

import argparse
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


def positive_jobs(value):
    try:
        jobs = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("jobs must be a positive integer") from error
    if jobs < 1:
        raise argparse.ArgumentTypeError("jobs must be a positive integer")
    return jobs


def build_components(components, destination, archives, compiler, cmake, rust, scripts, helper, *, jobs):
    # One component at a time keeps the selected limit a total build budget.
    frameserver = components.frameserver(destination / "frameserver", archives, compiler, jobs=jobs)
    decoder = components.decoder(destination / "decoder", archives, compiler, helper, jobs=jobs)
    plugin = components.lsmash(destination / "plugin", archives, compiler, cmake, decoder["prefix"], helper, jobs=jobs)
    engine = components.av1an(destination / "engine", archives, rust, compiler, scripts, jobs=jobs)
    return frameserver, decoder, plugin, engine


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--ffmpeg-build", type=Path, required=True)
    parser.add_argument("--msys-root", type=Path, required=True)
    parser.add_argument("--rust-directory", type=Path)
    parser.add_argument("--cache", type=Path, default=Path("target/tool-download-cache"))
    parser.add_argument("--jobs", type=positive_jobs, default=4, help="Maximum total concurrent compiler jobs (default: 4).")
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
    frameserver_result, decoder_result, plugin, engine_result = build_components(
        components, destination, archives, compiler, cmake, rust, SCRIPTS, helper, jobs=args.jobs)
    delivery.finish(destination, media, compiler, provenance, lock, archives, python_sources, frameserver_result, decoder_result, plugin, engine_result, rust_version, SCRIPTS)


if __name__ == "__main__":
    main()
