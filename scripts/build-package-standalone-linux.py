"""Build source-pinned Linux x264 and mainline SVT-AV1 package deliveries.

Requires Linux x86-64, Python 3.14+, GCC/G++, make, nasm, cmake, ninja,
git, binutils and a verified Linux FFmpeg delivery. --verify-only checks the
same source archives, exact local Git revision and licenses on other hosts.
Build directories are exclusive; failed work remains available for inspection.
"""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import subprocess
import sys

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
TARGET = "x86_64-unknown-linux-gnu"
FFMPEG_SHA = "cf38e0e28c7e5605942c4a77755349b0145804a397af37eb1fb4c77cb237f635"
X264_COMMIT = "b35605ace3ddf7c1a5d67a2eb553f034aef41d55"
X264_SHA = "6a4d3620201074edec84681aeb25ffa1eceaa9451f7347ca1448f532634eed55"
X264_FLAGS = ["--enable-static", "--enable-pic", "--disable-lavf", "--disable-swscale", "--disable-avs", "--disable-opencl", "--bit-depth=all", "--chroma-format=all"]
SVT_FLAGS = ["-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release", "-DBUILD_SHARED_LIBS=OFF", "-DBUILD_TESTING=OFF", "-DBUILD_APPS=ON", "-DNATIVE=OFF", "-DSVT_AV1_PGO=OFF", "-DFETCHCONTENT_FULLY_DISCONNECTED=ON", "-DFETCHCONTENT_UPDATES_DISCONNECTED=ON", "-DCMAKE_SKIP_RPATH=ON", "-DCMAKE_C_COMPILER=/usr/bin/gcc", "-DCMAKE_CXX_COMPILER=/usr/bin/g++", "-DCMAKE_MAKE_PROGRAM=/usr/bin/ninja", "-DCMAKE_ASM_NASM_COMPILER=/usr/bin/nasm"]


def module(path):
    spec = importlib.util.spec_from_file_location("linux_media_source_helper", path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


MEDIA = module(HERE / "build-package-ffmpeg-linux.py")


def environment(home, native):
    # An allowlist excludes shell startup, Git/CMake overrides, injected compiler
    # flags, alternate linkers, dynamic-loader substitutions and Python hooks.
    result = {key: value for key, value in os.environ.items() if key in {"SystemRoot", "WINDIR", "TEMP", "TMP", "PATHEXT"}}
    result.update(PATH="/usr/bin:/bin" if native else os.environ.get("PATH", ""), HOME=str(home), LC_ALL="C", TZ="UTC", SOURCE_DATE_EPOCH="1789257600", GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_SYSTEM=os.devnull, CONFIG_SITE=os.devnull)
    return result


def inventory(root):
    result = {}
    for directory, dirs, files in os.walk(root, followlinks=False):
        for name in [*dirs, *files]:
            path = Path(directory) / name
            if path.is_symlink():
                raise ValueError("A media delivery contains a redirected path.")
        for name in files:
            path = Path(directory) / name
            if not path.is_file() or len(result) >= 100_000:
                raise ValueError("The media delivery exceeds its regular-file bounds.")
            result[path.relative_to(root).as_posix()] = MEDIA.digest(path)
    return result


def verify_media(root):
    receipt_path = root / "build-provenance.json"
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    if receipt.get("schemaVersion") != 1 or receipt.get("target") != TARGET:
        raise ValueError("A version-1 Linux FFmpeg delivery is required.")
    if receipt["source"]["sha256"] != FFMPEG_SHA:
        raise ValueError("The shared FFmpeg release differs from the source pin.")
    if len(receipt["tools"]) != 2 or {tool["id"] for tool in receipt["tools"]} != {"ffmpeg", "ffprobe"}:
        raise ValueError("The shared media tools must be exactly FFmpeg and FFprobe.")
    expected = {"build-provenance.json": MEDIA.digest(receipt_path)}
    records = [*receipt["tools"], receipt["source"], *receipt["additionalSources"], *receipt["licenses"], *receipt["buildInputs"], *receipt.get("supportFiles", [])]
    if not receipt["licenses"] or not receipt["buildInputs"]:
        raise ValueError("The media source delivery lacks notices or build inputs.")
    for record in records:
        name = MEDIA.relative(record["path"]).as_posix()
        if name in expected:
            raise ValueError("The media receipt repeats a payload path.")
        expected[name] = record["sha256"]
    if inventory(root) != expected:
        raise ValueError("The media delivery differs from its complete hash receipt.")
    shared = [record for record in receipt["additionalSources"] if record["sha256"] == X264_SHA]
    if len(shared) != 1:
        raise ValueError("The media delivery lacks the exact shared x264 source.")
    return receipt, shared[0]


class Build:
    def __init__(self, directory, cache, jobs, native):
        self.directory = directory.absolute()
        self.directory.mkdir(parents=True, exist_ok=False)
        self.work = self.directory / "work"
        self.work.mkdir()
        self.home = self.directory / "empty-home"
        self.home.mkdir()
        self.hooks = self.directory / "empty-hooks"
        self.hooks.mkdir()
        self.cache = cache.absolute()
        self.cache.mkdir(parents=True, exist_ok=True)
        self.jobs = jobs
        self.env = environment(self.home, native)
        self.commands = []
        self.sources = {}
        self.archives = {}
        self.pins = {}
        self.licenses = {}

    def run(self, argv, cwd=None, capture=False, timeout=1800):
        command = {"argv": [str(value) for value in argv], "cwd": str(cwd or self.directory)}
        self.commands.append(command)
        with (self.directory / "build.log").open("a", encoding="utf-8") as log:
            log.write(json.dumps(command) + "\n")
            log.flush()
            result = subprocess.run(command["argv"], cwd=command["cwd"], env=self.env, text=True, stdout=subprocess.PIPE if capture else log, stderr=subprocess.STDOUT, check=True, timeout=timeout)
        return result.stdout if capture else None

    def prepare(self, media=None):
        lock = json.loads((HERE / "package-ffmpeg-linux-lock.json").read_text(encoding="utf-8"))
        x264 = next(value for value in lock["libraries"] if value["id"] == "x264")
        if x264["sha256"] != X264_SHA or x264["gitCommit"] != X264_COMMIT:
            raise ValueError("The shared x264 source pin changed.")
        if media:
            root, record = media
            archive = root / MEDIA.relative(record["path"])
        else:
            # The common download cache uses checksum names, so both standalone
            # and media source verification can run fully offline once populated.
            archive = MEDIA.download({**x264, "filename": X264_SHA}, self.cache)
        if MEDIA.digest(archive) != X264_SHA:
            raise ValueError("The x264 archive checksum differs.")
        bundle = MEDIA.extract(archive, self.work / "x264-bundle", x264["bundleRoot"])
        source = self.work / "x264"
        git = shutil.which("git", path=self.env["PATH"])
        if not git:
            raise ValueError("Git is required to check out the retained exact source revision.")
        base = [git, "-c", f"core.hooksPath={self.hooks}"]
        self.run([*base, "clone", "--no-checkout", "--no-hardlinks", "--local", f"--template={self.hooks}", bundle.parent / MEDIA.relative(x264["gitDirectory"]), source], timeout=120)
        self.run([*base, "-C", source, "checkout", "--detach", X264_COMMIT], timeout=120)
        if self.run([*base, "-C", source, "rev-parse", "HEAD"], capture=True, timeout=20).strip() != X264_COMMIT:
            raise ValueError("The local x264 source has a different revision.")
        self.sources["x264"], self.archives["x264"], self.pins["x264"] = source, archive, x264
        svt = json.loads((HERE / "standalone-tool-sources.json").read_text(encoding="utf-8"))["svtAv1"]
        if svt["commit"] != "9292ec8e32bce26f781f277ec8739b53426c4300" or svt["sha256"] != "fe7e58bbd61040b460373ce7cfe0119311464eddbd6e620d84eeed317f598a64":
            raise ValueError("The mainline SVT source pin changed.")
        archive = MEDIA.download({**svt, "filename": svt["sha256"]}, self.cache)
        self.sources["svt-av1"] = MEDIA.extract(archive, self.work / "svt-source", svt["root"])
        self.archives["svt-av1"], self.pins["svt-av1"] = archive, svt
        for identifier, names in [("x264", ["COPYING"]), ("svt-av1", ["LICENSE.md", "LICENSE-BSD2.md", "PATENTS.md"])]:
            self.licenses[identifier] = []
            for name in names:
                path = self.sources[identifier] / name
                if not path.is_file() or not 0 < path.stat().st_size <= 1024 * 1024:
                    raise ValueError("A required source notice is absent or oversized.")
                self.licenses[identifier].append({"path": name, "sha256": MEDIA.digest(path)})
        report = {"schemaVersion": 1, "compiled": False, "sources": [{"id": identifier, "url": self.pins[identifier]["url"], "sha256": MEDIA.digest(self.archives[identifier]), "commit": X264_COMMIT if identifier == "x264" else self.pins[identifier]["commit"], "licenses": self.licenses[identifier]} for identifier in self.sources]}
        (self.directory / "source-verification.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    def compile(self):
        mapped = shlex.quote(f"-ffile-prefix-map={self.directory}=.")
        self.env.update(CC="/usr/bin/gcc", CXX="/usr/bin/g++", AR="/usr/bin/ar", CFLAGS=f"-O2 -march=x86-64 -mtune=generic {mapped}", CXXFLAGS=f"-O2 -march=x86-64 -mtune=generic {mapped}")
        source = self.sources["x264"]
        self.run([source / "configure", *X264_FLAGS], cwd=source, timeout=300)
        self.run(["make", f"-j{self.jobs}"], cwd=source)
        source = self.sources["svt-av1"]
        self.run(["cmake", "-S", source, "-B", self.work / "svt-build", *SVT_FLAGS], timeout=300)
        self.run(["cmake", "--build", self.work / "svt-build", "--target", "SvtAv1EncApp", "--parallel", str(self.jobs)])

    def finish(self, media_root, receipt, shared):
        ffmpeg = media_root / next(tool["path"] for tool in receipt["tools"] if tool["id"] == "ffmpeg")
        ffprobe = media_root / next(tool["path"] for tool in receipt["tools"] if tool["id"] == "ffprobe")
        toolchain = {tool: self.run([tool, "--version"], capture=True, timeout=20) for tool in ["gcc", "g++", "ld", "ar", "make", "nasm", "cmake", "ninja", "git", "readelf", "ldd"]}
        toolchain["os-release"] = Path("/etc/os-release").read_text(encoding="utf-8")
        for identifier, relative in [("x264", "x264"), ("svt-av1", "Bin/Release/SvtAv1EncApp")]:
            delivery = self.directory / identifier / "delivery"
            delivery.mkdir(parents=True)
            source = self.sources[identifier]
            executable = delivery / Path(relative).name
            shutil.copy2(source / relative, executable)
            self.run(["strip", executable], timeout=30)
            version = self.run([executable, "--version"], capture=True, timeout=20).strip()
            if identifier == "x264":
                version = version.splitlines()[0]
                valid = version.startswith("x264 0.165.") and X264_COMMIT[:7] in version
            else:
                valid = "v4.2.0" in version and "HDR" not in version and "5fish" not in version
            if not valid:
                raise ValueError("The built standalone encoder has an unexpected identity.")
            dependencies = self.run(["ldd", executable], capture=True, timeout=20)
            MEDIA.validate_dependencies(dependencies, self.directory)
            dynamic = self.run(["readelf", "-d", executable], capture=True, timeout=20)
            if "(RPATH)" in dynamic or "(RUNPATH)" in dynamic:
                raise ValueError("Standalone tools must not retain a build-time runtime search path.")
            names = sorted({Path(line.split()[0]).name for line in dependencies.splitlines() if line.split()})
            smoke = self.smoke(identifier, executable, ffmpeg, ffprobe)
            for notice in self.licenses[identifier]:
                shutil.copy2(source / notice["path"], delivery / notice["path"])
            build = delivery / "build"
            build.mkdir()
            for filename in [Path(__file__).name, "build-package-ffmpeg-linux.py", "package-ffmpeg-linux-lock.json", "standalone-tool-sources.json"]:
                shutil.copy2(HERE / filename, build / filename)
            for name, value in [("commands.json", self.commands), ("toolchain.json", toolchain), ("smoke.json", smoke)]:
                (build / name).write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
            shutil.copy2(self.directory / "build.log", build / "build.log")
            if identifier == "svt-av1":
                shutil.copy2(self.archives[identifier], delivery / "source.tar.gz")
                source_record = {**self.pins[identifier], "path": "source.tar.gz"}
            else:
                source_record = shared
            result = {"schemaVersion": 1, "target": TARGET, "tool": {"id": identifier, "path": executable.name, "sha256": MEDIA.digest(executable), "version": version}, "sharedMediaProvenanceSha256": MEDIA.digest(media_root / "build-provenance.json"), "source": source_record, "sourceCommit": X264_COMMIT if identifier == "x264" else self.pins[identifier]["commit"], "runtimeSources": [], "systemDependencies": names, "configure": X264_FLAGS if identifier == "x264" else SVT_FLAGS, "licenses": self.licenses[identifier], "buildInputs": [MEDIA.record(path, delivery) for path in sorted(build.iterdir())]}
            (delivery / "build-provenance.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
            print(f"Verified standalone delivery: {delivery}", flush=True)

    def smoke(self, identifier, executable, ffmpeg, ffprobe):
        evidence = []
        for depth in (8, 10):
            source = self.work / f"{identifier}-{depth}.y4m"
            output = self.work / f"{identifier}-{depth}.{'264' if identifier == 'x264' else 'ivf'}"
            pixel_format = "yuv420p" if depth == 8 else "yuv420p10le"
            self.run([ffmpeg, "-v", "error", "-nostdin", "-f", "lavfi", "-i", "testsrc2=s=64x64:r=4:d=2", "-vf", f"format={pixel_format}", "-frames:v", "8", "-strict", "-1", "-f", "yuv4mpegpipe", "-n", source], timeout=30)
            if identifier == "x264":
                argv = [executable, "--demuxer", "y4m", "--preset", "ultrafast", "--crf", "25", "--output-depth", str(depth), "--threads", "2", "-o", output, source]
            else:
                argv = [executable, "-i", source, "--input-depth", str(depth), "-n", "8", "--preset", "12", "--crf", "35", "--lp", "2", "-b", output]
            self.run(argv, timeout=90)
            frames = json.loads(self.run([ffprobe, "-v", "error", "-select_streams", "v:0", "-show_frames", "-of", "json", output], capture=True, timeout=30))["frames"]
            if len(frames) != 8 or any(frame.get("width") != 64 or frame.get("height") != 64 or frame.get("pix_fmt") != pixel_format for frame in frames):
                raise ValueError("The standalone encoder failed the eight-frame depth/decode check.")
            evidence.append({"bitDepth": depth, "decodedFrames": len(frames), "pixelFormat": pixel_format, "encodedSha256": MEDIA.digest(output)})
        return evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-directory", type=Path, required=True)
    parser.add_argument("--ffmpeg-build", type=Path)
    parser.add_argument("--cache", type=Path, default=HERE.parent / "target/tool-download-cache")
    parser.add_argument("--jobs", type=int, default=4)
    parser.add_argument("--verify-only", action="store_true")
    args = parser.parse_args()
    if sys.version_info < (3, 14) or not 1 <= args.jobs <= 32:
        raise SystemExit("Python 3.14+ and 1–32 workers are required.")
    native = not args.verify_only
    if native and (sys.platform != "linux" or platform.machine().lower() not in {"amd64", "x86_64"} or not args.ffmpeg_build):
        raise SystemExit("Native builds require Linux x86-64 and --ffmpeg-build; use --verify-only elsewhere.")
    root = args.ffmpeg_build.resolve(strict=True) if args.ffmpeg_build else None
    receipt, shared = verify_media(root) if root else (None, None)
    build = Build(args.build_directory, args.cache, args.jobs, native)
    build.prepare((root, shared) if root else None)
    if args.verify_only:
        print("Exact standalone sources and notices verified; Linux compilation was not run.")
        return
    build.compile()
    build.finish(root, receipt, shared)


if __name__ == "__main__":
    main()
