"""Build or assemble pinned Windows x64 AOM, VPX and x265 CLI deliveries.

Each delivery is bound to a verified, source-complete FFmpeg delivery and its
isolated MSYS2 UCRT64 compiler. No executable is accepted from a user's PATH.
"""

import argparse
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path, PurePosixPath
import shlex
import shutil
import stat
import subprocess
import sys
import tarfile
import uuid
from urllib.request import Request, urlopen
import zipfile

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
SOURCES = HERE / "standalone-tool-sources.json"
PATCHES = HERE / "patches"
MAX_SOURCE_BYTES = 512 * 1024 * 1024


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def safe_extract(archive, destination, root=None):
    members = archive.getmembers()
    if len(members) > 30_000 or sum(item.size for item in members) > MAX_SOURCE_BYTES:
        raise ValueError("The source archive exceeds extraction bounds.")
    seen = set()
    for item in members:
        parts = PurePosixPath(item.name).parts
        if (not parts or item.name.startswith("/") or "\\" in item.name or
                any(part in ("..", "") for part in parts) or
                (root and parts[0] != root) or
                not (item.isfile() or item.isdir())):
            raise ValueError(f"Unsafe source archive member: {item.name}")
        if item.isfile() and item.name in seen:
            raise ValueError(f"Repeated source archive member: {item.name}")
        seen.add(item.name)
    archive.extractall(destination, filter="data")


def aom_tree_sha256(archive_path):
    with tarfile.open(archive_path) as archive:
        members = archive.getmembers()
        if len(members) > 30_000 or sum(item.size for item in members) > MAX_SOURCE_BYTES:
            raise ValueError("The AOM source archive exceeds bounds.")
        result = hashlib.sha256()
        seen = set()
        for item in sorted(members, key=lambda member: member.name):
            parts = PurePosixPath(item.name).parts
            if (not parts or item.name.startswith("/") or "\\" in item.name or
                    ".." in parts or item.name in seen or not (item.isfile() or item.isdir())):
                raise ValueError(f"Unsafe AOM source member: {item.name}")
            seen.add(item.name)
            content = hashlib.sha256(archive.extractfile(item).read()).hexdigest().encode() if item.isfile() else b"DIR"
            result.update(item.name.encode() + b"\0" + content + b"\n")
        return result.hexdigest()


def download_aom(pin, cache):
    # Gitiles currently stamps each response's tar members with request time.
    # Pin the complete file tree and commit rather than unstable tar metadata.
    cached = cache / f"aom-{pin['treeSha256']}.tar.gz"
    if cached.exists():
        if cached.is_symlink() or aom_tree_sha256(cached) != pin["treeSha256"]:
            raise ValueError("The cached AOM source tree changed.")
        return cached
    temporary = cache / f"aom-{pin['treeSha256']}.partial-{uuid.uuid4().hex}"
    with urlopen(Request(pin["url"], headers={"User-Agent": "jesses-package-builder"}), timeout=90) as response, temporary.open("xb") as output:
        count = 0
        while block := response.read(1024 * 1024):
            count += len(block)
            if count > MAX_SOURCE_BYTES:
                raise ValueError("The AOM source download exceeds its bound.")
            output.write(block)
    if aom_tree_sha256(temporary) != pin["treeSha256"]:
        raise ValueError(f"The AOM source tree changed; retained for inspection: {temporary}")
    try:
        os.link(temporary, cached)
    except FileExistsError:
        if aom_tree_sha256(cached) != pin["treeSha256"]:
            raise ValueError("A different AOM source archive appeared in the cache.")
    temporary.unlink()
    return cached


def pinned_media(media, compiler):
    provenance_path = media / "build-provenance.json"
    provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
    if provenance.get("schemaVersion") != 1 or provenance.get("target") != "x86_64-pc-windows-msvc":
        raise ValueError("A verified Windows x64 media delivery is required.")
    if not provenance.get("additionalSources") or not provenance.get("buildInputs"):
        raise ValueError("The shared media source closure is incomplete.")
    for record in provenance["buildInputs"]:
        if digest(media / record["path"]) != record["sha256"]:
            raise ValueError("The shared media build inputs changed.")
    helper = module("media_builder", media / "build/scripts/build-package-ffmpeg.py")
    helper.verify_compiler(compiler)
    runtime = [item for item in provenance["additionalSources"] if item["base"] in
               {"mingw-w64-gcc", "mingw-w64-crt", "mingw-w64-headers", "mingw-w64-winpthreads"}]
    if len(runtime) != 4:
        raise ValueError("The shared compiler runtime sources are incomplete.")
    return provenance, helper, runtime


def source_record(provenance, base):
    matches = [item for item in provenance["additionalSources"] if item["base"] == base]
    if len(matches) != 1:
        raise ValueError(f"Missing pinned source package: {base}")
    return matches[0]


def package_record(media, provenance, base):
    record = source_record(provenance, base)
    path = media / record["path"]
    if digest(path) != record["sha256"]:
        raise ValueError(f"Pinned source package changed: {base}")
    return record, path


def build_env(helper, compiler):
    env = {key: value for key, value in helper.build_environment("").items()
           if not key.upper().startswith(("CMAKE_", "GIT_"))}
    env["PATH"] = os.pathsep.join(map(str, [compiler / "ucrt64/bin", compiler / "usr/bin",
                                               Path(os.environ["SystemRoot"]) / "System32"]))
    env["MSYSTEM"] = "UCRT64"
    env["CC"] = str(compiler / "ucrt64/bin/gcc.exe")
    env["CXX"] = str(compiler / "ucrt64/bin/g++.exe")
    return env


def cmake_executable(destination, cache, lock, stager):
    record = lock["cmakeWindows"]
    archive_path = stager.download(record["url"], record["sha256"], cache)
    with zipfile.ZipFile(archive_path) as archive:
        members = archive.infolist()
        if len(members) > 20_000 or sum(item.file_size for item in members) > MAX_SOURCE_BYTES:
            raise ValueError("The CMake archive exceeds extraction bounds.")
        seen = set()
        for item in members:
            parts = PurePosixPath(item.filename).parts
            if (not parts or item.filename.startswith("/") or parts[0] != record["root"] or
                    ".." in parts or "\\" in item.filename or
                    stat.S_ISLNK(item.external_attr >> 16) or item.filename in seen):
                raise ValueError("Unexpected CMake archive member.")
            seen.add(item.filename)
        archive.extractall(destination)
    return destination / record["root"] / "bin/cmake.exe"


def shell_path(path):
    resolved = path.resolve()
    if len(resolved.drive) != 2 or resolved.drive[1] != ":":
        raise ValueError("The source must be on a Windows drive.")
    return "/" + resolved.drive[0].lower() + resolved.as_posix()[2:]


def run_logged(commands, destination, env):
    with (destination / "build.log").open("x", encoding="utf-8") as log:
        for command, cwd in commands:
            subprocess.run(command, cwd=cwd, env=env, stdout=log, stderr=subprocess.STDOUT,
                           check=True, timeout=3600)


def build_aom(destination, cache, lock, compiler, helper, stager):
    pin = lock["aom"]
    archive = download_aom(pin, cache)
    source = destination / "aom-source"
    source.mkdir()
    with tarfile.open(archive) as bundle:
        safe_extract(bundle, source)
    patch = PATCHES / pin["patch"]
    if digest(patch) != pin["patchSha256"]:
        raise ValueError("The pinned AOM Y4M patch changed.")
    env = build_env(helper, compiler)
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
               GIT_CONFIG_SYSTEM=os.devnull)
    git = shutil.which("git")
    if not git:
        raise ValueError("Git is required to apply the pinned AOM source patch.")
    # The build directory may itself sit inside the Jesses checkout. Give the
    # extracted source its own worktree boundary so git apply addresses it.
    subprocess.run([git, "init", "--quiet"], cwd=source, env=env, check=True, timeout=30)
    subprocess.run([git, "apply", "--check", str(patch)], cwd=source, env=env,
                   check=True, timeout=30)
    subprocess.run([git, "apply", str(patch)], cwd=source, env=env,
                   check=True, timeout=30)
    if "for 420, 420jpeg, 420mpeg2 or 420p10 input" not in (source / "common/y4minput.c").read_text(encoding="utf-8"):
        raise ValueError("The pinned AOM Y4M patch did not reach the extracted source.")
    cmake = cmake_executable(destination, cache, lock, stager)
    build = destination / "aom-build"
    flags = ["-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release", "-DENABLE_TESTS=OFF",
             "-DENABLE_EXAMPLES=ON", "-DENABLE_TOOLS=ON", "-DBUILD_SHARED_LIBS=OFF",
             "-DCMAKE_EXE_LINKER_FLAGS=-static -static-libgcc -static-libstdc++",
             f"-DCMAKE_C_COMPILER={compiler / 'ucrt64/bin/gcc.exe'}",
             f"-DCMAKE_CXX_COMPILER={compiler / 'ucrt64/bin/g++.exe'}",
             f"-DCMAKE_MAKE_PROGRAM={compiler / 'ucrt64/bin/ninja.exe'}"]
    run_logged([([str(cmake), "-S", str(source), "-B", str(build), *flags], destination),
                ([str(cmake), "--build", str(build), "--target", "aomenc", "--parallel", "8"], destination)],
               destination, env)
    public_flags = [flag for flag in flags if not flag.startswith(("-DCMAKE_C_COMPILER=", "-DCMAKE_CXX_COMPILER=", "-DCMAKE_MAKE_PROGRAM="))]
    return build / "aomenc.exe", archive, [source / "LICENSE", source / "PATENTS"], public_flags


def build_vpx(destination, media, provenance, lock, compiler, helper):
    pin = lock["vpx"]
    record, package = package_record(media, provenance, "mingw-w64-libvpx")
    if record["version"] != "1.17.0-1":
        raise ValueError("Unexpected VPX source package version.")
    with tarfile.open(package) as outer:
        member = outer.getmember(pin["sourceMember"])
        if not member.isfile() or member.size > MAX_SOURCE_BYTES:
            raise ValueError("The VPX upstream source member is invalid.")
        upstream = outer.extractfile(member).read()
    if hashlib.sha256(upstream).hexdigest() != pin["sourceMemberSha256"]:
        raise ValueError("The VPX upstream source member changed.")
    archive = destination / "source.tar.gz"
    archive.write_bytes(upstream)
    with tarfile.open(fileobj=io.BytesIO(upstream)) as bundle:
        safe_extract(bundle, destination, pin["sourceRoot"])
    source = destination / pin["sourceRoot"]
    build = destination / "vpx-build"
    build.mkdir()
    env = build_env(helper, compiler)
    # libvpx's GNU make recipes run through /bin/sh, which cannot execute a
    # backslash-form Windows compiler path. Resolve the verified compiler via
    # the isolated PATH supplied above.
    env["CC"] = "gcc"
    env["CXX"] = "g++"
    env["LD"] = "g++"
    # This isolated compiler omits diffutils. The pinned libvpx recipe only
    # probes `diff --version` and compares two generated version files. Supply
    # that exact equality contract with the compiler's pinned sha256sum.
    shim_dir = destination / "build-shims"
    shim_dir.mkdir()
    shim = shim_dir / "diff"
    shim.write_text(
        '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "file-equality diff shim 1"; exit 0; fi\n'
        '[ "$#" = 2 ] || exit 2\nfirst="$1"; second="$2"\n'
        'set -- $(sha256sum "$first"); first_hash="$1"\n'
        'set -- $(sha256sum "$second"); [ "$first_hash" = "$1" ]\n', encoding="utf-8", newline="\n")
    shim.chmod(0o755)
    env["PATH"] = str(shim_dir) + os.pathsep + env["PATH"]
    env["CFLAGS"] = "-O2 -fno-asynchronous-unwind-tables"
    env["LDFLAGS"] = "-static -static-libgcc -static-libstdc++"
    bash = compiler / "usr/bin/bash.exe"
    configure = ["--target=x86_64-win64-gcc", "--enable-vp9", "--enable-vp9-highbitdepth",
                 "--enable-vp8", "--enable-static", "--disable-shared", "--enable-runtime-cpu-detect",
                 "--disable-docs", "--disable-unit-tests", "--disable-install-docs",
                 "--disable-install-srcs", "--enable-examples"]
    command = shlex.quote(shell_path(source / "configure")) + " " + " ".join(map(shlex.quote, configure))
    run_logged([([str(bash), "-c", command], build),
                ([str(compiler / "usr/bin/make.exe"), "-j8", "target=libs"], build),
                ([str(compiler / "usr/bin/make.exe"), "-j8", "target=examples", "vpxenc.exe"], build)], destination, env)
    return build / "vpxenc.exe", archive, [source / "LICENSE", source / "PATENTS"], configure


def assemble_x265(destination, media, provenance, compiler, lock, cache, stager):
    pin = lock["x265"]
    source, _ = package_record(media, provenance, pin["sourcePackage"])
    if source["version"] != "4.3-1":
        raise ValueError("Unexpected x265 source package version.")
    media_lock = json.loads((media / "build/scripts/package-ffmpeg-windows-lock.json").read_text(encoding="utf-8"))
    binaries = [item for item in media_lock["packages"] if item["name"] == pin["package"]]
    if len(binaries) != 1 or binaries[0]["version"] != "4.3-1":
        raise ValueError("Missing exact x265 binary package lock.")
    binary_record = binaries[0]
    package = stager.download(binary_record["url"], binary_record["sha256"], cache)
    with tarfile.open(package) as bundle:
        member = bundle.getmember("ucrt64/bin/x265.exe")
        if not member.isfile() or member.size > 128 * 1024 * 1024:
            raise ValueError("Unexpected x265 executable package member.")
        executable = destination / "x265.exe"
        with bundle.extractfile(member) as original, executable.open("xb") as output:
            shutil.copyfileobj(original, output)
    if digest(executable) != digest(compiler / "ucrt64/bin/x265.exe"):
        raise ValueError("The compiler's installed x265 differs from its pinned binary package.")
    return executable, package, [], [binary_record]


def imports_for(executable, compiler):
    output = subprocess.check_output([str(compiler / "ucrt64/bin/objdump.exe"), "-p", str(executable)],
                                     text=True, timeout=30)
    return sorted({line.split("DLL Name:", 1)[1].strip() for line in output.splitlines() if "DLL Name:" in line})


def verify_aom_tunes(executable, destination, environment):
    """Exercise optional perceptual tunes with native high-depth Y4M input."""
    fixture = destination / "aom-tune-input.y4m"
    width = height = 64
    luma = b"\x00\x02" * (width * height)
    chroma = b"\x00\x02" * (width * height // 4)
    with fixture.open("wb") as output:
        output.write(b"YUV4MPEG2 W64 H64 F4:1 Ip A1:1 C420p10\n")
        for _ in range(4):
            output.write(b"FRAME\n" + luma + chroma + chroma)
    tested = []
    for tune in ("iq", "ssimulacra2"):
        result = destination / f"aom-tune-{tune}.ivf"
        subprocess.run([str(executable), "--ivf", f"--output={result}", "--passes=1",
                        "--end-usage=q", "--cq-level=63", "--cpu-used=8", "--profile=0",
                        "--bit-depth=10", "--input-bit-depth=10",
                        "--chroma-sample-position=vertical", f"--tune={tune}",
                        "--disable-warning-prompt", str(fixture)], env=environment,
                       capture_output=True, text=True, check=True, timeout=180)
        with result.open("rb") as stream:
            header = stream.read(32)
        if len(header) != 32 or header[:4] != b"DKIF" or header[8:12] != b"AV01" or int.from_bytes(header[24:28], "little") != 4:
            raise ValueError(f"The AOM {tune} tune did not produce four AV1 frames.")
        tested.append({"tune": tune, "input": "4 frames 64x64 10-bit 4:2:0 Y4M",
                       "chromaSamplePosition": "vertical", "encodedFrameCount": 4})
    return tested, [fixture, *(destination / f"aom-tune-{tune}.ivf" for tune in ("iq", "ssimulacra2"))]


def finish(identifier, destination, media, provenance, runtime, compiler, source_archive,
           executable, licenses, configure, lock):
    delivery = destination / "delivery"
    delivery.mkdir()
    name = {"aom": "aomenc.exe", "vpx": "vpxenc.exe", "x265": "x265.exe"}[identifier]
    target = delivery / name
    shutil.copy2(executable, target)
    if identifier != "x265":
        subprocess.run([str(compiler / "ucrt64/bin/strip.exe"), str(target)], check=True, timeout=30)
    imports = imports_for(target, compiler)
    allowed = {"kernel32.dll", "advapi32.dll", "shell32.dll", "user32.dll", "msvcrt.dll",
               "ole32.dll", "winmm.dll", "ws2_32.dll", "gdi32.dll"}
    allowed_runtime = {"libstdc++-6.dll", "libgcc_s_seh-1.dll", "libwinpthread-1.dll"}
    if any(item.lower() not in allowed | allowed_runtime and not item.lower().startswith("api-ms-win-")
           for item in imports):
        raise ValueError(f"Unexpected non-system import: {imports}")
    support = []
    queue = [item for item in imports if item.lower() in allowed_runtime]
    seen = set()
    while queue:
        item = queue.pop()
        if item.lower() in seen:
            continue
        seen.add(item.lower())
        original = compiler / "ucrt64/bin" / item
        if not original.is_file():
            raise ValueError(f"Missing pinned runtime DLL: {item}")
        copy = delivery / item
        shutil.copy2(original, copy)
        support.append({"path": item, "sha256": digest(copy), "imports": imports_for(copy, compiler)})
        queue.extend(dependency for dependency in support[-1]["imports"] if dependency.lower() in allowed_runtime)
    env = os.environ.copy()
    env["PATH"] = str(delivery) + os.pathsep + str(Path(os.environ["SystemRoot"]) / "System32")
    identity_switch = "--help" if identifier in {"aom", "vpx"} else "--version"
    result = subprocess.run([str(target), identity_switch], env=env, capture_output=True,
                            text=True, timeout=20)
    full_output = (result.stdout + result.stderr).strip()
    version = next((line.strip() for line in full_output.splitlines() if "3.14.1" in line), full_output) if identifier == "aom" else next((line.strip() for line in full_output.splitlines() if "VP9 Encoder v1.17.0" in line), full_output) if identifier == "vpx" else full_output
    expected = {"aom": "3.14.1", "vpx": "1.17.0", "x265": "4.3"}[identifier]
    if expected not in version:
        raise ValueError(f"Unexpected {identifier} executable identity: {version}")
    tune_qualification, qualification_files = verify_aom_tunes(target, destination, env) if identifier == "aom" else ([], [])
    source = source_record(provenance, "mingw-w64-x265") if identifier == "x265" else None
    if identifier == "vpx":
        source = source_record(provenance, "mingw-w64-libvpx")
    if identifier == "aom":
        shutil.copy2(source_archive, delivery / "source.tar.gz")
        source = {**lock["aom"], "path": "source.tar.gz", "sha256": digest(delivery / "source.tar.gz")}
    elif identifier == "vpx":
        shutil.copy2(source_archive, delivery / "source.tar.gz")
        source = {**source, "upstreamArchivePath": "source.tar.gz",
                  "upstreamArchiveSha256": digest(delivery / "source.tar.gz")}
    else:
        shutil.copy2(source_archive, delivery / "binary-package.pkg.tar.zst")
    license_records = []
    for license_path in licenses:
        shutil.copy2(license_path, delivery / license_path.name)
        license_records.append({"path": license_path.name, "sha256": digest(delivery / license_path.name)})
    if identifier == "x265":
        license_records = [item for item in provenance["licenses"]
                           if item["path"].startswith("licenses/mingw-w64-x265/")]
        if not license_records:
            raise ValueError("x265 source license closure is missing.")
    shutil.copy2(__file__, delivery / Path(__file__).name)
    shutil.copy2(SOURCES, delivery / SOURCES.name)
    inputs = [Path(__file__).name, SOURCES.name]
    if identifier == "aom":
        patch = PATCHES / lock["aom"]["patch"]
        shutil.copy2(patch, delivery / patch.name)
        inputs.append(patch.name)
    if identifier == "vpx":
        shutil.copy2(destination / "build-shims/diff", delivery / "vpx-diff-shim.sh")
        inputs.append("vpx-diff-shim.sh")
    tool_id = {"aom": "aomenc", "vpx": "vpxenc", "x265": "x265"}[identifier]
    receipt = {"schemaVersion": 1, "target": "x86_64-pc-windows-msvc", "id": identifier,
               "tool": {"id": tool_id, "path": name, "sha256": digest(target),
                        "version": version, "imports": imports},
               "sharedMediaProvenanceSha256": digest(media / "build-provenance.json"),
               "source": source, "runtimeSources": runtime, "configure": configure,
               "supportFiles": support, "licenses": license_records,
               "buildInputs": [{"path": item, "sha256": digest(delivery / item)} for item in inputs]}
    if tune_qualification:
        retained = []
        for item in qualification_files:
            location = delivery / "qualification" / item.name
            location.parent.mkdir(exist_ok=True)
            shutil.copy2(item, location)
            retained.append({"path": location.relative_to(delivery).as_posix(), "sha256": digest(location)})
        receipt["qualification"] = {"aomPerceptualTunes": tune_qualification, "files": retained}
    if identifier == "x265":
        receipt["binaryPackage"] = {**configure[0], "path": "binary-package.pkg.tar.zst"}
    (delivery / "build-provenance.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    print(f"Verified {identifier} delivery: {delivery}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--encoder", choices=["aom", "vpx", "x265"], required=True)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--ffmpeg-build", type=Path, required=True)
    parser.add_argument("--msys-root", type=Path, required=True)
    parser.add_argument("--cache", type=Path, default=Path("target/tool-download-cache"))
    args = parser.parse_args()
    if sys.platform != "win32":
        raise SystemExit("This recipe builds Windows x64 deliveries only.")
    media = args.ffmpeg_build.resolve(strict=True)
    compiler = args.msys_root.resolve(strict=True)
    provenance, helper, runtime = pinned_media(media, compiler)
    lock = json.loads(SOURCES.read_text(encoding="utf-8"))
    if lock.get("schemaVersion") != 1:
        raise ValueError("Unexpected standalone source lock schema.")
    destination = args.destination.absolute()
    destination.mkdir(parents=True, exist_ok=False)
    cache = args.cache.absolute()
    cache.mkdir(parents=True, exist_ok=True)
    stager = module("media_stager", media / "build/scripts/stage-bundled-tools.py")
    if args.encoder == "aom":
        result = build_aom(destination, cache, lock, compiler, helper, stager)
    elif args.encoder == "vpx":
        result = build_vpx(destination, media, provenance, lock, compiler, helper)
    else:
        result = assemble_x265(destination, media, provenance, compiler, lock, cache, stager)
    finish(args.encoder, destination, media, provenance, runtime, compiler, result[1], result[0],
           result[2], result[3], lock)


if __name__ == "__main__":
    main()
