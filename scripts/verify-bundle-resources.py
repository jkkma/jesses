"""Extract native installers without installing them and verify their tool resources.

This checks actual NSIS/AppImage/DEB payloads. It does not establish an interactive
installation, clean-machine compatibility, or the desktop window's operation.
"""

import argparse
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("package", ROOT / "scripts/package-desktop.py")
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


def verify_resources(resources):
    for name in ("runtime-contract.json", "LICENSE", "THIRD_PARTY_NOTICES.md", "licenses/shadcn-svelte-MIT.txt"):
        package.regular_file(resources / "resources" / package.relative_path(name))
    manifest = json.loads((resources / "resources/tools/manifest.json").read_text(encoding="utf-8"))
    if manifest.get("schemaVersion") != 1:
        raise ValueError("Unsupported extracted tool manifest.")
    records = {}
    for tool in manifest["tools"]:
        payloads = [tool, tool["source"], *tool["licenses"], *tool.get("additionalSources", []), *tool.get("buildInputs", []), *tool.get("supportFiles", [])]
        if "buildProvenance" in tool:
            payloads.append(tool["buildProvenance"])
        for record in payloads:
            name = record["path"]
            if name in records and records[name] != record["sha256"]:
                raise ValueError("Conflicting extracted payload checksums.")
            records[name] = record["sha256"]
    for name, expected in records.items():
        path = resources / "resources/tools" / package.relative_path(name)
        package.regular_file(path)
        if not path.resolve(strict=True).is_relative_to(resources.resolve(strict=True)) or package.digest(path) != expected:
            raise ValueError(f"Changed or redirected extracted tool resource: {name}")
    return sorted(tool["id"] for tool in manifest["tools"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--packages", type=Path, required=True)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--require-media", action="store_true")
    parser.add_argument("--require-tool", action="append", default=[], choices=["x264", "svt-av1", "av1an"])
    args = parser.parse_args()
    packages = args.packages.resolve(strict=True)
    probe = args.probe.resolve(strict=True)
    destination = args.destination.absolute()
    destination.mkdir(parents=True, exist_ok=False)
    manifest = json.loads((packages / "artifact-manifest.json").read_text(encoding="utf-8"))
    if manifest.get("schemaVersion") != 1 or manifest.get("product") != "jesses":
        raise ValueError("Unexpected artifact manifest.")
    receipts = []
    for record in manifest["artifacts"]:
        relative = package.relative_path(record["path"])
        if len(relative.parts) != 1:
            raise ValueError("An installer artifact must be a direct package-directory file.")
        artifact = packages / relative
        extension = artifact.suffix.lower()
        if extension not in {".exe", ".appimage", ".deb"}:
            continue
        package.regular_file(artifact)
        if package.digest(artifact) != record["sha256"]:
            raise ValueError(f"The installer checksum changed: {artifact.name}")
        extracted = destination / artifact.stem
        extracted.mkdir()
        if extension == ".exe":
            extractor = shutil.which("7z")
            if not extractor:
                raise ValueError("7-Zip is required to inspect the NSIS payload without installing it.")
            command = [extractor, "x", "-y", f"-o{extracted}", str(artifact)]
        elif extension == ".deb":
            command = ["dpkg-deb", "-x", str(artifact), str(extracted)]
        else:
            command = [str(artifact), "--appimage-extract"]
        with (extracted / "extraction.log").open("x", encoding="utf-8") as output:
            subprocess.run(command, cwd=extracted, stdout=output, stderr=subprocess.STDOUT, check=True, timeout=600)
        candidates = list(extracted.rglob("resources/tools/manifest.json"))
        if len(candidates) != 1:
            raise ValueError(f"Expected exactly one tool resource root inside {artifact.name}.")
        resources = candidates[0].parents[2]
        tools = verify_resources(resources)
        command = [sys.executable, str(ROOT / "scripts/qualify-package-tools.py"), "--probe", str(probe), "--resources", str(resources)]
        if args.require_media:
            command.append("--require-media")
        for tool in args.require_tool:
            command.extend(["--require-tool", tool])
        subprocess.run(command, check=True, timeout=150)
        receipts.append({"artifact": artifact.name, "sha256": record["sha256"], "resourceRoot": resources.relative_to(extracted).as_posix(), "verifiedBundledTools": tools})
    expected_count = 1 if manifest["target"] == "x86_64-pc-windows-msvc" else 2
    if len(receipts) != expected_count:
        raise ValueError("The platform artifact set was incomplete.")
    report = {"schemaVersion": 1, "target": manifest["target"], "artifacts": receipts, "qualification": "Native tool discovery from extracted installer resources with external tools unavailable; interactive installation and clean-machine operation are separate gates."}
    (packages / "bundle-resource-qualification.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
