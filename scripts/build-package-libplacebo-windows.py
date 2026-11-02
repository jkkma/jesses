"""Use the common pinned Vulkan processing recipe in isolated Windows UCRT64."""

import importlib.util
import json
from pathlib import Path
from types import SimpleNamespace


def load(directory, name, filename):
    spec = importlib.util.spec_from_file_location(name, directory / filename)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def build(destination, compiler, cache, env, run):
    scripts = destination / "build-inputs"
    linux = load(scripts, "guarded_processing_sources", "build-package-ffmpeg-linux.py")
    vulkan = load(scripts, "vulkan_processing", "build-package-libplacebo.py")
    work = destination / "libplacebo-work"
    delivery = destination / "libplacebo-delivery"
    for path in (work, delivery / "sources", delivery / "licenses"):
        path.mkdir(parents=True)
    prefix = destination / "static-libplacebo"
    prefix.mkdir()
    builder = SimpleNamespace(work=work, delivery=delivery, prefix=prefix, cache=cache,
                              sources=[], licenses=[], patches={}, jobs="2", env=env)
    builder.source = lambda item: linux.Builder.source(builder, item)
    builder.run = lambda args, cwd=None, **kwargs: run([str(arg) for arg in args], cwd or destination)
    builder.meson_build = lambda *args, **kwargs: linux.Builder.meson_build(builder, *args, **kwargs)
    builder.meson = [str(compiler / "ucrt64/bin/python.exe"), "-I", "-m", "mesonbuild.mesonmain"]
    authority = json.loads((scripts / "package-ffmpeg-windows-lock.json").read_text())
    records = vulkan.sources(builder, vulkan.locked_sources(authority["processingSourceLock"]))
    cmake_lock = json.loads((scripts / "standalone-tool-sources.json").read_text())["cmakeWindows"]
    cmake = destination / "av1-cmake" / cmake_lock["root"] / "bin/cmake.exe"
    env["CPPFLAGS"] = "-I" + (prefix / "include").as_posix()
    env["LDFLAGS"] = "-L" + (prefix / "lib").as_posix()
    env["CMAKE_PREFIX_PATH"] = prefix.as_posix()
    env["CC"] = str(compiler / "ucrt64/bin/gcc.exe")
    env["CXX"] = str(compiler / "ucrt64/bin/g++.exe")
    vulkan.build(builder, records, cmake, [
        f"-DCMAKE_C_COMPILER={env['CC']}", f"-DCMAKE_CXX_COMPILER={env['CXX']}",
        f"-DCMAKE_MAKE_PROGRAM={(compiler / 'ucrt64/bin/ninja.exe').as_posix()}",
    ])
    (delivery / "source-receipt.json").write_text(json.dumps({
        "sources": builder.sources, "licenses": builder.licenses,
    }, indent=2) + "\n")
    return prefix
