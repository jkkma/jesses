"""Build Linux CI's mainline SVT from the shared standalone source pin.

--verify-only checks the archive and source identity on any host without building
or changing GitHub environment files. --archive permits a previously downloaded
archive; its checksum is verified exactly as for a fresh download.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

HERE = Path(__file__).resolve().parent
MAX_ARCHIVE = 128 * 1024 * 1024
MAX_EXPANDED = 512 * 1024 * 1024
MAX_MEMBERS = 20_000
FLAGS = [
    "-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release",
    "-DBUILD_SHARED_LIBS=OFF", "-DBUILD_TESTING=OFF", "-DBUILD_APPS=ON",
    "-DNATIVE=OFF", "-DSVT_AV1_PGO=OFF",
    "-DFETCHCONTENT_FULLY_DISCONNECTED=ON", "-DFETCHCONTENT_UPDATES_DISCONNECTED=ON",
    "-DCMAKE_SKIP_RPATH=ON", "-DCMAKE_C_COMPILER=/usr/bin/gcc",
    "-DCMAKE_CXX_COMPILER=/usr/bin/g++", "-DCMAKE_MAKE_PROGRAM=/usr/bin/ninja",
    "-DCMAKE_ASM_NASM_COMPILER=/usr/bin/nasm",
]


def source_pin():
    lock = json.loads((HERE / "standalone-tool-sources.json").read_text(encoding="utf-8"))
    pin = lock["svtAv1"]
    if (
        lock.get("schemaVersion") != 1
        or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", pin["version"])
        or not re.fullmatch(r"[0-9a-f]{40}", pin["commit"])
        or not re.fullmatch(r"[0-9a-f]{64}", pin["sha256"])
        or pin["root"] != f"SVT-AV1-{pin['commit']}-{pin['commit']}"
        or pin["url"] != "https://gitlab.com/api/v4/projects/AOMediaCodec%2FSVT-AV1/repository/archive.tar.gz?sha=" + pin["commit"]
    ):
        raise ValueError("The shared mainline SVT source pin is invalid.")
    return pin


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def download(pin, destination):
    request = urllib.request.Request(pin["url"], headers={"User-Agent": "jesses-ci"})
    total = 0
    with urllib.request.urlopen(request, timeout=90) as source, destination.open("xb") as target:
        while block := source.read(1024 * 1024):
            total += len(block)
            if total > MAX_ARCHIVE:
                raise ValueError("The SVT source archive exceeds its download bound.")
            target.write(block)


def extract_verified(archive, destination, pin):
    if archive.stat().st_size > MAX_ARCHIVE or digest(archive) != pin["sha256"]:
        raise ValueError("The SVT source archive checksum or size does not match the pin.")
    with tarfile.open(archive, "r:gz") as bundle:
        members, names, total = [], set(), 0
        for member in bundle:
            name = member.name.rstrip("/")
            parts = name.split("/")
            total += member.size
            if (
                len(members) >= MAX_MEMBERS or total > MAX_EXPANDED
                or parts[0] != pin["root"]
                or any(part in {"", ".", ".."} for part in parts)
                or any(character in name for character in "\\:\r\n\0")
                or not (member.isfile() or member.isdir())
                or name in names or member.size < 0
            ):
                raise ValueError(f"Unexpected or oversized SVT archive member: {member.name}")
            names.add(name)
            members.append(member)
        project = bundle.extractfile(f"{pin['root']}/CMakeLists.txt")
        if project is None or not re.search(
            rb"project\(svt-av1\s+VERSION\s+" + re.escape(pin["version"].encode()) + rb"(?:\s|\))",
            project.read(1024 * 1024),
        ):
            raise ValueError("The SVT archive has an unexpected CMake project version.")
        # The destination must be new; existing work or failed builds are never
        # overwritten. No links or special files survived the complete scan.
        destination.mkdir(parents=True, exist_ok=False)
        bundle.extractall(destination, members=members, filter="data")
    return destination / pin["root"]


def environment(home):
    # Do not inherit compiler, CMake, shell-startup or loader overrides from CI.
    return {
        "PATH": "/usr/bin:/bin", "HOME": str(home), "LC_ALL": "C", "TZ": "UTC",
        "SOURCE_DATE_EPOCH": "1789257600", "CFLAGS": "-O2 -march=x86-64 -mtune=generic",
        "CXXFLAGS": "-O2 -march=x86-64 -mtune=generic",
    }


def validate_version(output, pin):
    lines = [line.strip() for line in output.splitlines()]
    expected = r"SVT-AV1(?: Encoder Lib)? v" + re.escape(pin["version"]) + r"(?:\s|$)"
    if not any(re.match(expected, line) for line in lines) or any(
        marker in output.lower() for marker in ("5fish", "svt-av1-hdr", "[hdr]", "[psy]")
    ):
        raise ValueError(f"The built mainline SVT has an unexpected version: {output.strip()}")


def publish_environment(executable):
    # GitHub applies these files to subsequent steps; the installer's parent
    # process environment is never changed. All discovery routes select this
    # exact binary, even if an older distribution SvtAv1EncApp is present.
    executable = executable.resolve(strict=True)
    if any(character in str(executable) for character in "\r\n"):
        raise ValueError("The SVT executable path cannot contain line breaks.")
    with Path(os.environ["GITHUB_ENV"]).open("a", encoding="utf-8") as output:
        output.write(f"JESSES_SVT_AV1={executable}\n")
    with Path(os.environ["GITHUB_PATH"]).open("a", encoding="utf-8") as output:
        output.write(f"{executable.parent}\n")


def prepare(root, pin, archive):
    local = root / "source.tar.gz"
    if archive is None:
        download(pin, local)
    else:
        if archive.stat().st_size > MAX_ARCHIVE:
            raise ValueError("The SVT source archive exceeds its download bound.")
        shutil.copyfile(archive, local)
    source = extract_verified(local, root / "source", pin)
    print(f"Verified mainline SVT {pin['version']} at {pin['commit']}: SHA-256 {pin['sha256']}", flush=True)
    return source


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verify-only", action="store_true")
    parser.add_argument("--archive", type=Path)
    args = parser.parse_args()
    pin = source_pin()
    if args.verify_only:
        with tempfile.TemporaryDirectory(prefix="jesses-svt-source-") as temporary:
            prepare(Path(temporary), pin, args.archive)
        return
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        raise SystemExit("Building the CI SVT tool requires Linux x86-64.")
    # Validate the handoff variables before spending time compiling.
    for name in ("RUNNER_TEMP", "GITHUB_ENV", "GITHUB_PATH"):
        if not os.environ.get(name):
            raise SystemExit(f"{name} is required for a CI installation.")
    root = Path(tempfile.mkdtemp(prefix=f"jesses-svt-{pin['version']}-", dir=os.environ["RUNNER_TEMP"]))
    print(f"SVT build and diagnostics: {root}", flush=True)
    source = prepare(root, pin, args.archive)
    home = root / "empty-home"
    home.mkdir()
    env = environment(home)
    commands = [
        ["/usr/bin/cmake", "-S", str(source), "-B", str(root / "build"), *FLAGS],
        ["/usr/bin/cmake", "--build", str(root / "build"), "--target", "SvtAv1EncApp", "--parallel", str(min(os.cpu_count() or 2, 4))],
    ]
    for argv, timeout in zip(commands, (300, 1800)):
        subprocess.run(argv, cwd=root, env=env, check=True, timeout=timeout)
    destination = root / "bin"
    destination.mkdir()
    executable = destination / "SvtAv1EncApp"
    shutil.copy2(source / "Bin/Release/SvtAv1EncApp", executable)
    with executable.open("rb") as binary:
        if binary.read(4) != b"\x7fELF":
            raise ValueError("The built SVT tool is not an ELF executable.")
    result = subprocess.run([str(executable), "--version"], cwd=root, env=env, capture_output=True, text=True, check=True, timeout=15)
    version = result.stdout + result.stderr
    validate_version(version, pin)
    receipt = {"schemaVersion": 1, "source": pin, "commands": commands, "executable": str(executable), "sha256": digest(executable), "version": version.strip()}
    (root / "provenance.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    publish_environment(executable)
    print(version.strip(), flush=True)


if __name__ == "__main__":
    main()
