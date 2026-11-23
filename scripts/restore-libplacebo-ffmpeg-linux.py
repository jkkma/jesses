"""Incrementally qualify CPU FFmpeg + Vulkan processing from a retained build.

Preserves the prior delivery. Reuses only the explicitly named retained static
prefix and FFmpeg object tree; records every reused prefix file and copies the
original corresponding sources/notices into the new delivery. No other build
directory or user cache is removed.
"""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys
import hashlib
import tarfile

ROOT = Path(__file__).resolve().parents[1]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / filename)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--previous-build", type=Path, required=True)
    parser.add_argument("--build-directory", type=Path, required=True)
    parser.add_argument("--cache", type=Path, default=ROOT / "target/tool-source-cache")
    parser.add_argument("--jobs", type=int, default=2)
    parser.add_argument("--dependencies-only", action="store_true")
    parser.add_argument("--resume", action="store_true", help="Resume only this owned, verified dependency build.")
    args = parser.parse_args()
    if sys.version_info < (3, 14) or sys.platform != "linux" or not 1 <= args.jobs <= 8:
        raise SystemExit("Linux, Python3.14+ and 1–8 build jobs are required.")
    recipe = module("linux_ffmpeg", "build-package-ffmpeg-linux.py")
    vulkan = module("vulkan_processing", "build-package-libplacebo.py")
    previous = args.previous_build.resolve(strict=True)
    retained = previous / "delivery"
    provenance = json.loads((retained / "build-provenance.json").read_text())
    if provenance["target"] != recipe.TARGET:
        raise ValueError("The retained FFmpeg delivery targets another platform.")
    for item in [provenance["source"], *provenance["tools"], *provenance["additionalSources"],
                 *provenance["licenses"], *provenance["buildInputs"]]:
        path = retained / recipe.relative(item["path"])
        if path.is_symlink() or not path.is_file() or recipe.digest(path) != item["sha256"]:
            raise ValueError(f"A retained FFmpeg delivery input changed: {path}")
    lock = json.loads(recipe.LOCK.read_text())
    ffmpeg = lock["ffmpeg"]
    ffmpeg_source = previous / "work/ffmpeg" / ffmpeg["sourceRoot"]
    version_file = ffmpeg_source / "RELEASE"
    if (provenance["source"]["sha256"] != ffmpeg["sha256"] or not version_file.is_file()
            or version_file.read_text().strip() != ffmpeg["version"]):
        raise ValueError("The retained FFmpeg build differs from the current source lock.")
    locked_dependencies = [*lock["libraries"], *lock["buildTools"]]
    retained_sources = {(item["url"], item["sha256"]): item for item in provenance["additionalSources"]}
    reused_sources = [provenance["source"]]
    for item in locked_dependencies:
        key = item["url"], item["sha256"]
        if key not in retained_sources:
            raise ValueError(f"The retained static prefix lacks locked source provenance for {item['id']}.")
        reused_sources.append(retained_sources[key])
    locked_notices = {f"licenses/{item.get('id', 'ffmpeg')}/{name.replace('/', '-')}"
                      for item in [ffmpeg, *locked_dependencies] for name in item["licenseFiles"]}
    reused_licenses = [item for item in provenance["licenses"] if item["path"] in locked_notices]
    if {item["path"] for item in reused_licenses} != locked_notices:
        raise ValueError("The retained static prefix lacks locked FFmpeg or dependency notices.")
    if args.resume and not (args.build_directory / "delivery/build/reused-static-prefix.json").is_file():
        raise ValueError("The requested directory is not an owned retained-prefix build.")
    builder = recipe.Builder(args.build_directory, args.cache, args.jobs, resume=args.resume)
    if args.resume:
        for line in (builder.directory / "build.log").read_text().splitlines():
            if line.startswith('{"argv":'):
                builder.commands.append(json.loads(line))
    builder.prefix = previous / "prefix"
    if builder.prefix.is_symlink() or not builder.prefix.is_dir():
        raise ValueError("The retained static prefix is absent or a symlink.")
    prefix_files = []
    for path in sorted(builder.prefix.rglob("*")):
        if path.is_symlink() and not path.resolve(strict=True).is_relative_to(builder.prefix):
            raise ValueError("A retained static-prefix symlink escapes the owned prefix.")
        if path.is_file():
            prefix_files.append({"path": path.relative_to(builder.prefix).as_posix(), "sha256": recipe.digest(path),
                                 **({"symlink": os.readlink(path)} if path.is_symlink() else {})})
    if not args.resume:
        (builder.delivery / "build/reused-static-prefix.json").write_text(json.dumps(prefix_files, indent=2) + "\n")
    for item in [*reused_sources, *reused_licenses]:
        relative = recipe.relative(item["path"])
        target = builder.delivery / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(retained / relative, target)
    builder.sources = reused_sources
    builder.licenses = reused_licenses
    builder.env.update({
        "CPPFLAGS": f"-I{builder.prefix}/include", "LDFLAGS": f"-L{builder.prefix}/lib",
        "PKG_CONFIG_LIBDIR": f"{builder.prefix}/lib/pkgconfig:{builder.prefix}/share/pkgconfig",
        "PATH": f"{builder.prefix}/bin" + os.pathsep + builder.env["PATH"],
    })
    meson = list((previous / "work/meson").glob("*/meson.py"))
    if len(meson) != 1:
        raise ValueError("Exactly one retained pinned Meson source is required.")
    builder.meson = [sys.executable, str(meson[0])]
    if args.resume:
        records = {}
        for item in json.loads(vulkan.LOCK.read_text())["libraries"]:
            archive = recipe.download(item, builder.cache)
            source = builder.work / item["id"] / item["sourceRoot"]
            with tarfile.open(archive) as bundle:
                for member in bundle:
                    if not member.isfile():
                        continue
                    path = source.parent / recipe.relative(member.name)
                    if path.is_symlink() or not path.is_file():
                        raise ValueError(f"A retained processing source is absent or a symlink: {path}")
                    with bundle.extractfile(member) as original:
                        contents = original.read()
                    if path.read_bytes() != contents:
                        raise ValueError(f"A retained processing source changed: {path}")
            packaged = builder.delivery / "sources" / item["filename"]
            if packaged.is_symlink() or recipe.digest(packaged) != item["sha256"]:
                raise ValueError("A retained corresponding-source archive changed.")
            records[item["id"]] = source
            builder.sources.append(recipe.record(packaged, builder.delivery, url=item["url"]))
            for name in item["licenseFiles"]:
                notice = builder.delivery / "licenses" / item["id"] / name.replace("/", "-")
                if notice.read_bytes() != (source / name).read_bytes():
                    raise ValueError("A retained processing license notice changed.")
                builder.licenses.append(recipe.record(notice, builder.delivery))
    else:
        records = vulkan.sources(builder)
    vulkan.build(builder, records)
    for name in ("restore-libplacebo-ffmpeg-linux.py", "build-package-libplacebo.py", "package-libplacebo-lock.json"):
        shutil.copy2(ROOT / "scripts" / name, builder.delivery / "build" / name)
    if args.dependencies_only:
        print(f"Static Vulkan processing dependencies built: {builder.prefix}", flush=True)
        return
    tools, flags = builder.ffmpeg(ffmpeg_source, ffmpeg["version"])
    builder.finish(lock, tools, flags)


if __name__ == "__main__":
    main()
