"""Build standalone x264 against a verified, source-complete Windows FFmpeg delivery.

The delivery already retains the identical x264 and compiler-runtime sources.
The resulting x264 receipt references those shared sources by exact hashes.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tarfile

sys.dont_write_bytecode = True
COMMIT = "b35605ace3ddf7c1a5d67a2eb553f034aef41d55"
SOURCE_SHA256 = "6a4d3620201074edec84681aeb25ffa1eceaa9451f7347ca1448f532634eed55"
CONFIGURE = ["--enable-static", "--enable-strip", "--disable-lavf", "--disable-swscale", "--disable-avs", "--disable-opencl", "--bit-depth=all", "--chroma-format=all", "--extra-ldflags=-static -static-libgcc"]


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--ffmpeg-build", type=Path, required=True)
    parser.add_argument("--msys-root", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "win32":
        raise SystemExit("This driver currently builds Windows x64 standalone x264.")
    media = args.ffmpeg_build.resolve(strict=True)
    compiler = args.msys_root.resolve(strict=True)
    provenance = json.loads((media / "build-provenance.json").read_text(encoding="utf-8"))
    if provenance.get("target") != "x86_64-pc-windows-msvc" or provenance.get("schemaVersion") != 1:
        raise ValueError("A verified Windows FFmpeg source delivery is required.")
    helper_record = next(record for record in provenance["buildInputs"] if record["path"] == "build/scripts/build-package-ffmpeg.py")
    helper_path = media / helper_record["path"]
    if digest(helper_path) != helper_record["sha256"]:
        raise ValueError("The shared media build recipe has changed.")
    spec = importlib.util.spec_from_file_location("media_builder", helper_path)
    helper = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(helper)
    helper.verify_compiler(compiler)
    source_record = next(record for record in provenance["additionalSources"] if record["base"] == "mingw-w64-x264")
    archive = media / source_record["path"]
    if source_record["sha256"] != SOURCE_SHA256 or digest(archive) != SOURCE_SHA256:
        raise ValueError("The shared x264 source differs from its pinned revision.")
    destination = args.destination.absolute()
    destination.mkdir(parents=True, exist_ok=False)
    with tarfile.open(archive) as bundle:
        members = bundle.getmembers()
        if len(members) > 100_000 or sum(member.size for member in members) > 512 * 1024 * 1024:
            raise ValueError("The x264 source archive exceeds its bounds.")
        names = set()
        for member in members:
            parts = member.name.split("/")
            if parts[0] != "mingw-w64-x264" or ".." in parts or "\\" in member.name or not (member.isfile() or member.isdir()):
                raise ValueError(f"Unexpected x264 source entry: {member.name}")
            if member.isfile() and member.name in names:
                raise ValueError("The x264 source repeats a file.")
            names.add(member.name)
        bundle.extractall(destination, filter="data")
    git = Path(shutil.which("git") or "").resolve(strict=True)
    empty_hooks = destination / "empty-git-hooks"
    empty_hooks.mkdir()
    env = {name: value for name, value in helper.build_environment("").items() if not name.upper().startswith("GIT_")}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_SYSTEM=os.devnull)
    source = destination / "source"
    git_args = [str(git), "-c", f"core.hooksPath={empty_hooks}"]
    subprocess.run([*git_args, "clone", "--no-checkout", "--no-hardlinks", "--local", f"--template={empty_hooks}", str(destination / "mingw-w64-x264/x264"), str(source)], env=env, check=True, timeout=120)
    subprocess.run([*git_args, "-C", str(source), "checkout", "--detach", COMMIT], env=env, check=True, timeout=120)
    observed = subprocess.check_output([*git_args, "-C", str(source), "rev-parse", "HEAD"], env=env, text=True).strip()
    if observed != COMMIT:
        raise ValueError("The x264 checkout has a different revision.")
    git_version = subprocess.check_output([str(git), "--version"], env=env, text=True).strip()
    env["PATH"] = os.pathsep.join(map(str, [compiler / "ucrt64/bin", compiler / "usr/bin", git.parent, Path(os.environ["SystemRoot"]) / "System32"]))
    git_posix = "/" + git.drive[0].lower() + git.parent.as_posix()[2:]
    commands = ["./configure " + shlex.join(CONFIGURE), "make -j4"]
    with (destination / "build.log").open("x", encoding="utf-8") as log:
        for command in commands:
            print(command, flush=True)
            subprocess.run([str(compiler / "usr/bin/bash.exe"), "--noprofile", "--norc", "-c", "export PATH=" + shlex.quote("/ucrt64/bin:/usr/bin:" + git_posix) + "; " + command], cwd=source, env=env, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=1800)
    finish(destination, source, media, compiler, provenance, source_record, git_version)


def finish(destination, source, media, compiler, provenance, source_record, git_version):
    delivery = destination / "delivery"
    delivery.mkdir()
    executable = delivery / "x264.exe"
    shutil.copy2(source / "x264.exe", executable)
    version = subprocess.check_output([str(executable), "--version"], text=True, timeout=20).splitlines()[0]
    if not version.startswith("x264 0.165.") or COMMIT[:7] not in version:
        raise ValueError(f"Unexpected standalone x264 identity: {version}")
    dependencies = subprocess.check_output([str(compiler / "ucrt64/bin/objdump.exe"), "-p", str(executable)], text=True, timeout=20)
    imports = sorted({line.split("DLL Name:", 1)[1].strip() for line in dependencies.splitlines() if "DLL Name:" in line})
    if not imports or any(name.lower() not in {"kernel32.dll", "msvcrt.dll", "shell32.dll", "winmm.dll", "user32.dll"} and not name.lower().startswith("api-ms-win-") for name in imports):
        raise ValueError(f"x264 imports a non-system runtime: {imports}")
    shutil.copy2(source / "COPYING", delivery / "COPYING")
    shutil.copy2(__file__, delivery / "build-package-x264.py")
    runtime_sources = [record for record in provenance["additionalSources"] if record["base"] in {"mingw-w64-gcc", "mingw-w64-crt", "mingw-w64-headers", "mingw-w64-winpthreads"}]
    if len(runtime_sources) != 4:
        raise ValueError("The shared compiler/runtime source closure is incomplete.")
    receipt = {"schemaVersion": 1, "target": provenance["target"], "tool": {"id": "x264", "path": "x264.exe", "version": version, "sha256": digest(executable), "systemImports": imports}, "sharedMediaProvenanceSha256": digest(media / "build-provenance.json"), "source": source_record, "sourceCommit": COMMIT, "runtimeSources": runtime_sources, "configure": CONFIGURE, "gitVersion": git_version, "licenses": [{"path": "COPYING", "sha256": digest(delivery / "COPYING")}], "buildInputs": [{"path": "build-package-x264.py", "sha256": digest(delivery / "build-package-x264.py")}]}
    (delivery / "build-provenance.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    print(f"Verified standalone x264 build: {delivery}", flush=True)


if __name__ == "__main__":
    main()
