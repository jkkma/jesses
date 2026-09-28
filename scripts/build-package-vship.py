"""Create a pinned, source-complete optional Vship Vulkan scorer delivery."""

import argparse
import importlib.util
import json
from pathlib import Path
import shutil
import sys
import tarfile
from urllib.request import Request, urlopen

sys.dont_write_bytecode = True
SCRIPTS = Path(__file__).resolve().parent
LOCK = SCRIPTS / "vship-windows-lock.json"
MAX_DOWNLOAD = 8 * 1024 * 1024

spec = importlib.util.spec_from_file_location("vship_support", SCRIPTS / "package-av1an-support.py")
support = importlib.util.module_from_spec(spec)
spec.loader.exec_module(support)


def checked_input(record, cache):
    name = support.member_path(record["path"])
    if len(name.parts) != 1 or not record["url"].startswith("https://"):
        raise ValueError("A Vship pin has an unsafe name or URL.")
    expected = record["sha256"]
    if len(expected) != 64 or any(c not in "0123456789abcdef" for c in expected):
        raise ValueError("A Vship input lacks a SHA256 pin.")
    path = cache / name
    if not path.exists():
        temporary = cache / (name.name + ".partial")
        if temporary.exists():
            raise ValueError(f"Inspect the incomplete Vship download: {temporary}")
        request = Request(record["url"], headers={"User-Agent": "jesses-vship-package"})
        size = 0
        with urlopen(request, timeout=90) as response, temporary.open("xb") as output:
            while chunk := response.read(1024 * 1024):
                size += len(chunk)
                if size > MAX_DOWNLOAD:
                    raise ValueError("The Vship download exceeds its size limit.")
                output.write(chunk)
        if support.digest(temporary) != expected:
            raise ValueError("A downloaded Vship input differs from its SHA256 pin.")
        temporary.rename(path)
    if path.is_symlink() or not path.is_file() or path.stat().st_size > MAX_DOWNLOAD:
        raise ValueError("The cached Vship input is not an ordinary bounded file.")
    if support.digest(path) != expected or (record.get("size") and path.stat().st_size != record["size"]):
        raise ValueError(f"A cached Vship input differs from its pin: {path}")
    return path


def license_from_source(source):
    with tarfile.open(source, "r:gz") as archive:
        names = [member.name for member in archive.getmembers() if member.isfile()]
        if len(names) != len(set(names)) or any(support.member_path(name).parts[0] != "vship" for name in names):
            raise ValueError("The pinned Vship source archive has an unexpected layout.")
        try:
            notice = archive.extractfile("vship/LICENSE")
        except KeyError as error:
            raise ValueError("The pinned Vship source lacks its license.") from error
        if notice is None:
            raise ValueError("The pinned Vship source lacks its license.")
        contents = notice.read(128 * 1024 + 1)
    if not contents or len(contents) > 128 * 1024:
        raise ValueError("The pinned Vship license is empty or oversized.")
    return contents


def build(destination, cache):
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    if lock.get("schemaVersion") != 1 or lock.get("version") != "5.1.1" or lock.get("revision") != "256dc5a85e56e42a88a7a90d641037fdc148b91e":
        raise ValueError("The Vship lock differs from the reviewed source revision.")
    binary = checked_input(lock["binary"], cache)
    source = checked_input(lock["source"], cache)
    license_bytes = license_from_source(source)
    destination.mkdir(parents=True, exist_ok=False)
    delivery = destination / "delivery"
    for name in ("plugins", "sources", "licenses/Vship", "build"):
        (delivery / name).mkdir(parents=True)
    shutil.copy2(binary, delivery / "plugins" / binary.name)
    shutil.copy2(source, delivery / "sources" / source.name)
    (delivery / "licenses/Vship/LICENSE").write_bytes(license_bytes)
    for recipe in (Path(__file__), LOCK):
        shutil.copy2(recipe, delivery / "build" / recipe.name)

    def record(relative):
        return {"path": relative, "sha256": support.digest(delivery / relative)}

    receipt = {
        "schemaVersion": 1,
        "target": "x86_64-pc-windows-msvc",
        "version": lock["version"],
        "sourceRevision": lock["revision"],
        "plugin": {"id": "com.lumen.vship", **record("plugins/" + binary.name)},
        "source": record("sources/" + source.name),
        "licenses": [record("licenses/Vship/LICENSE")],
        "buildInputs": [record("build/" + name) for name in (Path(__file__).name, LOCK.name)],
    }
    (delivery / "build-provenance.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    print(f"Verified pinned Vship source delivery: {delivery}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--cache", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "win32":
        raise SystemExit("This Vship binary recipe targets Windows x64.")
    cache = args.cache.resolve(strict=True)
    build(args.destination.absolute(), cache)


if __name__ == "__main__":
    main()
