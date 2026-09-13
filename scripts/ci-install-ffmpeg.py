"""Build the pinned official FFmpeg release for Linux CI, or verify its source."""

import argparse
import hashlib
import os
import platform
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request
from pathlib import Path

VERSION = "9.0.1"
ROOT = f"ffmpeg-{VERSION}"
ARCHIVE = f"{ROOT}.tar.xz"
SHA256 = "cf38e0e28c7e5605942c4a77755349b0145804a397af37eb1fb4c77cb237f635"
URL = f"https://ffmpeg.org/releases/{ARCHIVE}"
# Disable optional auto-detection so changing desktop libraries cannot silently
# change this build. x265 supplies HDR fixtures; dav1d decodes the SVT outputs.
CONFIGURE = [
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
    "--enable-zlib",
    "--enable-bzlib",
    "--enable-lzma",
]


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

    destination = None if args.verify_only else Path(os.environ["RUNNER_TEMP"]) / ROOT
    # CI restores this directory only under an exact key for this script and
    # the compiler/library package versions. No cross-version fallback is used.
    marker = f"{URL}\nSHA-256 {SHA256}\nconfigure {' '.join(CONFIGURE)}\n"
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
        subprocess.run(["./configure", *CONFIGURE], cwd=build, check=True, timeout=300)
        subprocess.run(
            ["make", f"-j{min(os.cpu_count() or 2, 4)}", "ffmpeg", "ffprobe"],
            cwd=build,
            check=True,
            timeout=2400,
        )
        destination.mkdir(parents=True, exist_ok=True)
        for name in ("ffmpeg", "ffprobe", "COPYING.GPLv2", "COPYING.GPLv3", "LICENSE.md"):
            shutil.copy2(build / name, destination / name)
        verify_tools(destination)
        # Write the cache completion marker only after both executables run.
        (destination / "provenance.txt").write_text(marker, encoding="utf-8")
        add_to_path(destination)


if __name__ == "__main__":
    main()
