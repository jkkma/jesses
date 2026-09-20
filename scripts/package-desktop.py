"""Assemble and verify unsigned desktop artifacts without touching user data.

Uses only the Python standard library. Media tools remain external: none are
copied from a developer's PATH or private tool installation into a package.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import stat
import subprocess
import sys
import tomllib
from urllib.parse import urlsplit
import zipfile

REPOSITORY = Path(__file__).resolve().parents[1]
SUPPORTED = {"x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"}
MANIFEST = "package-manifest.json"


def digest(path: Path) -> str:
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def regular_file(path: Path) -> None:
    if path.is_symlink() or path.is_junction() or not path.is_file():
        raise ValueError(f"Expected an ordinary file: {path}")


def relative_path(value: str) -> Path:
    path = PurePosixPath(value)
    if not value or "\\" in value or path.is_absolute() or any(part in {".", ".."} for part in value.split("/")) or ":" in value:
        raise ValueError(f"Unsafe package entry: {value}")
    return Path(*path.parts)


def configuration() -> tuple[dict, str]:
    config = json.loads((REPOSITORY / "src-tauri/tauri.conf.json").read_text(encoding="utf-8"))
    frontend = json.loads((REPOSITORY / "package.json").read_text(encoding="utf-8"))
    workspace = tomllib.loads((REPOSITORY / "Cargo.toml").read_text(encoding="utf-8"))
    version = config["version"]
    if version != frontend["version"] or version != workspace["workspace"]["package"]["version"]:
        raise ValueError("Desktop, frontend and Rust versions differ.")
    return config, version


def source_revision() -> dict:
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPOSITORY, text=True).strip()
    dirty = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=REPOSITORY, text=True).strip())
    return {"repository": "https://github.com/jkkma/jesses", "revisionAtPackaging": revision, "workingTreeModified": dirty,
            "note": "Records the packaging checkout; the executable hash identifies the supplied binary. This is not a source-to-binary attestation."}


def inventory(directory: Path) -> list[dict]:
    files = []
    for path in sorted(directory.rglob("*")):
        if path.is_symlink() or path.is_junction():
            raise ValueError(f"A package must not contain redirected entries: {path}")
        if path.is_file() and path != directory / MANIFEST:
            files.append({"path": path.relative_to(directory).as_posix(), "size": path.stat().st_size, "sha256": digest(path)})
    return files


def write_json(path: Path, value: object) -> None:
    with path.open("x", encoding="utf-8", newline="\n") as output:
        output.write(json.dumps(value, indent=2) + "\n")


def assemble(executable: Path, destination: Path, target: str, profile: str, portable: bool, tool_resources: Path | None = None) -> Path:
    if target not in SUPPORTED:
        raise ValueError("Only Windows x64 and Linux x64 are package targets.")
    if portable and target != "x86_64-pc-windows-msvc":
        raise ValueError("Portable ZIP assembly currently targets Windows; Linux uses the native AppImage/DEB bundles.")
    regular_file(executable)
    config, version = configuration()
    resources = config["bundle"]["resources"]
    if not isinstance(resources, dict):
        raise ValueError("Packaging requires an explicit source-to-resource mapping.")
    destination = destination.absolute()
    destination.mkdir(parents=True, exist_ok=False)
    name = "jesses.exe" if target.startswith("x86_64-pc-windows") else "jesses"
    shutil.copy2(executable, destination / name)
    for source_name, resource_name in resources.items():
        source = (REPOSITORY / "src-tauri" / source_name).resolve()
        if not source.is_relative_to(REPOSITORY):
            raise ValueError(f"Resource source must remain inside the checkout: {source_name}")
        regular_file(source)
        output = destination / relative_path(resource_name)
        output.parent.mkdir(parents=True, exist_ok=True)
        with output.open("xb") as staged:
            with source.open("rb") as original:
                shutil.copyfileobj(original, staged)
    if portable:
        (destination / "jesses.portable").write_bytes(b"1\n")
    if tool_resources is not None:
        for entry in inventory(tool_resources):
            source = tool_resources / relative_path(entry["path"])
            regular_file(source)
            output = destination / "resources/tools" / relative_path(entry["path"])
            output.parent.mkdir(parents=True, exist_ok=True)
            with source.open("rb") as original, output.open("xb") as staged:
                shutil.copyfileobj(original, staged)
    write_json(destination / MANIFEST, {
        "schemaVersion": 1, "product": "jesses", "version": version,
        "target": target, "profile": profile, "storage": "portable" if portable else "installed",
        "signing": "unsigned", "tools": "bundledWithExternalDependencies" if tool_resources else "external", "source": source_revision(),
        "files": inventory(destination),
    })
    verify(destination)
    return destination


def verify(directory: Path) -> dict:
    directory = directory.resolve(strict=True)
    regular_file(directory / MANIFEST)
    manifest = json.loads((directory / MANIFEST).read_text(encoding="utf-8"))
    if manifest.get("schemaVersion") != 1 or manifest.get("product") != "jesses" or manifest.get("target") not in SUPPORTED:
        raise ValueError("Unsupported desktop package manifest.")
    expected = {}
    for entry in manifest["files"]:
        name = entry["path"]
        relative_path(name)
        if name in expected or name == MANIFEST:
            raise ValueError(f"Duplicate or reserved package entry: {name}")
        expected[name] = entry
    actual = {entry["path"]: entry for entry in inventory(directory)}
    if expected != actual:
        raise ValueError("Package content differs from its file sizes, hashes or inventory.")
    binary = "jesses.exe" if manifest["target"] == "x86_64-pc-windows-msvc" else "jesses"
    required = {binary, "resources/runtime-contract.json", "resources/LICENSE", "resources/THIRD_PARTY_NOTICES.md", "resources/licenses/shadcn-svelte-MIT.txt"}
    if not required.issubset(actual):
        raise ValueError("The package is missing its executable or required runtime/license resources.")
    contract = json.loads((directory / "resources/runtime-contract.json").read_text(encoding="utf-8"))
    if contract.get("schemaVersion") != 1 or contract.get("product") != "jesses" or contract.get("toolDistribution") not in {"external", "manifest"}:
        raise ValueError("Unexpected runtime dependency contract.")
    tools_manifest = directory / "resources/tools/manifest.json"
    if tools_manifest.exists():
        bundled = json.loads(tools_manifest.read_text(encoding="utf-8"))
        if manifest["tools"] != "bundledWithExternalDependencies" or bundled["schemaVersion"] != 1 or bundled["target"] != manifest["target"]:
            raise ValueError("The bundled tool manifest does not match the package.")
        identifiers = set()
        for tool in bundled["tools"]:
            if tool["id"] in identifiers:
                raise ValueError("Duplicate bundled tool identity.")
            identifiers.add(tool["id"])
            records = [tool, tool["source"], *tool["licenses"], *tool.get("additionalSources", []), *tool.get("buildInputs", []), *tool.get("supportFiles", [])]
            if "buildProvenance" in tool:
                records.append(tool["buildProvenance"])
            for record in records:
                source = directory / "resources/tools" / relative_path(record["path"])
                regular_file(source)
                if digest(source) != record["sha256"]:
                    raise ValueError(f"Bundled tool, source or license checksum mismatch: {record['path']}")
    elif manifest["tools"] != "external" or (directory / "resources/tools").exists():
        raise ValueError("The bundled tool manifest is missing.")
    marker = directory / "jesses.portable"
    if manifest["storage"] == "portable":
        if not marker.is_file() or marker.read_bytes() != b"1\n":
            raise ValueError("The portable package must contain its explicit version-1 marker.")
    elif manifest["storage"] != "installed" or marker.exists():
        raise ValueError("Installed artifacts must not enable portable storage.")
    if (directory / "jesses-data").exists():
        raise ValueError("User data must never be packaged.")
    return manifest


def verify_portable_archive(archive_path: Path) -> dict:
    regular_file(archive_path)
    with zipfile.ZipFile(archive_path) as bundle:
        members = bundle.infolist()
        actual = {}
        actual_casefolded = {}
        for member in members:
            name = member.filename
            relative_path(name)
            if member.is_dir() or stat.S_ISLNK(member.external_attr >> 16):
                raise ValueError(f"Portable archives may contain only ordinary files: {name}")
            folded = name.casefold()
            if name in actual:
                raise ValueError(f"Duplicate portable archive entry: {name}")
            if folded in actual_casefolded:
                raise ValueError(
                    f"Case-colliding portable archive entries: {actual_casefolded[folded]} and {name}"
                )
            actual[name] = member
            actual_casefolded[folded] = name

        manifest_member = actual.get(MANIFEST)
        if manifest_member is None:
            raise ValueError("The portable archive has no package manifest.")
        if manifest_member.file_size > 16 * 1024 * 1024:
            raise ValueError("The portable archive package manifest is unreasonably large.")
        package_manifest = json.loads(bundle.read(manifest_member))
        if (
            package_manifest.get("schemaVersion") != 1
            or package_manifest.get("product") != "jesses"
            or package_manifest.get("target") != "x86_64-pc-windows-msvc"
            or package_manifest.get("storage") != "portable"
        ):
            raise ValueError("Scoop requires a Jesses Windows x64 portable package.")

        expected = {}
        expected_casefolded = {}
        for entry in package_manifest["files"]:
            name = entry["path"]
            relative_path(name)
            folded = name.casefold()
            if name == MANIFEST or name in expected:
                raise ValueError(f"Duplicate or reserved package manifest entry: {name}")
            if folded in expected_casefolded:
                raise ValueError(
                    f"Case-colliding package manifest entries: {expected_casefolded[folded]} and {name}"
                )
            expected[name] = entry
            expected_casefolded[folded] = name
        if set(actual) != set(expected) | {MANIFEST}:
            raise ValueError("Portable archive contents differ from package-manifest.json.")
        if not {"jesses.exe", "jesses.portable"}.issubset(expected):
            raise ValueError("The portable archive is missing its executable or storage marker.")
        if any(
            folded == "jesses-data" or folded.startswith("jesses-data/")
            for folded in expected_casefolded
        ):
            raise ValueError("User data must never be included in a portable archive.")

        for name, entry in expected.items():
            member = actual[name]
            if member.file_size != entry["size"]:
                raise ValueError(f"Portable archive size mismatch: {name}")
            digest_value = hashlib.sha256()
            size = 0
            with bundle.open(member) as source:
                while block := source.read(1024 * 1024):
                    digest_value.update(block)
                    size += len(block)
            if size != entry["size"] or digest_value.hexdigest() != entry["sha256"]:
                raise ValueError(f"Portable archive payload hash mismatch: {name}")
        if bundle.read(actual["jesses.portable"]) != b"1\n":
            raise ValueError("The portable archive has an invalid storage marker.")
    return package_manifest


def archive(directory: Path, output: Path) -> None:
    verify(directory)
    with zipfile.ZipFile(output, "x", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as bundle:
        for path in sorted(directory.rglob("*")):
            if path.is_file():
                bundle.write(path, path.relative_to(directory).as_posix())
    verify_portable_archive(output)


def collect(build_directory: Path, destination: Path, target: str, profile: str, tool_resources: Path | None = None) -> None:
    _, version = configuration()
    destination.mkdir(parents=True, exist_ok=False)
    # Windows distribution is the portable ZIP used by Scoop. Old NSIS output
    # in a reused build directory must never become a release artifact.
    extensions = {} if target == "x86_64-pc-windows-msvc" else {"appimage": "*.AppImage", "deb": "*.deb"}
    for kind, pattern in extensions.items():
        candidates = sorted((build_directory / "bundle" / kind).glob(pattern))
        if len(candidates) != 1:
            raise ValueError(f"Expected exactly one {kind} bundle, found {len(candidates)}. Use a fresh build output directory.")
        regular_file(candidates[0])
        shutil.copy2(candidates[0], destination / candidates[0].name)
    if target == "x86_64-pc-windows-msvc":
        staged = assemble(build_directory / "jesses.exe", destination / "portable", target, profile, True, tool_resources)
        archive(staged, destination / f"jesses_{version}_{target}_{profile}_portable.zip")
    shutil.copy2(REPOSITORY / "LICENSE", destination / "LICENSE")
    shutil.copy2(REPOSITORY / "THIRD_PARTY_NOTICES.md", destination / "THIRD_PARTY_NOTICES.md")
    shutil.copy2(REPOSITORY / "src-tauri/resources/runtime-contract.json", destination / "runtime-contract.json")
    receipts = [{"path": path.name, "size": path.stat().st_size, "sha256": digest(path)}
                for path in sorted(destination.iterdir()) if path.is_file()]
    write_json(destination / "artifact-manifest.json", {
        "schemaVersion": 1, "product": "jesses", "version": version, "target": target,
        "profile": profile, "signing": "unsigned", "tools": "bundledWithExternalDependencies" if tool_resources else "external", "source": source_revision(), "artifacts": receipts,
    })
    with (destination / "SHA256SUMS").open("x", encoding="utf-8", newline="\n") as checksums:
        for record in receipts:
            checksums.write(f"{record['sha256']}  {record['path']}\n")
        checksums.write(f"{digest(destination / 'artifact-manifest.json')}  artifact-manifest.json\n")


def diagnostics(directory: Path, sanitized_path: bool) -> dict:
    manifest = verify(directory)
    contract = json.loads((directory / "resources/runtime-contract.json").read_text(encoding="utf-8"))
    # This is deliberately a package dependency inventory, not a claim that the
    # native application's managed/override discovery or workflow passed.
    path = str(Path(os.environ.get("SystemRoot", "C:/Windows")) / "System32") if os.name == "nt" else "/usr/bin:/bin"
    if not sanitized_path:
        path = os.environ.get("PATH", "")
    tools_manifest = directory / "resources/tools/manifest.json"
    bundled = json.loads(tools_manifest.read_text(encoding="utf-8"))["tools"] if tools_manifest.exists() else []
    return {"packageVerified": True, "target": manifest["target"], "storage": manifest["storage"],
            "tools": manifest["tools"], "pathSanitized": sanitized_path,
            "bundledTools": [{"id": tool["id"], "path": tool["path"], "sha256": tool["sha256"]} for tool in bundled],
            "runtime": contract["platforms"][manifest["target"]],
            "pathToolInventory": [{"id": tool["id"], "path": find_on_path(tool["executable"], path)} for tool in contract["tools"]],
            "qualification": contract["qualification"]}


def scoop_manifest(archive_path: Path, url: str, destination: Path) -> None:
    location = urlsplit(url)
    local_test = location.scheme == "http" and location.hostname in {"127.0.0.1", "localhost", "::1"}
    if (location.scheme != "https" and not local_test) or not location.hostname or location.username or location.password:
        raise ValueError("Use an HTTPS release URL, or a loopback HTTP URL for local qualification.")
    package_manifest = verify_portable_archive(archive_path)
    write_json(destination, {
        "version": package_manifest["version"],
        "description": "Desktop media encoding, muxing, and analysis.",
        "homepage": "https://github.com/jkkma/jesses",
        "license": "GPL-3.0-only",
        "architecture": {"64bit": {"url": url, "hash": digest(archive_path)}},
        "bin": "jesses.exe",
        "persist": "jesses-data",
        "notes": [
            "Requires the Microsoft Edge WebView2 runtime already available on the machine.",
            "Preferences, history, recovery metadata and browser state are kept in jesses-data and persist across Scoop updates and ordinary uninstall.",
            "Keep source media and recovery workspace paths unchanged while a job is saved for resume.",
        ],
    })


def find_on_path(executable: str, search_path: str) -> str | None:
    # Never let a Windows CWD lookup or a relative PATH entry supply a tool.
    for entry in search_path.split(os.pathsep):
        directory = Path(entry)
        if not entry or not directory.is_absolute():
            continue
        candidate = directory / (executable + ".exe" if os.name == "nt" else executable)
        if candidate.is_file() and (os.name == "nt" or os.access(candidate, os.X_OK)):
            return str(candidate)
    return None


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="command", required=True)
    stage = subcommands.add_parser("assemble")
    stage.add_argument("--executable", required=True, type=Path)
    stage.add_argument("--destination", required=True, type=Path)
    stage.add_argument("--target", choices=sorted(SUPPORTED), required=True)
    stage.add_argument("--profile", choices=["debug", "release"], required=True)
    stage.add_argument("--portable", action="store_true")
    stage.add_argument("--tool-resources", type=Path)
    check = subcommands.add_parser("verify")
    check.add_argument("directory", type=Path)
    zip_command = subcommands.add_parser("archive")
    zip_command.add_argument("directory", type=Path)
    zip_command.add_argument("output", type=Path)
    gather = subcommands.add_parser("collect")
    gather.add_argument("--build-directory", required=True, type=Path)
    gather.add_argument("--destination", required=True, type=Path)
    gather.add_argument("--target", choices=sorted(SUPPORTED), required=True)
    gather.add_argument("--profile", choices=["debug", "release"], required=True)
    gather.add_argument("--tool-resources", type=Path)
    diagnose = subcommands.add_parser("diagnostics")
    diagnose.add_argument("directory", type=Path)
    diagnose.add_argument("--sanitized-path", action="store_true")
    scoop = subcommands.add_parser("scoop")
    scoop.add_argument("--archive", type=Path, required=True)
    scoop.add_argument("--url", required=True)
    scoop.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "assemble":
        result = assemble(args.executable, args.destination, args.target, args.profile, args.portable, args.tool_resources)
        print(f"Verified package: {result}")
    elif args.command == "verify":
        result = verify(args.directory)
        print(f"Verified {result['product']} {result['version']} {result['target']} ({result['storage']})")
    elif args.command == "archive":
        archive(args.directory, args.output)
        print(f"Verified archive: {args.output}")
    elif args.command == "collect":
        collect(args.build_directory, args.destination, args.target, args.profile, args.tool_resources)
        print(f"Collected unsigned artifacts: {args.destination}")
    elif args.command == "scoop":
        scoop_manifest(args.archive, args.url, args.destination)
        print(f"Wrote Scoop manifest: {args.destination}")
    else:
        print(json.dumps(diagnostics(args.directory, args.sanitized_path), indent=2))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.SubprocessError, zipfile.BadZipFile) as error:
        print(f"Packaging failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
