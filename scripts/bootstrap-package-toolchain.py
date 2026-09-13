"""Create an isolated Windows FFmpeg compiler from exact official MSYS2 inputs.

No package manager, moving repository index, profile or install hook is executed.
Only the SHA-256 pinned self-extractor and payloads in the checked-in lock are used.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("tool_stager", ROOT / "scripts/stage-bundled-tools.py")
stager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(stager)
LOCK = ROOT / "scripts/package-ffmpeg-windows-lock.json"


def native_file(root, name):
    parts = name.split("/")
    if not parts or parts[0] not in {"ucrt64", "usr"} or any(part in {"", ".", ".."} for part in parts) or "\\" in name or ":" in name:
        raise ValueError(f"Invalid toolchain entry: {name}")
    path = root.joinpath(*parts)
    if not path.resolve().is_relative_to(root.resolve()):
        raise ValueError(f"Redirected toolchain entry: {name}")
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--cache", type=Path, default=ROOT / "target/tool-download-cache")
    args = parser.parse_args()
    if sys.platform != "win32":
        raise SystemExit("This isolated compiler is for Windows x64 only.")
    destination = args.destination.absolute()
    destination.mkdir(parents=True, exist_ok=False)
    cache = args.cache.absolute()
    cache.mkdir(parents=True, exist_ok=True)
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    records = [lock["seed"], *lock["packages"]]
    with ThreadPoolExecutor(max_workers=4) as pool:
        downloads = list(pool.map(lambda record: stager.download(record["url"], record["sha256"], cache), records))
    seed = destination / "msys2-seed.exe"
    shutil.copy2(downloads[0], seed)
    with (destination / "bootstrap.log").open("x", encoding="utf-8") as output:
        subprocess.run([str(seed), "-y", f"-o{destination}"], stdout=output, stderr=subprocess.STDOUT, check=True, timeout=300, creationflags=subprocess.CREATE_NO_WINDOW)
    root = destination / "msys64"
    installed = {}
    for record, archive_path in zip(lock["packages"], downloads[1:], strict=True):
        with tarfile.open(archive_path) as archive:
            members = archive.getmembers()
            if len(members) > 100_000 or sum(member.size for member in members) > 2 * 1024**3:
                raise ValueError("A compiler package exceeds its extraction limits.")
            for member in members:
                if record.get("payloadPaths") and member.name not in record["payloadPaths"]:
                    continue
                if member.name.startswith("."):
                    continue  # Package metadata and hooks are not executable inputs.
                path = native_file(root, member.name.rstrip("/"))
                if member.isdir():
                    path.mkdir(parents=True, exist_ok=True)
                    continue
                if not (member.isfile() or member.islnk()):
                    raise ValueError(f"Unsupported compiler package entry: {member.name}")
                if member.islnk():
                    native_file(root, member.linkname)
                path.parent.mkdir(parents=True, exist_ok=True)
                # Hard links become ordinary copies, with no privilege or link
                # traversal dependency. extractfile resolves only this archive.
                with archive.extractfile(member) as source, path.open("wb") as output:
                    shutil.copyfileobj(source, output)
                installed[member.name] = stager.digest(path)
        print(f"Installed pinned build input: {record['name']} {record['version']}", flush=True)
    receipt = {"schemaVersion": 1, "lockSha256": stager.digest(LOCK), "files": installed}
    (root / "jesses-toolchain.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    print(f"Isolated compiler: {root}", flush=True)


if __name__ == "__main__":
    main()
