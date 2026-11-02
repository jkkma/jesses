"""Build the two pinned static AV1 FFmpeg libraries in the isolated Windows compiler."""

import hashlib
import io
from pathlib import Path, PurePosixPath
import tarfile
import zipfile


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def unpack_source(archive, record, destination):
    # Both the complete distribution source and its embedded upstream source are
    # pinned. No distribution build script or unreviewed patch is executed.
    if digest(archive) != record["sha256"]:
        raise ValueError("The complete AV1 source bundle changed.")
    with tarfile.open(archive) as outer:
        member = outer.getmember(record["embeddedArchive"])
        if not member.isfile() or not 0 < member.size <= 128 * 1024 * 1024:
            raise ValueError("The embedded AV1 source exceeds its bounds.")
        payload = outer.extractfile(member).read()
    if hashlib.sha256(payload).hexdigest() != record["embeddedSha256"]:
        raise ValueError("The embedded AV1 source checksum differs.")
    with tarfile.open(fileobj=io.BytesIO(payload)) as source:
        members = source.getmembers()
        if len(members) > 30_000 or sum(member.size for member in members) > 512 * 1024 * 1024:
            raise ValueError("The AV1 source exceeds its extraction bounds.")
        names = set()
        for member in members:
            parts = PurePosixPath(member.name).parts
            if not parts or parts[0] != record["sourceRoot"] or ".." in parts or "\\" in member.name or not (member.isfile() or member.isdir()):
                raise ValueError(f"Unexpected AV1 source member: {member.name}")
            if member.name in names:
                raise ValueError("The AV1 source repeats a member.")
            names.add(member.name)
        destination.mkdir()
        source.extractall(destination, filter="data")
    return destination / record["sourceRoot"]


def unpack_cmake(archive, record, destination):
    if digest(archive) != record["sha256"]:
        raise ValueError("The pinned CMake input changed.")
    with zipfile.ZipFile(archive) as source:
        members = source.infolist()
        if len(members) > 30_000 or sum(member.file_size for member in members) > 512 * 1024 * 1024:
            raise ValueError("The CMake input exceeds its extraction bounds.")
        names = set()
        for member in members:
            parts = PurePosixPath(member.filename).parts
            kind = (member.external_attr >> 16) & 0o170000
            if not parts or parts[0] != record["root"] or ".." in parts or "\\" in member.filename or kind == 0o120000:
                raise ValueError(f"Unexpected CMake input member: {member.filename}")
            if member.filename in names:
                raise ValueError("The CMake input repeats a member.")
            names.add(member.filename)
        destination.mkdir()
        source.extractall(destination)
    return destination / record["root"]


def build(destination, compiler, lock, tool_lock, cache, stager, run, *, jobs=4):
    cmake_record = tool_lock["cmakeWindows"]
    cmake_archive = stager.download(cmake_record["url"], cmake_record["sha256"], cache)
    cmake_root = unpack_cmake(cmake_archive, cmake_record, destination / "av1-cmake")
    cmake = cmake_root / "bin/cmake.exe"
    prefix = destination / "static-av1"
    for identifier in ("aom", "svt-av1"):
        record = next(record for record in lock["linkedSources"] if record.get("id") == identifier)
        archive = stager.download(record["url"], record["sha256"], cache)
        source = unpack_source(archive, record, destination / f"{identifier}-source")
        build_dir = destination / f"{identifier}-build"
        options = ["-DENABLE_TESTS=OFF", "-DENABLE_DOCS=OFF", "-DENABLE_EXAMPLES=OFF", "-DENABLE_TOOLS=OFF", "-DCONFIG_PIC=1"] if identifier == "aom" else ["-DBUILD_APPS=OFF", "-DBUILD_TESTING=OFF", "-DNATIVE=OFF", "-DSVT_AV1_PGO=OFF"]
        common = ["-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release", f"-DCMAKE_INSTALL_PREFIX={prefix.as_posix()}", "-DCMAKE_INSTALL_LIBDIR=lib", "-DBUILD_SHARED_LIBS=OFF", "-DCMAKE_POSITION_INDEPENDENT_CODE=ON", "-DFETCHCONTENT_FULLY_DISCONNECTED=ON", "-DFETCHCONTENT_UPDATES_DISCONNECTED=ON", f"-DCMAKE_C_COMPILER={(compiler / 'ucrt64/bin/gcc.exe').as_posix()}", f"-DCMAKE_CXX_COMPILER={(compiler / 'ucrt64/bin/g++.exe').as_posix()}", f"-DCMAKE_MAKE_PROGRAM={(compiler / 'ucrt64/bin/ninja.exe').as_posix()}", f"-DCMAKE_ASM_NASM_COMPILER={(compiler / 'usr/bin/nasm.exe').as_posix()}"]
        run([str(cmake), "-S", str(source), "-B", str(build_dir), *common, *options], destination)
        run([str(cmake), "--build", str(build_dir), "--parallel", str(jobs)], destination)
        run([str(cmake), "--install", str(build_dir)], destination)
    for name in ("aom.pc", "SvtAv1Enc.pc"):
        if not (prefix / "lib/pkgconfig" / name).is_file():
            raise ValueError(f"The static AV1 build did not install {name}.")
    if list(prefix.rglob("*.dll")) or list(prefix.rglob("*.dll.a")):
        raise ValueError("The private AV1 prefix contains shared-library inputs.")
    return prefix, cmake_root / "doc/cmake/LICENSE.rst"
