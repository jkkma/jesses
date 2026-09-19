"""Restore checksum-bound tools after linuxdeploy and rebuild the unsigned AppImage.

linuxdeploy inspects every ELF below AppDir/usr/lib and may strip it or rewrite its
RPATH. Jesses tools are already built, verified and represented by exact hashes in
their manifest. This helper restores that exact tree in Tauri's retained AppDir,
runs the full resource verifier, and calls the appimagetool embedded in Tauri's
already-downloaded output plugin. It does not run linuxdeploy a second time.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import stat
import subprocess
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
OUTPUT_PLUGIN_URL = (
    "https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/continuous/"
    "linuxdeploy-plugin-appimage-x86_64.AppImage"
)
TAURI_BUNDLER_SOURCE_URL = (
    "https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.11.4/"
    "crates/tauri-bundler/src/bundle/linux/appimage/linuxdeploy.rs"
)


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


verifier = load_module("bundle_resource_verifier", ROOT / "scripts/verify-bundle-resources.py")


def digest(path: Path) -> str:
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def relative_path(value: str) -> Path:
    pure = PurePosixPath(value)
    if not value or pure.is_absolute() or "\\" in value or any(part in {"", ".", ".."} for part in pure.parts):
        raise ValueError(f"Unsafe tool resource path: {value!r}")
    return Path(*pure.parts)


def require_directory(path: Path, label: str) -> Path:
    if path.is_symlink() or not path.is_dir():
        raise ValueError(f"{label} must be a real directory: {path}")
    return path.resolve(strict=True)


def require_file(path: Path, label: str, executable: bool = False) -> Path:
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"{label} must be a regular file: {path}")
    resolved = path.resolve(strict=True)
    if executable and not os.access(resolved, os.X_OK):
        raise ValueError(f"{label} is not executable: {path}")
    return resolved


def require_descendant(path: Path, root: Path, label: str) -> Path:
    resolved = path.resolve(strict=True)
    if resolved == root or not resolved.is_relative_to(root):
        raise ValueError(f"{label} is outside its required parent: {path}")
    return resolved


def tree_inventory(root: Path) -> list[dict]:
    root = require_directory(root, "Tool tree")
    inventory = []
    for path in sorted(root.rglob("*"), key=lambda entry: entry.relative_to(root).as_posix()):
        relative = path.relative_to(root).as_posix()
        mode = stat.S_IMODE(path.lstat().st_mode)
        if path.is_symlink():
            target = os.readlink(path)
            target_path = Path(target)
            if target_path.is_absolute():
                raise ValueError(f"Absolute tool-resource link: {relative}")
            resolved = (path.parent / target_path).resolve(strict=True)
            if not resolved.is_relative_to(root):
                raise ValueError(f"Escaping tool-resource link: {relative}")
            inventory.append({"path": relative, "kind": "symlink", "mode": mode, "target": target})
        elif path.is_dir():
            inventory.append({"path": relative, "kind": "directory", "mode": mode})
        elif path.is_file():
            resolved = path.resolve(strict=True)
            if not resolved.is_relative_to(root):
                raise ValueError(f"Redirected tool resource: {relative}")
            inventory.append({"path": relative, "kind": "file", "mode": mode,
                              "size": path.stat().st_size, "sha256": digest(path)})
        else:
            raise ValueError(f"Unsupported tool-resource entry: {relative}")
    return inventory


def inventory_digest(inventory: list[dict]) -> str:
    payload = json.dumps(inventory, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(payload).hexdigest()


def copy_exact_tree(source: Path, destination: Path) -> list[dict]:
    source_inventory = tree_inventory(source)
    if destination.exists() or destination.is_symlink():
        raise FileExistsError(f"A fresh restored-tool destination is required: {destination}")
    shutil.copytree(source, destination, symlinks=True, copy_function=shutil.copy2)
    restored_inventory = tree_inventory(destination)
    if restored_inventory != source_inventory:
        raise ValueError("The restored tool tree differs in content, modes or links.")
    return restored_inventory


def validate_tool_manifest(tools: Path, inventory: list[dict]) -> dict:
    manifest_path = require_file(tools / "manifest.json", "Tool manifest")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("schemaVersion") != 1 or manifest.get("target") != "x86_64-unknown-linux-gnu":
        raise ValueError("The staged tools do not have the expected Linux manifest.")
    records = {}
    for tool in manifest.get("tools", []):
        payloads = [tool, tool["source"], *tool["licenses"], *tool.get("additionalSources", []),
                    *tool.get("buildInputs", []), *tool.get("supportFiles", [])]
        if "buildProvenance" in tool:
            payloads.append(tool["buildProvenance"])
        for record in payloads:
            name = record["path"]
            relative = relative_path(name)
            path = tools / relative
            resolved = path.resolve(strict=True)
            if not resolved.is_relative_to(tools.resolve(strict=True)):
                raise ValueError(f"Redirected staged tool resource: {name}")
            if name in records and records[name] != record["sha256"]:
                raise ValueError(f"Conflicting staged tool checksum: {name}")
            if digest(path) != record["sha256"]:
                raise ValueError(f"Changed staged tool resource: {name}")
            records[name] = record["sha256"]
    actual_payloads = {entry["path"] for entry in inventory if entry["kind"] in {"file", "symlink"}}
    expected_payloads = {"manifest.json", *records}
    if actual_payloads != expected_payloads:
        raise ValueError("The staged tool tree has missing or unrecorded payloads.")
    return manifest


def one_path(paths, label: str) -> Path:
    paths = list(paths)
    if len(paths) != 1:
        raise ValueError(f"Expected exactly one {label}, found {len(paths)}.")
    return paths[0]


def repack_appimage(plugin: Path, app_dir: Path, output: Path, work: Path) -> dict:
    plugin = require_file(plugin, "Tauri AppImage output plugin", executable=True)
    extraction = work / "output-plugin"
    extraction.mkdir()
    extraction_log = work / "output-plugin-extraction.log"
    with extraction_log.open("x", encoding="utf-8") as log:
        subprocess.run([str(plugin), "--appimage-extract"], cwd=extraction, stdout=log,
                       stderr=subprocess.STDOUT, check=True, timeout=120)
    plugin_root = require_directory(extraction / "squashfs-root", "Extracted Tauri output plugin")
    appimagetool = require_file(plugin_root / "usr/bin/appimagetool", "Tauri appimagetool wrapper", executable=True)
    appimagetool_apprun = require_file(
        plugin_root / "appimagetool-prefix/AppRun", "Bundled appimagetool AppRun", executable=True
    )
    appimagetool_runtime = require_file(
        plugin_root / "appimagetool-prefix/usr/bin/appimagetool", "Bundled appimagetool runtime", executable=True
    )
    mksquashfs = require_file(
        plugin_root / "appimagetool-prefix/usr/bin/mksquashfs", "Bundled mksquashfs", executable=True
    )
    if "appimagetool-prefix/AppRun" not in appimagetool.read_text(encoding="utf-8"):
        raise ValueError("The Tauri appimagetool wrapper does not use the bundled AppRun.")
    if 'export PATH="$this_dir"/usr/bin:' not in appimagetool_apprun.read_text(encoding="utf-8"):
        raise ValueError("The bundled appimagetool AppRun does not prefer its bundled tools.")
    environment = os.environ.copy()
    environment["ARCH"] = "x86_64"
    package_log = work / "appimagetool.log"
    with package_log.open("x", encoding="utf-8") as log:
        subprocess.run([str(appimagetool), str(app_dir), str(output)], env=environment, stdout=log,
                       stderr=subprocess.STDOUT, check=True, timeout=600)
    require_file(output, "Rebuilt AppImage", executable=True)
    return {
        "pluginSource": OUTPUT_PLUGIN_URL,
        "tauriBundlerSource": TAURI_BUNDLER_SOURCE_URL,
        "pluginSha256": digest(plugin),
        "appimagetoolWrapperSha256": digest(appimagetool),
        "appimagetoolAppRunSha256": digest(appimagetool_apprun),
        "appimagetoolRuntimeSha256": digest(appimagetool_runtime),
        "mksquashfsSha256": digest(mksquashfs),
    }


def restore(build_directory: Path, source_tools: Path, plugin: Path, work: Path, repack=repack_appimage) -> dict:
    build_directory = require_directory(build_directory, "Package build directory")
    source_tools = require_directory(source_tools, "Verified staged tools")
    source_inventory = tree_inventory(source_tools)
    source_manifest = validate_tool_manifest(source_tools, source_inventory)

    appimage_directory = require_directory(build_directory / "bundle/appimage", "AppImage build directory")
    require_descendant(appimage_directory, build_directory, "AppImage build directory")
    app_dir = require_directory(one_path(appimage_directory.glob("*.AppDir"), "AppDir"), "AppDir")
    require_descendant(app_dir, appimage_directory, "AppDir")
    appimage = require_file(one_path(appimage_directory.glob("*.AppImage"), "AppImage"), "Original AppImage", executable=True)
    require_descendant(appimage, appimage_directory, "Original AppImage")
    manifest_path = one_path(app_dir.rglob("resources/tools/manifest.json"), "AppDir tool manifest")
    require_file(manifest_path, "AppDir tool manifest")
    destination_tools = manifest_path.parent
    if destination_tools.is_symlink() or not destination_tools.resolve(strict=True).is_relative_to(app_dir):
        raise ValueError("The AppDir tool destination is redirected outside the AppDir.")
    resources = manifest_path.parents[2]
    if resources.is_symlink() or not resources.resolve(strict=True).is_relative_to(app_dir):
        raise ValueError("The AppDir resource root is redirected outside the AppDir.")
    if work.exists() or work.is_symlink():
        raise FileExistsError(f"A fresh AppImage repair directory is required: {work}")
    work.mkdir(parents=True)

    shutil.rmtree(destination_tools)
    restored_inventory = copy_exact_tree(source_tools, destination_tools)
    validate_tool_manifest(destination_tools, restored_inventory)
    verified_tools = verifier.verify_resources(resources)
    if verified_tools != sorted(tool["id"] for tool in source_manifest["tools"]):
        raise ValueError("The full AppDir verifier returned a different tool inventory.")

    original = work / "original" / appimage.name
    original.parent.mkdir()
    original_hash = digest(appimage)
    shutil.move(appimage, original)
    repack_tool = repack(plugin, app_dir, appimage, work)
    rebuilt = require_file(appimage, "Rebuilt AppImage", executable=True)
    receipt = {
        "schemaVersion": 1,
        "target": "x86_64-unknown-linux-gnu",
        "originalAppImage": {"path": f"original/{original.name}", "size": original.stat().st_size, "sha256": original_hash},
        "rebuiltAppImage": {"path": rebuilt.name, "size": rebuilt.stat().st_size, "sha256": digest(rebuilt)},
        "tools": {"count": len(verified_tools), "ids": verified_tools,
                  "manifestSha256": digest(destination_tools / "manifest.json"),
                  "inventorySha256": inventory_digest(restored_inventory)},
        "repackTool": repack_tool,
        "qualification": "Restored the exact verified tool tree after linuxdeploy, ran the full resource verifier, and rebuilt only the unsigned AppImage filesystem with Tauri's cached appimagetool.",
    }
    receipt_path = work / "restoration.json"
    receipt_path.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8", newline="\n")
    print(json.dumps(receipt, indent=2))
    return receipt


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-directory", type=Path, required=True)
    parser.add_argument("--tool-resources", type=Path, required=True)
    parser.add_argument("--appimage-plugin", type=Path, required=True)
    parser.add_argument("--work-directory", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "linux":
        raise ValueError("AppImage restoration requires a Linux host.")
    restore(args.build_directory, args.tool_resources, args.appimage_plugin, args.work_directory)


if __name__ == "__main__":
    main()
