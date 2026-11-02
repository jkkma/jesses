"""Pinned static libplacebo/Vulkan processing dependencies for package FFmpeg.

The caller supplies its existing source-verifying builder and isolated compiler.
No package manager, Git checkout, Meson wrap or CMake download is permitted.
Vulkan's platform loader/driver is supplied at runtime; neither is linked here.
"""

import json
from pathlib import Path
import shutil
import hashlib

LOCK = Path(__file__).with_name("package-libplacebo-lock.json")
SUBMODULES = {
    "vulkan-headers": "Vulkan-Headers",
    "fast-float": "fast_float",
    "jinja": "jinja",
    "markupsafe": "markupsafe",
}
PYTHON314_FILE = "src/vulkan/utils_gen.py"
PYTHON314_BEFORE = b"registry = VkXML(ET.parse(xmlfile))"
PYTHON314_AFTER = b"registry = VkXML(ET.parse(xmlfile).getroot())"


def compatible_source(relative, contents):
    if relative == PYTHON314_FILE:
        if contents.count(PYTHON314_BEFORE) != 1:
            raise ValueError("The pinned libplacebo ElementTree call differs.")
        return contents.replace(PYTHON314_BEFORE, PYTHON314_AFTER)
    return contents


def sources(builder, lock=None):
    lock = lock or json.loads(LOCK.read_text(encoding="utf-8"))
    if lock["schemaVersion"] != 1:
        raise ValueError("Unsupported Vulkan processing source lock.")
    for patch in lock["compatibilityPatches"]:
        path = LOCK.with_name(patch["path"])
        if path.is_symlink() or hashlib.sha256(path.read_bytes()).hexdigest() != patch["sha256"]:
            raise ValueError("The Vulkan processing compatibility patch changed.")
    records = {item["id"]: builder.source(item) for item in lock["libraries"]}
    if set(records) != {"libplacebo", "glslang", *SUBMODULES}:
        raise ValueError("The Vulkan processing source closure is incomplete.")
    return records


def locked_sources(descriptor):
    if descriptor["path"] != LOCK.name or hashlib.sha256(LOCK.read_bytes()).hexdigest() != descriptor["sha256"]:
        raise ValueError("The FFmpeg processing-source lock differs.")
    return json.loads(LOCK.read_text(encoding="utf-8"))


def build(builder, records, cmake="cmake", cmake_options=()):
    """Build with Builder.source/run/meson_build's guarded source contract."""
    placebo = records["libplacebo"]
    builder.env["PYTHONDONTWRITEBYTECODE"] = "1"
    generator = placebo / PYTHON314_FILE
    contents = generator.read_bytes()
    if PYTHON314_BEFORE in contents:
        generator.write_bytes(compatible_source(PYTHON314_FILE, contents))
    elif contents.count(PYTHON314_AFTER) != 1:
        raise ValueError("The libplacebo Python3.14 compatibility patch differs.")
    for identifier, name in SUBMODULES.items():
        target = placebo / "3rdparty" / name
        if target.exists():
            # Git archive includes empty submodule directories. Never replace
            # content that could hide an unpinned source or local modification.
            if not target.is_dir():
                raise ValueError(f"The libplacebo submodule is not a directory: {name}")
            if any(target.iterdir()):
                # A previous interrupted generator may have produced Python's
                # disposable bytecode inside this owned submodule copy.
                for cache in target.rglob("__pycache__"):
                    if cache.is_symlink() or any(path.is_symlink() or not path.is_file() or path.suffix != ".pyc" for path in cache.iterdir()):
                        raise ValueError("An owned Python bytecode cache contains foreign entries.")
                    shutil.rmtree(cache)
                def identity(root):
                    return {path.relative_to(root).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
                            for path in root.rglob("*") if path.is_file() and not path.is_symlink()}
                if identity(target) != identity(records[identifier]) or any(path.is_symlink() for path in target.rglob("*")):
                    raise ValueError(f"The libplacebo submodule differs from its pinned source: {name}")
                continue
            target.rmdir()
        shutil.copytree(records[identifier], target)
    common = [
        "-G", "Ninja", f"-DCMAKE_INSTALL_PREFIX={builder.prefix.as_posix()}",
        "-DCMAKE_INSTALL_LIBDIR=lib", "-DCMAKE_BUILD_TYPE=Release",
        "-DCMAKE_POSITION_INDEPENDENT_CODE=ON", "-DBUILD_SHARED_LIBS=OFF",
        "-DFETCHCONTENT_FULLY_DISCONNECTED=ON", "-DFETCHCONTENT_UPDATES_DISCONNECTED=ON",
        *cmake_options,
    ]
    for identifier, options in [
        ("vulkan-headers", ["-DVULKAN_HEADERS_ENABLE_TESTS=OFF", "-DVULKAN_HEADERS_ENABLE_INSTALL=ON"]),
        ("glslang", ["-DENABLE_OPT=OFF", "-DBUILD_TESTING=OFF", "-DGLSLANG_TESTS=OFF",
                     "-DENABLE_CTEST=OFF", "-DENABLE_GLSLANG_BINARIES=ON", "-DENABLE_SPVREMAPPER=OFF",
                     "-DENABLE_PCH=OFF", "-DBUILD_EXTERNAL=OFF", "-DGLSLANG_ENABLE_INSTALL=ON"]),
    ]:
        directory = builder.work / f"{identifier}-build"
        builder.run([cmake, "-S", records[identifier], "-B", directory, *common, *options], timeout=300)
        builder.run([cmake, "--build", directory, "--parallel", builder.jobs])
        builder.run([cmake, "--install", directory])
    # Meson's find_library probes GCC's library search directories, separately
    # from linker arguments. Only this owned static prefix is added explicitly.
    builder.env["LIBRARY_PATH"] = str(builder.prefix / "lib")
    builder.meson_build(
        "libplacebo", placebo, "-Dprefer_static=true", "-Dvulkan=enabled",
        "-Dvk-proc-addr=disabled", "-Dglslang=enabled", "-Dshaderc=disabled",
        "-Dopengl=disabled", "-Dd3d11=disabled", "-Ddovi=enabled", "-Dlibdovi=disabled",
        "-Dlcms=disabled", "-Dunwind=disabled", "-Dxxhash=disabled",
        "-Ddemos=false", "-Dtests=false", "-Dbench=false",
    )
    pc = builder.prefix / "lib/pkgconfig/libplacebo.pc"
    # libplacebo is C++ internally. Its static .pc must carry the compiler's
    # C++ runtime when FFmpeg's C linker consumes it.
    text = pc.read_text(encoding="utf-8")
    # Meson emits absolute find_library archive paths. FFmpeg's pkg-config
    # parser classifies bare archive paths as compile flags, placing them
    # before objects and breaking static link order. Preserve the exact same
    # owned libraries/order using conventional -l entries instead.
    for name in ("glslang-default-resource-limits", "SPIRV", "glslang", "MachineIndependent", "OSDependent", "OGLCompiler", "GenericCodeGen"):
        text = text.replace((builder.prefix / "lib" / f"lib{name}.a").as_posix(), f"-l{name}")
    if "-lstdc++" not in text:
        text += "\nLibs.private: -lstdc++\n" if "Libs.private:" not in text else ""
        if "-lstdc++" not in text:
            text = text.replace("Libs.private:", "Libs.private: -lstdc++", 1)
    pc.write_text(text, encoding="utf-8")
    for name in ("libplacebo.a", "libglslang.a", "libSPIRV.a"):
        if not (builder.prefix / "lib" / name).is_file():
            raise ValueError(f"The static Vulkan processing library is absent: {name}")
