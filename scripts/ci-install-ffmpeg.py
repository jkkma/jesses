"""Build the pinned official FFmpeg release for Linux CI, or verify its source."""

import argparse
import hashlib
import importlib.util
import json
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

VERSION = "9.0.1"
ROOT = f"ffmpeg-{VERSION}"
ARCHIVE = f"{ROOT}.tar.xz"
SHA256 = "cf38e0e28c7e5605942c4a77755349b0145804a397af37eb1fb4c77cb237f635"
URL = f"https://ffmpeg.org/releases/{ARCHIVE}"
HERE = Path(__file__).resolve().parent
CODEC_INPUTS = [HERE / "build-package-ffmpeg-linux.py", HERE / "package-ffmpeg-linux-lock.json", HERE / "qualify-cpu-encoders.py", HERE / "cpu-encoding-policy.json"]
# Disable optional auto-detection so changing desktop libraries cannot silently
# change this build. x265 supplies HDR fixtures; dav1d decodes the SVT outputs.
CPU_POLICY=json.loads((HERE/"cpu-encoding-policy.json").read_text(encoding="utf-8"))
CONFIGURE = [
    "--disable-encoder="+",".join(CPU_POLICY["excludedEncoders"]),
    "--disable-autodetect",
    "--disable-doc",
    "--disable-ffplay",
    "--disable-debug",
    "--disable-shared",
    "--enable-static",
    "--enable-gpl",
    "--enable-pthreads",
    "--enable-libx264",
    "--enable-libx265",
    "--enable-libvpx",
    "--enable-libopus",
    "--enable-libmp3lame",
    "--enable-libvorbis",
    "--enable-libdav1d",
    "--enable-libzimg",
    "--enable-libass",
    "--enable-libaom",
    "--enable-libsvtav1",
    "--enable-libvmaf",
    "--enable-zlib",
    "--enable-bzlib",
    "--enable-lzma",
]


def prepare_codec_dependencies(directory):
    spec = importlib.util.spec_from_file_location("ci_media_sources", CODEC_INPUTS[0])
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    lock = json.loads(CODEC_INPUTS[1].read_text(encoding="utf-8"))
    builder = module.Builder(directory / "codec-build", Path(os.environ["RUNNER_TEMP"]) / "ffmpeg-codec-source-cache", min(os.cpu_count() or 2, 4))
    meson = builder.source(next(item for item in lock["buildTools"] if item["id"] == "meson"))
    builder.meson = [sys.executable, str(meson / "meson.py")]
    for identifier in ("aom", "svt-av1", "vmaf"):
        source = builder.source(next(item for item in lock["libraries"] if item["id"] == identifier))
        builder.library(identifier, source)
    for path in CODEC_INPUTS:
        shutil.copy2(path, builder.delivery / "build" / path.name)
    (builder.delivery / "build/commands.json").write_text(json.dumps(builder.commands, indent=2) + "\n", encoding="utf-8")
    (builder.delivery / "build-provenance.json").write_text(json.dumps({"schemaVersion": 1, "sources": builder.sources, "licenses": builder.licenses, "libraries": ["aom", "svt-av1", "vmaf"]}, indent=2) + "\n", encoding="utf-8")
    return builder


def verify_tools(destination):
    for name in ("ffmpeg", "ffprobe"):
        executable = destination / name
        with executable.open("rb") as binary:
            if binary.read(4) != b"\x7fELF":
                raise SystemExit(f"The {name} build is not an ELF executable.")
        result = subprocess.run(
            [str(executable), "-version"],
            capture_output=True,
            text=True,
            check=True,
            timeout=15,
        )
        first = result.stdout.splitlines()[0]
        if not first.startswith(f"{name} version {VERSION} "):
            raise SystemExit(f"Unexpected {name} identity: {first}")
        print(first, flush=True)

    spec=importlib.util.spec_from_file_location("ci_cpu_encoding_qualification",HERE/"qualify-cpu-encoders.py")
    qualifier=importlib.util.module_from_spec(spec);spec.loader.exec_module(qualifier)
    (destination/"cpu-encoders.json").write_text(json.dumps(qualifier.qualify(destination/"ffmpeg"),indent=2)+"\n",encoding="utf-8")


def add_to_path(destination):
    with open(os.environ["GITHUB_PATH"], "a", encoding="utf-8") as path_file:
        path_file.write(f"{destination}\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verify-only", action="store_true")
    args = parser.parse_args()
    if not args.verify_only and (
        platform.system() != "Linux" or platform.machine() not in {"x86_64", "AMD64"}
    ):
        raise SystemExit("This qualification pair is for Linux x86-64 only.")
    if not args.verify_only and sys.version_info < (3, 14):
        raise SystemExit("Python 3.14+ is required for the pinned AV1/metric source bundles.")

    destination = None if args.verify_only else Path(os.environ["RUNNER_TEMP"]) / ROOT
    # CI restores this directory only under an exact key for this script and
    # the compiler/library package versions. No cross-version fallback is used.
    codec_fingerprint = hashlib.sha256(b"".join(path.read_bytes() for path in CODEC_INPUTS)).hexdigest()
    marker = f"{URL}\nSHA-256 {SHA256}\nconfigure {' '.join(CONFIGURE)}\ncodec-inputs {codec_fingerprint}\n"
    if destination is not None and (destination / "provenance.txt").is_file():
        if (destination / "provenance.txt").read_text(encoding="utf-8") != marker:
            raise SystemExit("The cached FFmpeg build has unexpected provenance.")
        verify_tools(destination)
        add_to_path(destination)
        return

    with tempfile.TemporaryDirectory(prefix="jesses-ffmpeg-") as temporary:
        archive = Path(temporary) / ARCHIVE
        digest = hashlib.sha256()
        request = urllib.request.Request(URL, headers={"User-Agent": "jesses-ci"})
        with urllib.request.urlopen(request, timeout=90) as source, archive.open("wb") as target:
            while block := source.read(1024 * 1024):
                digest.update(block)
                target.write(block)
        if digest.hexdigest() != SHA256:
            raise SystemExit("The official FFmpeg source archive SHA-256 did not match.")

        with tarfile.open(archive, "r:xz") as bundle:
            members = bundle.getmembers()
            if len(members) > 20_000 or sum(member.size for member in members) > 512 * 1024 * 1024:
                raise SystemExit("The FFmpeg source archive exceeds its expected bounds.")
            for member in members:
                parts = member.name.split("/")
                if (
                    parts[0] != ROOT
                    or ".." in parts
                    or not (member.isfile() or member.isdir())
                ):
                    raise SystemExit(f"Unexpected FFmpeg archive member: {member.name}")
            source = bundle.extractfile(f"{ROOT}/RELEASE")
            if source is None or source.read().strip() != VERSION.encode():
                raise SystemExit("The FFmpeg source release identity did not match.")
            if not args.verify_only:
                bundle.extractall(temporary, filter="data")

        print(f"Verified {URL}: SHA-256 {SHA256}", flush=True)
        if args.verify_only:
            return

        build = Path(temporary) / ROOT
        codecs = prepare_codec_dependencies(Path(temporary))
        build_env = os.environ.copy()
        build_env["PKG_CONFIG_PATH"] = str(codecs.prefix / "lib/pkgconfig")
        flags = [*CONFIGURE, f"--extra-cflags=-I{codecs.prefix}/include", f"--extra-ldflags=-L{codecs.prefix}/lib", "--extra-libs=-lstdc++ -lm -lpthread"]
        subprocess.run(["./configure", *flags], cwd=build, env=build_env, check=True, timeout=300)
        subprocess.run(
            ["make", f"-j{min(os.cpu_count() or 2, 4)}", "ffmpeg", "ffprobe"],
            cwd=build,
            env=build_env,
            check=True,
            timeout=2400,
        )
        destination.mkdir(parents=True, exist_ok=True)
        for name in ("ffmpeg", "ffprobe", "COPYING.GPLv2", "COPYING.GPLv3", "LICENSE.md"):
            shutil.copy2(build / name, destination / name)
        verify_tools(destination)
        # Run the actual wrappers and all three embedded models before caching.
        # The full package recipe applies the same gates to its static tools.
        executable = destination / "ffmpeg"
        for codec, options in [("libaom-av1", ["-cpu-used", "8", "-threads", "2"]), ("libsvtav1", ["-preset", "12", "-svtav1-params", "lp=2"])]:
            subprocess.run([str(executable), "-v", "error", "-nostdin", "-f", "lavfi", "-i", "testsrc2=s=128x128:r=4:d=1", "-c:v", codec, *options, "-f", "null", "-"], check=True, timeout=90)
        for model in ("vmaf_v0.6.1", "vmaf_v0.6.1neg", "vmaf_4k_v0.6.1"):
            subprocess.run([str(executable), "-v", "error", "-nostdin", "-f", "lavfi", "-i", "testsrc2=s=192x108:r=4:d=1", "-filter_complex", f"split[a][b];[a][b]libvmaf=model=version={model}", "-f", "null", "-"], check=True, timeout=60)
        shutil.copytree(codecs.delivery, destination / "codec-sources")
        # Write the cache completion marker only after both executables run.
        (destination / "provenance.txt").write_text(marker, encoding="utf-8")
        add_to_path(destination)


if __name__ == "__main__":
    main()
