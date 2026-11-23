"""Install the verified Windows x64 Subtitle Edit and Tesseract OCR tools."""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import zipfile

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent


def load(name):
    spec = importlib.util.spec_from_file_location(name, HERE / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


STAGE = load("stage-bundled-tools")
ARCHIVE = load("build-package-mkvmerge")


def install_seconv(pin, destination, cache):
    source = STAGE.download(pin["url"], pin["sha256"], cache)
    with zipfile.ZipFile(source) as archive:
        entries = archive.infolist()
        if len(entries) > 20_000 or sum(entry.file_size for entry in entries) > ARCHIVE.MAX_DOWNLOAD:
            raise ValueError("The SeConv archive exceeds its extraction bounds.")
        members = set()
        for entry in entries:
            name = entry.filename.rstrip("/")
            ARCHIVE.member_path(name)
            if name.casefold() in members or stat.S_ISLNK(entry.external_attr >> 16):
                raise ValueError(f"Invalid SeConv archive entry: {name}")
            members.add(name.casefold())
        archive.extractall(destination)


def install_tesseract(lock, destination, cache):
    destination.mkdir()
    for package in lock["packages"]:
        source = STAGE.download(package["url"], package["sha256"], cache)
        with tarfile.open(source) as archive:
            members = ARCHIVE.checked_members(archive)
            for record in package["files"]:
                member = record["member"]
                name = ARCHIVE.member_path(member).name
                if member != f"ucrt64/bin/{name}" or Path(name).suffix.lower() not in {".dll", ".exe"}:
                    raise ValueError(f"Invalid Tesseract archive entry: {member}")
                target = destination / name
                ARCHIVE.write_bytes(target, ARCHIVE.ordinary_member(archive, members, member))
                if ARCHIVE.digest(target) != record["sha256"]:
                    raise ValueError(f"The Tesseract runtime file changed: {name}")
    pin = lock["english"]
    models = destination / "tessdata"
    models.mkdir()
    shutil.copy2(STAGE.download(pin["url"], pin["sha256"], cache), models / "eng.traineddata")


def qualify(root, lock):
    tesseract = root / "tesseract"
    environment = ARCHIVE.runtime_environment(tesseract)
    environment["TESSDATA_PREFIX"] = str(tesseract / "tessdata")

    def output(executable, *arguments):
        result = subprocess.run([str(executable), *arguments], env=environment,
                                cwd=root, capture_output=True, text=True, check=True, timeout=20)
        return result.stdout + result.stderr

    seconv = output(root / "seconv/seconv.exe", "--version").strip()
    version = output(tesseract / "tesseract.exe", "--version").splitlines()[0]
    expected = re.escape(lock["tesseractVersion"])
    if seconv != lock["seconv"]["version"] or not re.fullmatch(rf"tesseract v?{expected}", version):
        raise ValueError("The OCR executables reported unexpected versions.")
    if "eng" not in output(tesseract / "tesseract.exe", "--list-langs").splitlines():
        raise ValueError("Tesseract did not discover the installed English model.")


def main():
    if sys.platform != "win32":
        raise SystemExit("This OCR recipe requires Windows x64.")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, default=Path(os.environ["LOCALAPPDATA"]) / "jesses/tools/subtitle-ocr")
    parser.add_argument("--cache", type=Path, default=HERE.parent / "target/tool-download-cache")
    args = parser.parse_args()
    destination = args.destination.absolute()
    if destination.exists() or destination.is_symlink() or destination.is_junction():
        raise FileExistsError(f"The OCR destination already exists: {destination}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    cache = args.cache.absolute()
    cache.mkdir(parents=True, exist_ok=True)
    lock = json.loads((HERE / "subtitle-ocr-windows-lock.json").read_text(encoding="utf-8"))
    if lock.get("schemaVersion") != 1:
        raise ValueError("Unsupported OCR dependency lock.")
    with tempfile.TemporaryDirectory(prefix=".jesses-subtitle-ocr-", dir=destination.parent) as temporary:
        outgoing = Path(temporary) / "subtitle-ocr"
        outgoing.mkdir()
        install_seconv(lock["seconv"], outgoing / "seconv", cache)
        install_tesseract(lock, outgoing / "tesseract", cache)
        qualify(outgoing, lock)
        outgoing.rename(destination)
    print(f"Installed verified OCR tools: {destination}")


if __name__ == "__main__":
    main()
