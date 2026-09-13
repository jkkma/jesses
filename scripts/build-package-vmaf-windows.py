"""Build static VMAF with embedded models from its pinned complete source bundle."""

import hashlib
import io
from pathlib import Path
import tarfile

VERSION = "3.2.0"
EMBEDDED_SHA256 = "a28f93f3b4fa65601be324587072e32a6a704a304ba7b1aec9b70b3f709bc1dc"


def complete_static_metadata(metadata):
    lines = metadata.read_text(encoding="utf-8").splitlines()
    for index, line in enumerate(lines):
        if line.startswith("Libs.private:"):
            if "-lstdc++" not in line.split():
                lines[index] += " -lstdc++"
            break
    else:
        lines.append("Libs.private: -lstdc++")
    metadata.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")


def build(destination, compiler, source_archive, run):
    with tarfile.open(source_archive) as outer:
        payload = outer.extractfile(f"mingw-w64-vmaf/vmaf-{VERSION}.tar.gz").read()
    if hashlib.sha256(payload).hexdigest() != EMBEDDED_SHA256:
        raise ValueError("The embedded VMAF source archive has an unexpected checksum.")
    with tarfile.open(fileobj=io.BytesIO(payload)) as source:
        members = source.getmembers()
        if len(members) > 20_000 or sum(member.size for member in members) > 256 * 1024 * 1024:
            raise ValueError("The VMAF source exceeds its extraction bounds.")
        for member in members:
            parts = member.name.split("/")
            if parts[0] != f"vmaf-{VERSION}" or ".." in parts or not (member.isfile() or member.isdir()):
                raise ValueError(f"Unexpected VMAF source entry: {member.name}")
        source.extractall(destination, filter="data")
    prefix = destination / "static-vmaf"
    build_dir = destination / "vmaf-build"
    python = compiler / "ucrt64/bin/python.exe"
    xxd = compiler / "usr/bin/xxd.exe"
    if not xxd.is_file():
        raise ValueError("Pinned xxd is required; VMAF otherwise silently omits its models.")
    run([str(python), "-I", "-m", "mesonbuild.mesonmain", "setup", str(build_dir), str(destination / f"vmaf-{VERSION}/libvmaf"), "--prefix", str(prefix), "--default-library=static", "--buildtype=release", "--wrap-mode=nodownload", "-Denable_tests=false", "-Denable_docs=false", "-Denable_tools=false", "-Denable_float=true", "-Dbuilt_in_models=true"], destination)
    run([str(compiler / "ucrt64/bin/ninja.exe"), "-C", str(build_dir), "-j4"], destination)
    config = (build_dir / "src/config.h").read_text(encoding="utf-8")
    if "#define VMAF_BUILT_IN_MODELS 1" not in config:
        raise ValueError("The VMAF build did not include the required default models.")
    run([str(python), "-I", "-m", "mesonbuild.mesonmain", "install", "-C", str(build_dir), "--no-rebuild"], destination)
    if not (prefix / "lib/libvmaf.a").is_file():
        raise ValueError("The static VMAF library was not installed into its private build prefix.")
    # VMAF 3.2 uses a C++ model parser; its generated static .pc omits the C++
    # runtime, which makes FFmpeg's C compiler feature probe fail to link.
    complete_static_metadata(prefix / "lib/pkgconfig/libvmaf.pc")
    return prefix
