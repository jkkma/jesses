"""Stage pinned public SVT forks and complete pinned sources for native packaging.

Both binary and source archive hashes are mandatory. Does not install into the
user's data directory, modify PATH or overwrite an existing staging destination.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import uuid
from urllib.request import Request, urlopen

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
MAX_DOWNLOAD = 512 * 1024 * 1024


def stage_ffmpeg(delivery: Path, tools_root: Path, target: str) -> list:
    spec = importlib.util.spec_from_file_location("desktop_package", ROOT / "scripts/package-desktop.py")
    package = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(package)
    inventory = package.inventory(delivery)
    actual = {record["path"]: record["sha256"] for record in inventory}
    provenance_path = delivery / "build-provenance.json"
    provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
    if provenance.get("schemaVersion") != 1 or provenance.get("target") != target:
        raise ValueError("The FFmpeg build provenance has a different schema or platform.")
    if {tool["id"] for tool in provenance["tools"]} != {"ffmpeg", "ffprobe"} or len(provenance["tools"]) != 2:
        raise ValueError("The media build must contain exactly the FFmpeg and FFprobe pair.")
    if provenance["source"]["sha256"] != "cf38e0e28c7e5605942c4a77755349b0145804a397af37eb1fb4c77cb237f635":
        raise ValueError("The package FFmpeg build has an unexpected official source release.")
    if not provenance.get("additionalSources") or not provenance.get("licenses") or not provenance.get("buildInputs"):
        raise ValueError("The FFmpeg build must include library sources, notices and build inputs.")
    records = [*provenance["tools"], provenance["source"], *provenance["additionalSources"], *provenance["licenses"], *provenance["buildInputs"], *provenance.get("supportFiles", [])]
    expected = {"build-provenance.json": digest(provenance_path)}
    for record in records:
        package.relative_path(record["path"])
        if record["path"] in expected:
            raise ValueError("The FFmpeg delivery has duplicate payload records.")
        expected[record["path"]] = record["sha256"]
    if expected != actual:
        raise ValueError("The FFmpeg delivery inventory differs from its build provenance.")
    for tool in provenance["tools"]:
        executable = delivery / package.relative_path(tool["path"])
        result = subprocess.run([str(executable), "-version"], capture_output=True, text=True, check=True, timeout=20)
        if result.stdout.splitlines()[0] != tool["version"]:
            raise ValueError(f"The staged {tool['id']} identity differs from its build receipt.")
    directory = tools_root / "ffmpeg"
    shutil.copytree(delivery, directory)
    def prefix(record):
        return {**record, "path": "ffmpeg/" + record["path"]}
    common = {"source": prefix(provenance["source"]), "additionalSources": [prefix(record) for record in provenance["additionalSources"]], "licenses": [prefix(record) for record in provenance["licenses"]], "buildInputs": [prefix(record) for record in provenance["buildInputs"]], "buildProvenance": {"path": "ffmpeg/build-provenance.json", "sha256": expected["build-provenance.json"]}, "supportFiles": [prefix(record) for record in provenance.get("supportFiles", [])]}
    return [{**prefix(tool), **common} for tool in provenance["tools"]]


def digest(path: Path) -> str:
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def stage_x264(delivery: Path, tools_root: Path, target: str) -> dict:
    return stage_standalone(delivery, tools_root, target, "x264")


def stage_svt(delivery: Path, tools_root: Path, target: str) -> dict:
    return stage_standalone(delivery, tools_root, target, "svt-av1")


def stage_av1an(delivery: Path, tools_root: Path, target: str) -> dict:
    def module(name, path):
        spec = importlib.util.spec_from_file_location(name, path)
        result = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(result)
        return result
    package = module("desktop_package", ROOT / "scripts/package-desktop.py")
    runtime = module("av1an_delivery", ROOT / "scripts/package-av1an-delivery.py")
    return runtime.stage(delivery, tools_root, target, package.inventory)


def stage_standalone(delivery: Path, tools_root: Path, target: str, identifier: str) -> dict:
    spec = importlib.util.spec_from_file_location("desktop_package", ROOT / "scripts/package-desktop.py")
    package = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(package)
    provenance_path = delivery / "build-provenance.json"
    receipt = json.loads(provenance_path.read_text(encoding="utf-8"))
    media_provenance = tools_root / "ffmpeg/build-provenance.json"
    if receipt.get("schemaVersion") != 1 or receipt.get("target") != target or receipt["tool"]["id"] != identifier:
        raise ValueError("The standalone build has a different identity or platform.")
    if not media_provenance.is_file() or digest(media_provenance) != receipt["sharedMediaProvenanceSha256"]:
        raise ValueError("The standalone build requires its exact shared media source delivery.")
    media = json.loads(media_provenance.read_text(encoding="utf-8"))
    pins = {"x264": ("b35605ace3ddf7c1a5d67a2eb553f034aef41d55", "6a4d3620201074edec84681aeb25ffa1eceaa9451f7347ca1448f532634eed55"), "svt-av1": ("9292ec8e32bce26f781f277ec8739b53426c4300", "fe7e58bbd61040b460373ce7cfe0119311464eddbd6e620d84eeed317f598a64")}
    if (receipt["sourceCommit"], receipt["source"]["sha256"]) != pins[identifier]:
        raise ValueError("The standalone build uses an unexpected source revision.")
    expected_runtime = {"mingw-w64-gcc", "mingw-w64-crt", "mingw-w64-headers", "mingw-w64-winpthreads"} if target == "x86_64-pc-windows-msvc" else set()
    if len(receipt["runtimeSources"]) != len(expected_runtime) or {record["base"] for record in receipt["runtimeSources"]} != expected_runtime:
        raise ValueError("The standalone runtime sources are incomplete.")
    if target == "x86_64-unknown-linux-gnu":
        allowed = {"linux-vdso.so.1", "ld-linux-x86-64.so.2", "libc.so.6", "libm.so.6", "libpthread.so.0", "librt.so.1", "libdl.so.2", "libgcc_s.so.1", "libstdc++.so.6"}
        dependencies = receipt.get("systemDependencies", [])
        if not dependencies or any(name not in allowed for name in dependencies):
            raise ValueError("The standalone Linux tool has an unsupported system dependency contract.")
    shared_sources = [*receipt["runtimeSources"]]
    own_sources = [receipt["source"]] if identifier == "svt-av1" else []
    if identifier == "x264":
        shared_sources.append(receipt["source"])
    if any(record not in media["additionalSources"] for record in shared_sources):
        raise ValueError("The standalone source records differ from the verified shared sources.")
    actual = {record["path"]: record["sha256"] for record in package.inventory(delivery)}
    expected = {"build-provenance.json": digest(provenance_path)}
    for record in [receipt["tool"], *own_sources, *receipt["licenses"], *receipt["buildInputs"]]:
        package.relative_path(record["path"])
        if record["path"] in expected:
            raise ValueError("The standalone delivery repeats a payload.")
        expected[record["path"]] = record["sha256"]
    if not receipt["licenses"] or not receipt["buildInputs"] or actual != expected:
        raise ValueError("The standalone payload inventory differs from its receipt.")
    executable = delivery / receipt["tool"]["path"]
    version = subprocess.check_output([str(executable), "--version"], stderr=subprocess.STDOUT, text=True, timeout=20).strip()
    if identifier == "x264":
        version = version.splitlines()[0]
        correct_identity = version.startswith("x264 0.165.")
    else:
        correct_identity = "v4.2.0" in version and "HDR" not in version and "5fish" not in version
    if version != receipt["tool"]["version"] or not correct_identity:
        raise ValueError("The standalone identity differs from its build receipt.")
    shutil.copytree(delivery, tools_root / identifier)
    def prefix(record, directory):
        return {**record, "path": directory + "/" + record["path"]}
    runtime_notices = [record for record in media["licenses"] if any(record["path"].startswith(f"licenses/{base}/") for base in expected_runtime)]
    return {**prefix(receipt["tool"], identifier), "source": prefix(receipt["source"], identifier if own_sources else "ffmpeg"), "sourceCommit": receipt["sourceCommit"], "additionalSources": [prefix(record, "ffmpeg") for record in receipt["runtimeSources"]], "licenses": [*[prefix(record, identifier) for record in receipt["licenses"]], *[prefix(record, "ffmpeg") for record in runtime_notices]], "buildInputs": [*[prefix(record, identifier) for record in receipt["buildInputs"]], *[prefix(record, "ffmpeg") for record in media["buildInputs"]]], "buildProvenance": {"path": identifier + "/build-provenance.json", "sha256": digest(provenance_path)}}


def download(url: str, expected: str, cache: Path) -> Path:
    cached = cache / expected
    if cached.exists():
        if cached.is_symlink() or not cached.is_file() or digest(cached) != expected:
            raise ValueError(f"The cached upstream archive failed its checksum: {cached}")
        return cached
    temporary = cache / (expected + ".partial-" + uuid.uuid4().hex)
    request = Request(url, headers={"User-Agent": "jesses-package-stager"})
    count = 0
    with urlopen(request, timeout=30) as response, temporary.open("xb") as output:
        while block := response.read(1024 * 1024):
            count += len(block)
            if count > MAX_DOWNLOAD:
                raise ValueError("The upstream archive exceeds its size limit.")
            output.write(block)
    if digest(temporary) != expected:
        raise ValueError(f"Upstream archive checksum mismatch; retained for inspection: {temporary}")
    try:
        os.link(temporary, cached)
    except FileExistsError:
        if digest(cached) != expected:
            raise ValueError(f"A different cached archive appeared: {cached}")
    temporary.unlink()
    return cached


def stage(destination: Path, target: str, cache: Path, ffmpeg_build: Path | None = None, x264_build: Path | None = None, svt_build: Path | None = None, av1an_build: Path | None = None) -> Path:
    host = "x86_64-pc-windows-msvc" if sys.platform == "win32" else "x86_64-unknown-linux-gnu" if sys.platform == "linux" else "unsupported"
    if target != host or platform.machine().lower() not in {"amd64", "x86_64"}:
        raise ValueError("Stage and verify tools on the matching Windows x64 or Linux x64 host.")
    if (x264_build or svt_build or av1an_build) and not ffmpeg_build:
        raise ValueError("Standalone encoders share verified source resources with the FFmpeg build.")
    if sys.platform == "win32":
        import ctypes
        ctypes.windll.kernel32.SetErrorMode(0x0001 | 0x0002 | 0x8000)
    forks_path = ROOT / "scripts/svt-forks.json"
    sources_path = ROOT / "scripts/bundled-tool-sources.json"
    forks = json.loads(forks_path.read_text(encoding="utf-8"))
    sources = json.loads(sources_path.read_text(encoding="utf-8"))
    destination = destination.absolute()
    destination.mkdir(parents=True, exist_ok=False)
    cache = cache.absolute()
    cache.mkdir(parents=True, exist_ok=True)
    tools_root = destination / "resources/tools"
    tools_root.mkdir(parents=True)
    staged = []
    asset_key = "windowsX86_64" if sys.platform == "win32" else "linuxX86_64"
    # Only source-derived pinned license text is used. No separate moving URL is
    # treated as the authoritative license of a previously pinned binary.
    for encoder in forks["encoders"]:
        identifier = encoder["id"]
        asset = encoder["assets"][asset_key]
        source = sources["sources"][identifier]
        directory = tools_root / identifier
        directory.mkdir()
        binary_archive = download(asset["url"], asset["sha256"], cache)
        with tarfile.open(binary_archive) as archive:
            members = archive.getmembers()
            if len(members) != 1 or not members[0].isfile() or members[0].name != asset["entry"] or members[0].size > MAX_DOWNLOAD:
                raise ValueError(f"Unexpected binary archive layout for {identifier}.")
            with archive.extractfile(members[0]) as original, (directory / asset["entry"]).open("xb") as output:
                shutil.copyfileobj(original, output)
        executable = directory / asset["entry"]
        executable.chmod(0o755)
        executable_hash = digest(executable)
        if "executableSha256" in asset and executable_hash != asset["executableSha256"]:
            raise ValueError(f"The extracted executable checksum differs for {identifier}.")
        # No PATH fallback or shell is involved. An unsupported CPU or missing
        # runtime dependency fails this native stage instead of shipping blindly.
        version_result = subprocess.run([str(executable), "--version"], capture_output=True, text=True, timeout=15, check=True)
        version = (version_result.stdout + version_result.stderr).strip()
        if encoder["versionMarker"] not in version:
            raise ValueError(f"The staged executable identity differs for {identifier}: {version}")
        source_archive = download(source["url"], source["sha256"], cache)
        shutil.copy2(source_archive, directory / "source.tar.gz")
        licenses = []
        with tarfile.open(source_archive) as archive:
            members = archive.getmembers()
            for license_name in encoder["licenses"]:
                matches = [member for member in members if member.isfile() and len(Path(member.name).parts) == 2 and Path(member.name).name == license_name]
                if len(matches) != 1 or matches[0].size > 1024 * 1024:
                    raise ValueError(f"Missing or ambiguous source license: {identifier}/{license_name}")
                with archive.extractfile(matches[0]) as original, (directory / license_name).open("xb") as output:
                    shutil.copyfileobj(original, output)
                licenses.append({"path": f"{identifier}/{license_name}", "sha256": digest(directory / license_name)})
        staged.append({"id": identifier, "path": f"{identifier}/{asset['entry']}", "sha256": executable_hash,
                       "version": version, "architecture": asset["architecture"], "archive": asset,
                       "sourceRepository": encoder["repository"], "sourceCommit": encoder["commit"],
                       "source": {**source, "path": f"{identifier}/source.tar.gz"}, "licenses": licenses})
        print(f"Verified package tool: {identifier} ({executable_hash})")
    if ffmpeg_build is not None:
        staged.extend(stage_ffmpeg(ffmpeg_build.absolute(), tools_root, target))
    if x264_build is not None:
        staged.append(stage_x264(x264_build.absolute(), tools_root, target))
    if svt_build is not None:
        staged.append(stage_svt(svt_build.absolute(), tools_root, target))
    if av1an_build is not None:
        staged.append(stage_av1an(av1an_build.absolute(), tools_root, target))
    available = {tool["id"] for tool in staged}
    if "av1an" in available:
        available.update(("VapourSynth", "L-SMASH Works"))
    manifest = {"schemaVersion": 1, "target": target, "tools": staged,
                "binaryManifestSha256": digest(forks_path), "sourceManifestSha256": digest(sources_path),
                "externalTools": [identifier for identifier in ["ffmpeg", "ffprobe", "svt-av1", "x264", "av1an", "VapourSynth", "L-SMASH Works"] if identifier not in available]}
    (tools_root / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8", newline="\n")
    config = destination / "tauri-tools.conf.json"
    config.write_text(json.dumps({"bundle": {"resources": {str(tools_root): "resources/tools/"}}}, indent=2) + "\n", encoding="utf-8", newline="\n")
    return config


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--cache", type=Path, default=ROOT / "target/tool-download-cache")
    parser.add_argument("--target", choices=["x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"], required=True)
    parser.add_argument("--ffmpeg-build", type=Path, help="Verified source-complete FFmpeg delivery directory")
    parser.add_argument("--x264-build", type=Path, help="Verified standalone x264 build sharing that media source delivery")
    parser.add_argument("--svt-build", type=Path, help="Verified mainline SVT-AV1 build sharing that media runtime source delivery")
    parser.add_argument("--av1an-build", type=Path, help="Verified portable av1an engine and source-complete frameserver delivery")
    args = parser.parse_args()
    print(f"Package configuration: {stage(args.destination, args.target, args.cache, args.ffmpeg_build, args.x264_build, args.svt_build, args.av1an_build)}")


if __name__ == "__main__":
    main()
