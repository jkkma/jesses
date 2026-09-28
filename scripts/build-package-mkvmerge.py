"""Build a source-complete Windows mkvmerge delivery from pinned MSYS2 packages.

The lock names every executable/DLL and the source-only package for each
binary package. Nothing is installed into Windows or resolved from PATH.
Only ordinary, explicitly selected archive members enter the delivery.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import uuid
from urllib.request import Request, urlopen

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
LOCK = ROOT / "scripts/package-mkvmerge-windows-lock.json"
MAX_DOWNLOAD = 512 * 1024 * 1024
MAX_MEMBER = 128 * 1024 * 1024
SYSTEM_IMPORTS = {
    "advapi32.dll", "authz.dll", "bcrypt.dll", "kernel32.dll", "mpr.dll",
    "netapi32.dll", "ntdll.dll", "ole32.dll", "oleaut32.dll", "shell32.dll",
    "user32.dll", "userenv.dll", "version.dll", "winmm.dll", "ws2_32.dll",
}


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def member_path(name: str) -> Path:
    parts = PurePosixPath(name).parts
    if (not name or "\\" in name or ":" in name or name.startswith("/") or
            any(part in {"", ".", ".."} for part in name.split("/"))):
        raise ValueError(f"Unsafe package member: {name}")
    return Path(*parts)


def checked_members(archive: tarfile.TarFile, *, limit: int = 100_000) -> dict:
    result = {}
    folded = set()
    for member in archive:
        member_path(member.name.rstrip("/"))
        key = member.name.casefold()
        if key in folded or len(result) >= limit or member.size > MAX_DOWNLOAD:
            raise ValueError(f"Ambiguous or oversized package member: {member.name}")
        folded.add(key)
        result[member.name] = member
    return result


def ordinary_member(archive: tarfile.TarFile, members: dict, name: str, limit: int = MAX_MEMBER) -> bytes:
    member_path(name)
    member = members.get(name)
    if member is None or not member.isfile() or member.size > limit:
        raise ValueError(f"Missing or unsafe ordinary package member: {name}")
    with archive.extractfile(member) as stream:
        data = stream.read(limit + 1)
    if len(data) != member.size:
        raise ValueError(f"Truncated package member: {name}")
    return data


def download(record: dict, cache: Path) -> Path:
    filename = member_path(record["filename"])
    if len(filename.parts) != 1 or not record["url"].startswith("https://repo.msys2.org/mingw/"):
        raise ValueError("An MKVToolNix input has an unexpected source URL or filename.")
    path = cache / filename
    if path.exists():
        if path.is_symlink() or not path.is_file() or digest(path) != record["sha256"]:
            raise ValueError(f"A cached MKVToolNix input changed: {path}")
        return path
    temporary = cache / (filename.name + ".partial-" + uuid.uuid4().hex)
    size = 0
    try:
        request = Request(record["url"], headers={"User-Agent": "jesses-mkvmerge-packager"})
        with urlopen(request, timeout=90) as response, temporary.open("xb") as output:
            while block := response.read(1024 * 1024):
                size += len(block)
                if size > MAX_DOWNLOAD:
                    raise ValueError("An MKVToolNix input exceeds its download limit.")
                output.write(block)
        if digest(temporary) != record["sha256"]:
            raise ValueError(f"An upstream MKVToolNix input failed its pinned checksum: {record['filename']}")
        try:
            os.link(temporary, path)
        except FileExistsError:
            if digest(path) != record["sha256"]:
                raise ValueError(f"A different cached MKVToolNix input appeared: {path}")
        return path
    finally:
        # A failed download is not trusted as a cache entry.
        temporary.unlink(missing_ok=True)


def pe_imports(path: Path) -> list[str]:
    """Read PE import and delay-import tables without a host toolchain."""
    data = path.read_bytes()

    def number(offset: int, fmt: str) -> int:
        size = struct.calcsize(fmt)
        if offset < 0 or offset + size > len(data):
            raise ValueError(f"Truncated PE binary: {path.name}")
        return struct.unpack_from(fmt, data, offset)[0]

    if len(data) < 0x40 or data[:2] != b"MZ":
        raise ValueError(f"Expected a Windows PE binary: {path.name}")
    pe = number(0x3C, "<I")
    if data[pe:pe + 4] != b"PE\0\0" or number(pe + 4, "<H") != 0x8664:
        raise ValueError(f"Expected an x64 PE binary: {path.name}")
    sections = number(pe + 6, "<H")
    optional_size = number(pe + 20, "<H")
    opt = pe + 24
    if number(opt, "<H") != 0x20B or optional_size < 112 + 14 * 8:
        raise ValueError(f"Unsupported PE optional header: {path.name}")
    image_base = number(opt + 24, "<Q")
    header_size = number(opt + 60, "<I")
    section_start = opt + optional_size
    segments = []
    for index in range(sections):
        section = section_start + index * 40
        virtual_size = number(section + 8, "<I")
        virtual_address = number(section + 12, "<I")
        raw_size = number(section + 16, "<I")
        raw_address = number(section + 20, "<I")
        segments.append((virtual_address, max(virtual_size, raw_size), raw_address, raw_size))

    def mapped(rva: int, size: int = 1) -> int:
        if rva < header_size and rva + size <= len(data):
            return rva
        for start, length, raw, raw_size in segments:
            if start <= rva and rva + size <= start + length and rva + size <= start + raw_size:
                offset = raw + rva - start
                if offset + size <= len(data):
                    return offset
        raise ValueError(f"Invalid PE import address in {path.name}")

    def cstring(rva: int) -> str:
        offset = mapped(rva)
        end = data.find(b"\0", offset, min(offset + 260, len(data)))
        if end < 0:
            raise ValueError(f"Unterminated PE DLL name in {path.name}")
        return data[offset:end].decode("ascii")

    if number(opt + 108, "<I") < 14:
        raise ValueError(f"PE import directories are unavailable: {path.name}")
    imports = set()
    for index, descriptor_size, name_field in ((1, 20, 12), (13, 32, 4)):
        directory = opt + 112 + index * 8
        rva = number(directory, "<I")
        size = number(directory + 4, "<I")
        if not rva:
            continue
        if size > 65536:
            raise ValueError(f"PE import directory is unreasonably large: {path.name}")
        for position in range(0, size, descriptor_size):
            descriptor = mapped(rva + position, descriptor_size)
            if not any(data[descriptor:descriptor + descriptor_size]):
                break
            name_rva = number(descriptor + name_field, "<I")
            if index == 13 and not (number(descriptor, "<I") & 1):
                name_rva -= image_base
            imports.add(cstring(name_rva))
        else:
            raise ValueError(f"Unterminated PE import directory: {path.name}")
    return sorted(imports, key=str.lower)


def verify_imports(files: list[dict], directory: Path) -> list[dict]:
    names = {record["path"].lower() for record in files}
    if len(names) != len(files) or "mkvmerge.exe" not in names:
        raise ValueError("The MKVToolNix runtime file set is incomplete or colliding.")
    imports = []
    for record in files:
        path = directory / member_path(record["path"])
        found = pe_imports(path)
        if found != record["imports"]:
            raise ValueError(f"The PE import table changed for {record['path']}.")
        missing = [name for name in found if name.lower() not in names | SYSTEM_IMPORTS and not name.lower().startswith(("api-ms-win-", "ext-ms-win-"))]
        if missing:
            raise ValueError(f"Unpackaged MKVToolNix runtime dependency in {record['path']}: {missing}")
        imports.append({"path": record["path"], "imports": found})
    return imports


def runtime_environment(directory: Path) -> dict:
    root = os.environ.get("SystemRoot")
    if not root:
        raise ValueError("Windows SystemRoot is unavailable.")
    keep = {"SYSTEMROOT", "WINDIR", "TEMP", "TMP", "USERPROFILE", "HOMEDRIVE", "HOMEPATH"}
    env = {key: value for key, value in os.environ.items() if key.upper() in keep}
    env["PATH"] = str(directory) + os.pathsep + str(Path(root) / "System32")
    env["QT_PLUGIN_PATH"] = str(directory / "no-plugins")
    return env


def qualify(directory: Path, scratch: Path, version: str) -> str:
    scratch.mkdir(parents=True, exist_ok=False)
    executable = directory / "mkvmerge.exe"
    env = runtime_environment(directory)
    identity = subprocess.run([str(executable), "--version"], cwd=scratch, env=env, capture_output=True, text=True, check=True, timeout=20)
    if identity.stdout.strip() != version:
        raise ValueError("The packaged mkvmerge version differs from the lock.")
    subtitle = scratch / "sample.srt"
    subtitle.write_text("1\n00:00:00,000 --> 00:00:00,750\nsource backed mux\n", encoding="utf-8")
    output = scratch / "sample.mkv"
    mux = subprocess.run([str(executable), "-o", str(output), str(subtitle)], cwd=scratch, env=env, capture_output=True, text=True, timeout=30)
    if mux.returncode != 0 or not output.is_file() or output.stat().st_size < 500:
        raise ValueError(f"The isolated mkvmerge mux failed: {mux.stdout}\n{mux.stderr}")
    identify = subprocess.run([str(executable), "--identify", "--identification-format", "json", str(output)], cwd=scratch, env=env, capture_output=True, text=True, check=True, timeout=20)
    identified = json.loads(identify.stdout)
    tracks = identified.get("tracks", [])
    if identified.get("container", {}).get("type") != "Matroska" or len(tracks) != 1 or tracks[0].get("type") != "subtitles":
        raise ValueError("The isolated mkvmerge Matroska identify result was unexpected.")
    return f"{version}\n{mux.stdout}{mux.stderr}\nidentify: {json.dumps(identified, sort_keys=True)}\n"


def write_bytes(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("xb") as output:
        output.write(data)


def records(directory: Path) -> list[dict]:
    result = []
    for path in sorted(directory.rglob("*")):
        if path.is_symlink() or path.is_junction():
            raise ValueError(f"A delivery contains a redirected entry: {path}")
        if path.is_file():
            result.append({"path": path.relative_to(directory).as_posix(), "sha256": digest(path)})
    return result


def build(destination: Path, cache: Path) -> Path:
    if sys.platform != "win32":
        raise ValueError("The mkvmerge delivery requires a matching Windows x64 host.")
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    if lock.get("schemaVersion") != 1 or lock.get("target") != "x86_64-pc-windows-msvc" or lock.get("version") != "97.0":
        raise ValueError("Unsupported MKVToolNix lock.")
    packages = {item["name"]: item for item in lock["packages"]}
    sources = {item["base"]: item for item in lock["sources"]}
    if len(packages) != len(lock["packages"]) or len(sources) != len(lock["sources"]):
        raise ValueError("Duplicate MKVToolNix package or source records.")
    if {(item["base"], item["version"]) for item in packages.values()} != {(item["base"], item["version"]) for item in sources.values()}:
        raise ValueError("Every runtime binary package must have its matching source-only package.")
    if len(lock["files"]) != 23 or len(packages) != 21 or len(sources) != 20:
        raise ValueError("The reviewed MKVToolNix dependency closure changed.")
    destination = destination.absolute()
    destination.mkdir(parents=True, exist_ok=False)
    cache = cache.absolute()
    cache.mkdir(parents=True, exist_ok=True)
    with ThreadPoolExecutor(max_workers=4) as pool:
        package_paths = dict(zip(packages, pool.map(lambda item: download(item, cache), packages.values()), strict=True))
        source_paths = dict(zip(sources, pool.map(lambda item: download(item, cache), sources.values()), strict=True))

    delivery = destination / "delivery"
    delivery.mkdir()
    licenses = []
    package_files = {}
    for name, record in packages.items():
        with tarfile.open(package_paths[name]) as archive:
            members = checked_members(archive)
            metadata = ordinary_member(archive, members, ".PKGINFO", 256 * 1024).decode("utf-8")
            if f"pkgname = {name}\n" not in metadata or f"pkgver = {record['version']}\n" not in metadata:
                raise ValueError(f"MSYS2 package identity changed: {name}")
            for tool in (item for item in lock["files"] if item["package"] == name):
                if tool["member"] != "ucrt64/bin/" + tool["path"] or "/" in tool["path"] or "\\" in tool["path"]:
                    raise ValueError("The mkvmerge executable/DLL placement is invalid.")
                output = delivery / tool["path"]
                write_bytes(output, ordinary_member(archive, members, tool["member"]))
                if digest(output) != tool["sha256"]:
                    raise ValueError(f"MSYS2 runtime file changed: {tool['path']}")
                package_files[tool["path"]] = name
            prefix = "ucrt64/share/licenses/"
            for member_name, member in sorted(members.items()):
                if member_name.startswith(prefix) and member.isfile():
                    relative = member_name[len(prefix):]
                    member_path(relative)
                    output = delivery / "licenses" / relative
                    write_bytes(output, ordinary_member(archive, members, member_name, 2 * 1024 * 1024))
                    licenses.append({"path": output.relative_to(delivery).as_posix(), "sha256": digest(output)})
    if set(package_files) != {item["path"] for item in lock["files"]}:
        raise ValueError("A locked mkvmerge runtime file was not delivered.")
    imports = verify_imports(lock["files"], delivery)

    source_records = []
    for base, record in sources.items():
        source = source_paths[base]
        with tarfile.open(source) as archive:
            members = checked_members(archive)
            ordinary_member(archive, members, f"{base}/PKGBUILD", 2 * 1024 * 1024)
            upstream = members.get(record["requiredSourceMember"])
            if upstream is None or not upstream.isfile() or upstream.size < 20_000:
                raise ValueError(f"The complete upstream source is missing from {base}.")
        output = delivery / "sources" / record["filename"]
        output.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, output)
        source_records.append({**record, "path": output.relative_to(delivery).as_posix()})
    for notice in lock["extraNotices"]:
        source = source_paths[notice["base"]]
        with tarfile.open(source) as package:
            members = checked_members(package)
            inner_name = f"{notice['base']}/{notice['innerArchive']}"
            data = ordinary_member(package, members, inner_name)
        with tarfile.open(fileobj=io.BytesIO(data)) as archive:
            inner_members = checked_members(archive)
            for name in notice["members"]:
                output = delivery / "licenses" / notice["base"] / PurePosixPath(name).name
                write_bytes(output, ordinary_member(archive, inner_members, name, 2 * 1024 * 1024))
                licenses.append({"path": output.relative_to(delivery).as_posix(), "sha256": digest(output)})

    build_dir = delivery / "build"
    build_dir.mkdir()
    for path in (LOCK, Path(__file__)):
        shutil.copy2(path, build_dir / path.name)
    build_inputs = records(build_dir)
    build_inputs = [{**record, "path": "build/" + record["path"]} for record in build_inputs]
    runtime = [{"path": item["path"], "sha256": item["sha256"], "package": item["package"]} for item in lock["files"]]
    tool = next(item for item in runtime if item["path"] == "mkvmerge.exe")
    tool = {**tool, "id": "mkvmerge", "version": lock["versionLine"]}
    support_files = [item for item in runtime if item["path"] != "mkvmerge.exe"]
    source = next(item for item in source_records if item["base"] == "mingw-w64-mkvtoolnix")
    additional_sources = [item for item in source_records if item["base"] != "mingw-w64-mkvtoolnix"]
    qualification_log = qualify(delivery, destination / "qualification", lock["versionLine"])
    write_bytes(delivery / "qualification/native-mux-and-identify.log", qualification_log.encode("utf-8"))
    qualification_files = [{"path": "qualification/native-mux-and-identify.log", "sha256": digest(delivery / "qualification/native-mux-and-identify.log")}]
    receipt = {"schemaVersion": 1, "target": lock["target"], "tool": tool, "source": source,
               "additionalSources": additional_sources, "licenses": licenses, "buildInputs": build_inputs,
               "supportFiles": support_files, "qualificationFiles": qualification_files,
               "binaryPackages": list(packages.values()), "nativeDependencies": imports,
               "lockSha256": digest(LOCK), "buildRecipeSha256": digest(Path(__file__)),
               "sourcePolicy": "Each staged MSYS2 binary package has its exact-version source-only package, including upstream source and PKGBUILD. Only the locked CLI and recursive PE DLL closure are staged; no host DLL or tool is copied."}
    write_bytes(delivery / "build-provenance.json", (json.dumps(receipt, indent=2) + "\n").encode("utf-8"))
    print(f"Verified source-complete mkvmerge delivery: {delivery}", flush=True)
    return delivery


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--cache", type=Path, default=ROOT / "target/tool-download-cache")
    args = parser.parse_args()
    build(args.destination, args.cache)


if __name__ == "__main__":
    main()
