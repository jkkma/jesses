"""Native source builds used by build-package-av1an.py."""

import importlib.util
import json
import os
from pathlib import Path
import shlex
import shutil


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


support = load("av1an_package_support", Path(__file__).with_name("package-av1an-support.py"))


def frameserver(destination, archives, compiler, *, msvc_root=None, sdk_root=None, sdk_version=None):
    destination.mkdir()
    source = support.unpack(archives["vapoursynth"], destination / "source")
    cython = support.unpack(archives["cython"], destination / "cython")
    for name in ("libp2p", "zimg"):
        library = support.unpack(archives[name], destination / (name + "-source"))
        if name == "zimg":
            graph = support.unpack(archives["graphengine"], destination / "graphengine-source")
            shutil.copytree(graph, library / "graphengine", dirs_exist_ok=True)
        shutil.copytree(library, source / "subprojects" / name)
        shutil.copytree(source / "subprojects/packagefiles" / name, source / "subprojects" / name, dirs_exist_ok=True)
    python = support.unpack(archives["pythonBuild"], destination / "python-input", single_root=False) / "tools/python.exe"
    env, cl, compiler_receipt = support.msvc_environment(compiler, python, msvc_root, sdk_root, sdk_version)
    empty = destination / "empty-pkgconfig"
    empty.mkdir()
    env["PKG_CONFIG_LIBDIR"] = str(empty)
    native = destination / "native.ini"
    native.write_text("[binaries]\n" + "\n".join(f"{name} = {str(path.as_posix())!r}" for name, path in [("c", cl), ("cpp", cl), ("ar", cl.parent / "lib.exe")]) + f"\ncython = [{python.as_posix()!r}, {(cython / 'cython.py').as_posix()!r}]\n", encoding="utf-8")
    mapped = repr(["/experimental:deterministic", "/pathmap:" + destination.as_posix() + "=."])
    flags = ["--native-file", str(native), "--wrap-mode=nodownload", "-Db_vscrt=mt", "-Db_lto=false", "-Dc_args=" + mapped, "-Dcpp_args=" + mapped]
    with (destination / "build.log").open("x", encoding="utf-8") as log:
        support.run([python, "-m", "mesonbuild.mesonmain", "setup", destination / "build", source, *flags], destination, env, log)
        support.run([python, "-m", "mesonbuild.mesonmain", "compile", "-C", destination / "build", "-j", "4"], destination, env, log)
    return {"source": source, "build": destination / "build", "compiler": compiler_receipt, "configure": [flag for flag in flags if not flag.startswith(("-Dc_args=", "-Dcpp_args=")) and flag != str(native)]}


def decoder(destination, archives, compiler, media_helper):
    destination.mkdir()
    source = support.unpack(archives["decoderFfmpeg"], destination / "source")
    comparison = support.unpack(archives["diffutilsBuild"], destination / "diffutils", single_root=False) / "usr/bin"
    prefix = destination / "prefix"
    env = media_helper.build_environment("")
    # The compiler helper clears compiler/linker overrides; also remove unrelated
    # native configuration inherited by an interactive parent process.
    env = {key: value for key, value in env.items() if not key.upper().startswith(("CMAKE_", "GIT_"))}
    env["JESSES_DIFFUTILS_BIN"] = str(comparison)
    flags = ["--prefix=" + prefix.as_posix(), "--disable-autodetect", "--disable-programs", "--disable-doc", "--disable-debug", "--disable-shared", "--enable-static", "--disable-muxers", "--disable-avdevice", "--disable-avfilter", "--disable-encoders", "--disable-filters", "--disable-devices", "--disable-network", "--enable-libdav1d", "--enable-libvpx", "--enable-zlib", "--disable-bzlib", "--disable-iconv", "--disable-lzma", "--disable-pthreads", "--enable-w32threads", "--pkg-config-flags=--static --dont-define-prefix", "--extra-ldflags=-static -static-libgcc -static-libstdc++", "--extra-cflags=-ffile-prefix-map=" + destination.as_posix() + "=."]
    with (destination / "build.log").open("x", encoding="utf-8") as log:
        for command in ["./configure " + shlex.join(flags), "make -j4", "make install-libs install-headers"]:
            support.run([compiler / "usr/bin/bash.exe", "--noprofile", "--norc", "-c", 'export PATH=/ucrt64/bin:/usr/bin; extra=$(cygpath -u "$JESSES_DIFFUTILS_BIN"); export PATH="$extra:$PATH"; ' + command], source, env, log)
    return {"prefix": prefix, "configure": [flag for flag in flags if not flag.startswith(("--prefix=", "--extra-cflags="))]}


def lsmash(destination, archives, compiler, cmake, decoder_prefix, media_helper):
    destination.mkdir()
    source = support.unpack(archives["lsmash"], destination / "source")
    for name, key in [("xxHash", "xxhash"), ("obuparse", "obuparse"), ("l-smash", "liblsmash")]:
        library = support.unpack(archives[key], destination / (key + "-source"))
        shutil.copytree(library, source / name, dirs_exist_ok=True)
    env = media_helper.build_environment("")
    env = {key: value for key, value in env.items() if not key.upper().startswith(("CMAKE_", "GIT_"))}
    env["PATH"] = os.pathsep.join(map(str, [compiler / "ucrt64/bin", compiler / "usr/bin", support.system32()]))
    env["PKG_CONFIG_PATH"] = ""
    env["PKG_CONFIG_LIBDIR"] = str(compiler / "ucrt64/lib/pkgconfig")
    flags = ["-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release", "-DBUILD_AVS_PLUGIN=OFF", "-DBUILD_VS_PLUGIN=ON", "-DBUILD_AU2_PLUGIN=OFF", "-DENABLE_MFX=OFF", "-DENABLE_XML2=OFF", "-DBUILD_INDEXING_TOOL=OFF", "-DBUILD_SHARED_LIBS=OFF", "-DCMAKE_SHARED_LINKER_FLAGS=-static -static-libgcc -static-libstdc++", "-DCMAKE_MODULE_LINKER_FLAGS=-static -static-libgcc -static-libstdc++", "-DCMAKE_DISABLE_FIND_PACKAGE_Git=TRUE"]
    local = [f"-DFFMPEG_ROOT={decoder_prefix}", f"-DCMAKE_PREFIX_PATH={decoder_prefix};{compiler / 'ucrt64'}", f"-DCMAKE_C_COMPILER={compiler / 'ucrt64/bin/gcc.exe'}", f"-DCMAKE_CXX_COMPILER={compiler / 'ucrt64/bin/g++.exe'}", f"-DCMAKE_MAKE_PROGRAM={compiler / 'ucrt64/bin/ninja.exe'}", f"-DCMAKE_RC_COMPILER={compiler / 'ucrt64/bin/windres.exe'}", "-DCMAKE_C_FLAGS=-ffile-prefix-map=" + destination.as_posix() + "=."]
    with (destination / "build.log").open("x", encoding="utf-8") as log:
        support.run([cmake, "-S", source, "-B", destination / "build", *flags, *[flag.replace("\\", "/") for flag in local]], destination, env, log)
        support.run([cmake, "--build", destination / "build", "--target", "LSMASHSource", "--parallel", "4"], destination, env, log)
    return {"binary": destination / "build/LSMASHSource.dll", "configure": flags}


def av1an(destination, archives, rust_directory, compiler, scripts):
    destination.mkdir()
    source = support.unpack(archives["av1an"], destination / "source")
    patcher = load("av1an_patch", scripts / "patch-package-av1an.py")
    patcher.patch_source(source, archives["av1an"], destination / "ffmpeg9-passthrough.patch")
    env, cl, _ = support.msvc_environment(compiler, rust_directory / "cargo.exe")
    env.pop("PYTHONPATH", None)
    env["PATH"] = os.pathsep.join(map(str, [rust_directory, cl.parent, compiler / "usr/bin", support.system32()]))
    env.update(CARGO_HOME=str(destination / "cargo-home"), CARGO_TARGET_DIR=str(destination / "target"), CARGO_BUILD_JOBS="4", CARGO_PROFILE_RELEASE_STRIP="symbols", VERGEN_GIT_SHA="805dad69143fa0a81cfe2fb89c0b9e90a828ea72", VERGEN_IDEMPOTENT="1", RUSTFLAGS="--remap-path-prefix=" + str(destination) + "=. -C target-feature=+crt-static")
    (source / ".cargo").mkdir()
    with (destination / "vendor.log").open("x", encoding="utf-8") as log, (source / ".cargo/config.toml").open("x", encoding="utf-8") as config:
        import subprocess
        subprocess.run([str(rust_directory / "cargo.exe"), "vendor", "--locked", "vendor"], cwd=source, env=env, stdout=config, stderr=log, check=True, timeout=600)
    with (destination / "build.log").open("x", encoding="utf-8") as log:
        support.run([rust_directory / "cargo.exe", "build", "--release", "--frozen", "--offline", "-p", "av1an"], source, env, log)
    return {"binary": destination / "target/release/av1an.exe", "source": source, "patch": destination / "ffmpeg9-passthrough.patch"}
