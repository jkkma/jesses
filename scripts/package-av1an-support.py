"""Shared bounded extraction and compiler helpers for the Windows av1an bundle."""

import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import stat
import subprocess
import tarfile
import zipfile


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def member_path(name):
    path = PurePosixPath(name)
    if not path.parts or path.is_absolute() or ".." in path.parts or "\\" in name or any(":" in part for part in path.parts):
        raise ValueError(f"Unsafe archive member: {name}")
    return path


def unpack(archive_path, destination, *, single_root=True):
    destination.mkdir(parents=True, exist_ok=False)
    names = set()
    roots = set()
    if zipfile.is_zipfile(archive_path):
        with zipfile.ZipFile(archive_path) as archive:
            members = archive.infolist()
            if len(members) > 100_000 or sum(member.file_size for member in members) > 1024**3:
                raise ValueError("Archive exceeds its extraction bounds.")
            for member in members:
                path = member_path(member.filename)
                key = path.as_posix().casefold()
                if key in names or stat.S_ISLNK(member.external_attr >> 16):
                    raise ValueError("Archive contains duplicate names or a symbolic link.")
                names.add(key)
                roots.add(path.parts[0])
            archive.extractall(destination)
    else:
        with tarfile.open(archive_path) as archive:
            members = archive.getmembers()
            if len(members) > 100_000 or sum(member.size for member in members) > 1024**3:
                raise ValueError("Archive exceeds its extraction bounds.")
            regular = []
            for member in members:
                path = member_path(member.name)
                key = path.as_posix().casefold()
                if member.isfile() and key in names:
                    raise ValueError("Archive repeats a file.")
                names.add(key)
                roots.add(path.parts[0])
                if member.isfile() or member.isdir():
                    regular.append(member)
                elif member.issym() and "/doc/example/misc/" in member.name:
                    # These zimg example links are not compiled. The complete
                    # original source archive, including them, is distributed.
                    continue
                else:
                    raise ValueError(f"Unexpected non-regular source member: {member.name}")
            archive.extractall(destination, members=regular, filter="data")
    if single_root:
        if len(roots) != 1:
            raise ValueError("The source archive must contain one root directory.")
        return destination / roots.pop()
    return destination


def isolated_environment():
    allowed = {"SYSTEMROOT", "WINDIR", "COMSPEC", "TEMP", "TMP", "USERPROFILE", "HOMEDRIVE", "HOMEPATH", "PROCESSOR_ARCHITECTURE", "NUMBER_OF_PROCESSORS", "PROGRAMFILES", "PROGRAMFILES(X86)", "PROGRAMW6432", "PATHEXT"}
    env = {key: value for key, value in os.environ.items() if key.upper() in allowed}
    env.update(SOURCE_DATE_EPOCH="1789257600", LC_ALL="C", TZ="UTC", PYTHONDONTWRITEBYTECODE="1", GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_SYSTEM=os.devnull)
    return env


def system32():
    return Path(os.environ["SystemRoot"]) / "System32"


def msvc_environment(compiler, python, msvc_root=None, sdk_root=None, sdk_version=None):
    if msvc_root is None:
        vswhere = Path(os.environ["ProgramFiles(x86)"]) / "Microsoft Visual Studio/Installer/vswhere.exe"
        installation = subprocess.check_output([str(vswhere), "-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"], text=True, timeout=20).strip()
        if not installation:
            raise ValueError("A native MSVC x64 compiler is required for VapourSynth.")
        version = (Path(installation) / "VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt").read_text().strip()
        msvc_root = Path(installation) / "VC/Tools/MSVC" / version
    msvc_root = msvc_root.resolve(strict=True)
    sdk_root = (sdk_root or Path(os.environ["ProgramFiles(x86)"]) / "Windows Kits/10").resolve(strict=True)
    if sdk_version is None:
        versions = [path.name for path in (sdk_root / "Include").iterdir() if (path / "um/Windows.h").is_file()]
        sdk_version = max(versions, key=lambda name: tuple(map(int, name.split("."))))
    if any(part in sdk_version for part in ("/", "\\", ":", "..")):
        raise ValueError("Invalid SDK version.")
    cl = msvc_root / "bin/HostX64/x64/cl.exe"
    if not cl.is_file() or not (sdk_root / "Lib" / sdk_version / "um/x64/kernel32.lib").is_file():
        raise ValueError("The selected Windows compiler or SDK is incomplete.")
    env = isolated_environment()
    env["PATH"] = os.pathsep.join(map(str, [python.parent, cl.parent, sdk_root / "bin" / sdk_version / "x64", compiler / "ucrt64/bin", compiler / "usr/bin", system32()]))
    env["INCLUDE"] = os.pathsep.join(map(str, [msvc_root / "include", *[sdk_root / "Include" / sdk_version / name for name in ("ucrt", "shared", "um", "winrt")]]))
    env["LIB"] = os.pathsep.join(map(str, [msvc_root / "lib/x64", sdk_root / "Lib" / sdk_version / "ucrt/x64", sdk_root / "Lib" / sdk_version / "um/x64"]))
    env["PYTHONPATH"] = str(compiler / "ucrt64/lib/python3.14/site-packages")
    version = subprocess.run([str(cl)], env=env, capture_output=True, text=True, timeout=20)
    return env, cl, {"msvcVersion": msvc_root.name, "sdkVersion": sdk_version, "compilerSha256": digest(cl), "compilerBanner": (version.stdout + version.stderr).splitlines()[0]}


def run(command, directory, env, log, timeout=1800):
    subprocess.run(list(map(str, command)), cwd=directory, env=env, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=timeout)


def inventory(directory):
    records = []
    for path in sorted(directory.rglob("*")):
        if path.is_symlink() or path.is_junction():
            raise ValueError("A package payload cannot contain filesystem redirections.")
        if path.is_file():
            records.append({"path": path.relative_to(directory).as_posix(), "sha256": digest(path)})
    return records


def source_notices(archive_path, destination):
    destination.mkdir(parents=True, exist_ok=False)
    records = []
    def preserve(name, data):
        member_path(name)
        basename = PurePosixPath(name).name.upper()
        if len(data) > 1024 * 1024 or not basename.startswith(("LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE", "PATENT", "UNLICENSE")):
            return
        path = destination / (hashlib.sha256(name.encode()).hexdigest()[:12] + "-" + PurePosixPath(name).name)
        path.write_bytes(data)
        records.append(path)
    if zipfile.is_zipfile(archive_path):
        with zipfile.ZipFile(archive_path) as archive:
            for member in archive.infolist():
                if not member.is_dir() and member.file_size <= 1024 * 1024:
                    preserve(member.filename, archive.read(member))
    else:
        with tarfile.open(archive_path) as archive:
            for member in archive:
                if member.isfile() and member.size <= 1024 * 1024:
                    preserve(member.name, archive.extractfile(member).read())
    return records


def archive_directory(source, output):
    import gzip
    with output.open("xb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", filename="", mtime=1789257600) as compressed, tarfile.open(fileobj=compressed, mode="w|") as archive:
        for path in sorted(source.rglob("*")):
            if path.is_symlink() or path.is_junction():
                raise ValueError("Vendored source contains a redirection.")
            info = archive.gettarinfo(str(path), arcname=path.relative_to(source).as_posix())
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            info.mtime = 1789257600
            info.mode = 0o755 if path.is_dir() else 0o644
            if path.is_file():
                with path.open("rb") as stream:
                    archive.addfile(info, stream)
            else:
                archive.addfile(info)
