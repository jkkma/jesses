"""Build and verify ad-hoc macOS app archives with explicit external media dependencies.

No Homebrew binary, developer profile, signing credential or user data is copied.
The existing .app is signed by Tauri before this script inventories its bytes.
"""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import plistlib
import shutil
import struct
import subprocess
import sys
import tarfile
import time

ROOT = Path(__file__).resolve().parents[1]
TARGETS = {"aarch64-apple-darwin": 0x0100000C, "x86_64-apple-darwin": 0x01000007}
MANIFEST = "macos-package-manifest.json"
MAX_BYTES = 4 * 1024**3

def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()

def safe_name(text):
    path = PurePosixPath(text)
    if not text or path.as_posix() != text or path.is_absolute() or any(part in {"", ".", ".."} for part in path.parts) or "\\" in text or any(character in text for character in "\0\r\n"):
        raise ValueError(f"Unsafe package member: {text!r}")
    return path

def inventory(directory, exclude_manifest=False):
    files, folded, total = [], set(), 0
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"macOS application packages cannot contain redirected entries: {path}")
        if not path.is_file():
            if not path.is_dir():
                raise ValueError(f"Unsupported package entry: {path}")
            continue
        name = path.relative_to(directory).as_posix()
        safe_name(name)
        if exclude_manifest and name == MANIFEST:
            continue
        if name.casefold() in folded:
            raise ValueError(f"Case-colliding package member: {name}")
        folded.add(name.casefold())
        total += path.stat().st_size
        if len(files) >= 20_000 or total > MAX_BYTES:
            raise ValueError("macOS application payload exceeds its inventory bounds.")
        files.append({"path": name, "size": path.stat().st_size, "sha256": digest(path), "mode": path.stat().st_mode & 0o777})
    return files

def verify_app(app, target, native=False):
    if target not in TARGETS or app.suffix != ".app" or not app.is_dir() or app.is_symlink():
        raise ValueError("Choose one native Jesses .app and a supported Darwin target.")
    inventory(app)
    with (app / "Contents/Info.plist").open("rb") as source:
        info = plistlib.load(source)
    if info.get("CFBundleIdentifier") != "io.github.jkkma.jesses" or info.get("CFBundleExecutable") != "jesses":
        raise ValueError("The application bundle identity does not match Jesses.")
    executable = app / "Contents/MacOS/jesses"
    with executable.open("rb") as source:
        header = source.read(32)
    if len(header) != 32 or header[:4] != b"\xcf\xfa\xed\xfe" or struct.unpack_from("<I", header, 4)[0] != TARGETS[target]:
        raise ValueError("The application must contain the exact target's native 64-bit Mach-O executable.")
    if not executable.stat().st_mode & 0o111:
        raise ValueError("The application launcher is not executable.")
    resource = app / "Contents/Resources/resources"
    required = ["runtime-contract.json", "LICENSE", "THIRD_PARTY_NOTICES.md", "licenses/shadcn-svelte-MIT.txt"]
    for name in required:
        if not (resource / name).is_file():
            raise ValueError(f"The application is missing a runtime/license resource: {name}")
    contract = json.loads((resource / "runtime-contract.json").read_text(encoding="utf-8"))
    if target not in contract.get("platforms", {}):
        raise ValueError("The runtime contract omits this Darwin target.")
    if (resource / "tools").exists():
        raise ValueError("This lean macOS recipe uses explicitly external media tools.")
    if native:
        if sys.platform != "darwin":
            raise ValueError("Native macOS validation must execute on macOS.")
        subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(app)], check=True, timeout=60)
        dependencies = subprocess.run(["/usr/bin/otool", "-L", str(executable)], capture_output=True, text=True, check=True, timeout=15).stdout.splitlines()[1:]
        if not dependencies or any(not line.strip().startswith(("/System/Library/", "/usr/lib/")) for line in dependencies):
            raise ValueError("The lean app launcher has an unbundled non-system dynamic dependency.")
    return info

def verify_directory(directory):
    with (directory / MANIFEST).open(encoding="utf-8") as source:
        receipt = json.load(source)
    if receipt.get("schemaVersion") != 1 or receipt.get("product") != "jesses" or receipt.get("target") not in TARGETS or receipt.get("tools") != "external":
        raise ValueError("Unsupported macOS package manifest.")
    if receipt.get("files") != inventory(directory, exclude_manifest=True):
        raise ValueError("The macOS package content differs from its exact inventory.")
    verify_app(directory / "jesses.app", receipt["target"])
    return receipt

def archive(directory, output):
    verify_directory(directory)
    with output.open("xb") as raw, gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed, tarfile.open(fileobj=compressed, mode="w") as bundle:
        for path in sorted(directory.rglob("*")):
            if path.is_file():
                bundle.add(path, arcname=path.relative_to(directory).as_posix(), recursive=False)
    verify_archive(output)

def verify_archive(path):
    actual, manifest, folded, total = {}, None, set(), 0
    with tarfile.open(path, "r:gz") as bundle:
        for member in bundle:
            safe_name(member.name)
            total += member.size
            if not member.isfile() or member.name in actual or member.name.casefold() in folded or len(actual) >= 20_001 or total > MAX_BYTES:
                raise ValueError("The macOS archive contains unexpected, duplicate or excessive entries.")
            folded.add(member.name.casefold())
            source = bundle.extractfile(member)
            if source is None:
                raise ValueError("Missing archive payload.")
            if member.name == MANIFEST:
                if member.size > 16 * 1024**2:
                    raise ValueError("The archive manifest is too large.")
                manifest = json.load(source)
            else:
                actual[member.name] = {"path": member.name, "size": member.size, "sha256": hashlib.file_digest(source, "sha256").hexdigest(), "mode": member.mode & 0o777}
    if manifest is None or manifest.get("target") not in TARGETS or manifest.get("product") != "jesses" or manifest.get("tools") != "external":
        raise ValueError("The archive has no supported Jesses macOS manifest.")
    expected = {entry["path"]: entry for entry in manifest["files"]}
    if len(expected) != len(manifest["files"]) or expected != actual:
        raise ValueError("macOS archive payload hashes, modes or inventory differ.")
    return manifest

def collect(app, destination, target, profile, native=False):
    info = verify_app(app, target, native)
    original = inventory(app)
    destination.mkdir(parents=True, exist_ok=False)
    payload = destination / "payload"
    payload.mkdir()
    shutil.copytree(app, payload / "jesses.app")
    if original != inventory(app) or original != inventory(payload / "jesses.app"):
        raise ValueError("The source application changed while staging the archive.")
    shutil.copy2(ROOT / "docs/macos.md", payload / "README-macOS.md")
    shutil.copy2(ROOT / "LICENSE", payload / "LICENSE")
    version = info.get("CFBundleShortVersionString")
    if not isinstance(version, str) or not version or any(character not in "0123456789.-abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ" for character in version):
        raise ValueError("The application has an invalid version.")
    revision = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"], check=True, text=True, capture_output=True, timeout=15).stdout.strip()
    receipt = {"schemaVersion": 1, "product": "jesses", "version": version, "target": target, "profile": profile, "tools": "external", "signing": "adHocDevelopment", "notarized": False, "revisionAtPackaging": revision, "files": inventory(payload)}
    with (payload / MANIFEST).open("x", encoding="utf-8") as output:
        output.write(json.dumps(receipt, indent=2) + "\n")
    output = destination / f"jesses_{version}_{target}_{profile}.tar.gz"
    archive(payload, output)
    with (destination / "SHA256SUMS").open("x", encoding="utf-8") as checksums:
        checksums.write(f"{digest(output)}  {output.name}\n")
    return output

def launch_smoke(app, log):
    if sys.platform != "darwin" or os.environ.get("GITHUB_ACTIONS") != "true":
        raise ValueError("The fresh-profile native startup check runs only on a hosted macOS runner.")
    with log.open("xb") as output:
        process = subprocess.Popen([str(app / "Contents/MacOS/jesses")], stdout=output, stderr=subprocess.STDOUT)
        try:
            time.sleep(4)
            if process.poll() is not None:
                raise ValueError(f"The native application exited during startup ({process.returncode}).")
        finally:
            process.terminate()
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=10)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--target", choices=sorted(TARGETS), required=True)
    parser.add_argument("--profile", choices=["debug", "release"], default="release")
    parser.add_argument("--native", action="store_true")
    parser.add_argument("--launch-smoke", action="store_true")
    args = parser.parse_args()
    result = collect(args.app.resolve(), args.destination.absolute(), args.target, args.profile, args.native)
    if args.launch_smoke:
        launch_smoke(args.destination / "payload/jesses.app", args.destination / "native-startup.log")
    print(f"Verified macOS archive: {result}")

if __name__ == "__main__":
    main()
