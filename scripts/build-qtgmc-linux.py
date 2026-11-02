"""Build a task-local, pinned Linux R79/QTGMC qualification provider.

Requires Python3.14+ to read the locked source bundles, Python3.12+ development
headers for --python, GCC/G++, make, Ninja, CMake3.28+, nasm, pkg-config,
autoconf, automake and libtool. No package manager or global install is used.
The complete production FFmpeg build is an explicit, hash-verified input.
Existing destinations require --resume; completed-stage payload hashes are
checked before reuse. Failed commands and their source trees are retained.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import subprocess
import sys
import zipfile

sys.dont_write_bytecode = True
SCRIPTS = Path(__file__).resolve().parent
LOCK_NAMES = ("qtgmc-linux-lock.json", "qtgmc-windows-lock.json", "package-av1an-windows-lock.json", "package-ffmpeg-linux-lock.json")
COMMON = ("vapoursynth", "cython", "zimg", "graphengine", "lsmash", "decoderFfmpeg", "xxhash", "obuparse", "liblsmash")
PLUGINS = {"znedi3": "vsznedi3.so", "fmtconv": "libfmtconv.so", "mvtools": "libmvtools.so", "miscfilters": "libmiscfilters.so", "removegrainvs": "libremovegrain.so", "eedi3": "libeedi3m.so", "fft3dfilter": "libfft3dfilter.so", "temporalsoften2": "libtemporalsoften2.so", "lsmash": "liblsmashsource.so"}


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


SUPPORT = module("qtgmc_linux_support", "package-av1an-support.py")
DOWNLOADS = module("qtgmc_linux_downloads", "stage-bundled-tools.py")
MEDIA = module("qtgmc_linux_media", "build-package-ffmpeg-linux.py")
SCORERS = module("qtgmc_linux_readers", "build-vapoursynth-linux-scorers.py")


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def source_inputs():
    common = json.loads((SCRIPTS / LOCK_NAMES[2]).read_text())["inputs"]
    qtgmc = json.loads((SCRIPTS / LOCK_NAMES[1]).read_text())["inputs"]
    linux = json.loads((SCRIPTS / LOCK_NAMES[0]).read_text())
    if linux["schemaVersion"] != 1 or linux["target"] != "x86_64-unknown-linux-gnu":
        raise ValueError("Unsupported Linux QTGMC source lock.")
    result = {name: dict(common[name]) for name in COMMON}
    result.update({name: {"url": row["sourceUrl"], "sha256": row["sourceSha256"], "revision": row["revision"]} for name, row in qtgmc.items() if name != "vapoursynth"})
    result.update({row["id"]: dict(row) for row in linux["submodules"]})
    return result


def safe_child(root, value):
    value = Path(value)
    if value.is_absolute() or ".." in value.parts or not value.parts:
        raise ValueError("A retained build path must stay inside its owned root.")
    candidate = root / value
    if candidate.is_symlink() or not candidate.resolve().is_relative_to(root.resolve()):
        raise ValueError("A retained build path was redirected.")
    return candidate


class Build:
    def __init__(self, args):
        self.args = args
        self.root = args.destination.absolute()
        self.cache = args.cache.absolute()
        self.cache.mkdir(parents=True, exist_ok=True)
        self.rows = source_inputs()
        self.sources = {}
        self.archives = {}
        self.prefix = args.media_build.resolve(strict=True) / "prefix"
        self.install = self.root / "install"
        self.plugins = self.root / "plugins"
        self.python_modules = self.root / "python"
        self.runtime = None
        self.python = args.python.resolve(strict=True)
        configuration = {"target": "x86_64-unknown-linux-gnu", "destination": str(self.root), "mediaBuild": str(args.media_build.resolve(strict=True)), "mediaReceipt": digest(args.media_build / "delivery/build-provenance.json"), "mediaLibraries": {name: digest(self.prefix / "lib" / name) for name in ("libdav1d.a", "libvpx.a", "libz.a")}, "python": str(self.python), "pythonSha256": digest(self.python), "locks": {name: digest(SCRIPTS / name) for name in LOCK_NAMES}}
        if args.resume:
            if self.root.is_symlink() or not self.root.is_dir():
                raise ValueError("Resume requires an existing non-redirected build directory.")
            self.state = json.loads((self.root / "state.json").read_text())
            if self.state["configuration"] != configuration:
                raise ValueError("Resume inputs differ from this build's recorded identity.")
            for records in self.state["stages"].values():
                for row in records:
                    path = safe_child(self.root, row["path"])
                    if not path.is_file() or digest(path) != row["sha256"]:
                        raise ValueError("A completed stage's payload differs from its receipt.")
        else:
            self.root.mkdir(parents=True, exist_ok=False)
            for path in (self.install, self.plugins, self.python_modules, self.root / "work"):
                path.mkdir()
            self.state = {"configuration": configuration, "stages": {}, "sources": {}, "recipeRuns": []}
        self.state["recipeRuns"].append(digest(Path(__file__)))
        self.save()
        self.env = {name: value for name, value in os.environ.items() if name in {"HOME", "LANG", "TMPDIR"}}
        self.env.update(PATH="/usr/bin:/bin", LC_ALL="C", TZ="UTC", CC="gcc", CXX="g++", CFLAGS="-O2 -fPIC -march=x86-64 -mtune=generic", CXXFLAGS="-O2 -fPIC -march=x86-64 -mtune=generic", CPPFLAGS=f"-I{self.install}/include", LDFLAGS=f"-L{self.install}/lib", PKG_CONFIG_PATH="", PKG_CONFIG_LIBDIR=str(self.install / "lib/pkgconfig"), GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull)

    def save(self):
        path = self.root / "state.json"
        temporary = path.with_suffix(".new")
        temporary.write_text(json.dumps(self.state, indent=2) + "\n")
        temporary.replace(path)

    def run(self, name, command, cwd=None, env=None):
        command = list(map(str, command))
        with (self.root / (name + ".log")).open("a", encoding="utf-8") as log:
            log.write("COMMAND " + json.dumps({"argv": command, "cwd": str(cwd or self.root)}) + "\n")
            log.flush()
            subprocess.run(command, cwd=cwd or self.root, env=env or self.env, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=2400)

    def stage(self, name, operation):
        if name in self.state["stages"]:
            print("Rechecked completed stage: " + name, flush=True)
            return
        print("Building stage: " + name, flush=True)
        files = operation()
        self.state["stages"][name] = [{"path": path.relative_to(self.root).as_posix(), "sha256": digest(path)} for path in sorted(files)]
        self.save()

    def prepare(self):
        for name, row in self.rows.items():
            archive = DOWNLOADS.download(row["url"], row["sha256"], self.cache)
            self.archives[name] = archive
            if name in self.state["sources"]:
                self.sources[name] = safe_child(self.root, self.state["sources"][name])
                if not self.sources[name].is_dir():
                    raise ValueError("A prepared source tree is missing.")
            else:
                path = SUPPORT.unpack(archive, self.root / "work" / name, single_root=name != "fmtconv")
                self.sources[name] = path
                self.state["sources"][name] = path.relative_to(self.root).as_posix()
                self.save()
        linux = json.loads((SCRIPTS / LOCK_NAMES[0]).read_text())
        with zipfile.ZipFile(self.archives["znedi3"]) as archive:
            if archive.comment.decode() != linux["znedi3SourceCommit"]:
                raise ValueError("The pinned znedi3 archive has a different source commit.")
        meson = next(row for row in json.loads((SCRIPTS / LOCK_NAMES[3]).read_text())["buildTools"] if row["id"] == "meson")
        self.archives["meson"] = MEDIA.download(meson, self.cache)
        self.rows["meson"] = meson
        if "meson" not in self.state["sources"]:
            archive = self.archives["meson"]
            bundle = MEDIA.extract(archive, self.root / "work/meson-bundle", meson["bundleRoot"])
            embedded = bundle.parent / meson["embeddedArchive"]
            if digest(embedded) != meson["embeddedSha256"]:
                raise ValueError("The pinned Meson inner source differs.")
            path = MEDIA.extract(embedded, self.root / "work/meson", meson["sourceRoot"])
            self.state["sources"]["meson"] = path.relative_to(self.root).as_posix()
            self.save()
        self.meson = [sys.executable, str(safe_child(self.root, self.state["sources"]["meson"]) / "meson.py")]
        self.sources["meson"] = safe_child(self.root, self.state["sources"]["meson"])

    def runtime_build(self):
        source = self.sources["vapoursynth"]
        zimg = source / "subprojects/zimg"
        if not zimg.exists():
            shutil.copytree(self.sources["zimg"], zimg)
            shutil.copytree(self.sources["graphengine"], zimg / "graphengine", dirs_exist_ok=True)
            shutil.copytree(source / "subprojects/packagefiles/zimg", zimg, dirs_exist_ok=True)
        native = self.root / "native.ini"
        native.write_text("[binaries]\npython = " + repr(str(self.python)) + "\ncython = " + repr([str(self.python), str(self.sources["cython"] / "cython.py")]) + "\n")
        build = self.root / "runtime-build"
        if not (build / "build.ninja").exists():
            self.run("runtime", [*self.meson, "setup", build, source, "--native-file", native, "--prefix", self.install, "--libdir", "lib", "--wrap-mode=nodownload", "--force-fallback-for=zimg", "-Db_lto=false"])
        self.run("runtime", [*self.meson, "compile", "-C", build, "-j", self.args.jobs])
        self.run("runtime", [*self.meson, "install", "-C", build])
        return [path for path in self.install.rglob("*") if path.is_file() and not path.is_symlink()]

    def runtime_paths(self):
        matches = list(self.install.rglob("vapoursynth.abi3.so"))
        if len(matches) != 1:
            raise ValueError("The built R79 Python extension is missing or ambiguous.")
        self.runtime = matches[0].parent
        # R79 retains its API3 compatibility layer, while Meson intentionally
        # installs only API4 headers. The pinned MVTools/TemporalSoften sources
        # still compile against API3; use the same R79 source's legacy headers.
        for name in ("VapourSynth.h", "VSHelper.h"):
            shutil.copy2(self.sources["vapoursynth"] / "include" / name, self.runtime / "include" / name)
        # The old plugin Meson files also request libdir, which R79's current
        # headers-only pc file no longer declares. Keep the original installed
        # file intact and provide this explicit, task-local SDK compatibility pc.
        compatibility = self.root / "compat-pkgconfig"
        compatibility.mkdir(exist_ok=True)
        (compatibility / "vapoursynth.pc").write_text(f"prefix={self.runtime}\nincludedir=${{prefix}}/include\nlibdir=${{prefix}}\nName: vapoursynth\nDescription: Pinned R79 SDK with legacy plugin libdir\nVersion: 79\nCflags: -I${{includedir}}\n")
        self.env["PKG_CONFIG_PATH"] = str(compatibility)
        self.env["CPPFLAGS"] += " -I" + str(self.runtime / "include")

    def meson_plugin(self, name):
        build = self.root / (name + "-build")
        if not (build / "build.ninja").exists():
            self.run(name, [*self.meson, "setup", build, self.sources[name], "--prefix", self.install, "--libdir", "lib", "--wrap-mode=nodownload", "-Db_lto=false"])
        self.run(name, [*self.meson, "compile", "-C", build, "-j", self.args.jobs])
        candidates = list(build.glob("*.so"))
        if len(candidates) != 1:
            raise ValueError("The expected plugin output is missing or ambiguous: " + name)
        shutil.copy2(candidates[0], self.plugins / PLUGINS[name])

    def plugin_build(self):
        fftw = self.sources["fftw"]
        if not (fftw / "Makefile").exists():
            self.run("fftw", [fftw / "configure", f"--prefix={self.install}", "--libdir=" + str(self.install / "lib"), "--enable-float", "--enable-static", "--disable-shared", "--with-pic", "--enable-threads", "--disable-fortran", "--enable-sse2", "--enable-avx"], cwd=fftw)
        self.run("fftw", ["make", f"-j{self.args.jobs}"], cwd=fftw)
        self.run("fftw", ["make", "install"], cwd=fftw)
        znedi3 = self.sources["znedi3"]
        for name in ("graphengine", "vsxx"):
            shutil.copytree(self.sources["znedi3-" + name], znedi3 / name, dirs_exist_ok=True)
        self.run("znedi3", ["make", f"-j{self.args.jobs}", "X86=1", "X86_AVX512=1"], cwd=znedi3)
        shutil.copy2(znedi3 / "vsznedi3.so", self.plugins / PLUGINS["znedi3"])
        shutil.copy2(znedi3 / "nnedi3_weights.bin", self.plugins / "nnedi3_weights.bin")
        for name in ("miscfilters", "removegrainvs"):
            sources = sorted((self.sources[name] / "src").glob("*.cpp"))
            self.run(name, ["g++", "-std=c++17", "-O2", "-fPIC", "-shared", "-fvisibility=hidden", "-DVS_TARGET_CPU_X86", "-I" + str(self.runtime / "include"), "-I" + str(self.sources["vapoursynth"] / "src/core"), *sources, "-o", self.plugins / PLUGINS[name]])
        for name in ("mvtools", "eedi3", "fft3dfilter", "temporalsoften2"):
            self.meson_plugin(name)
        fmtconv = self.sources["fmtconv"] / "build/unix"
        if not (fmtconv / "configure").exists():
            self.run("fmtconv", ["sh", "autogen.sh"], cwd=fmtconv)
        if not (fmtconv / "Makefile").exists():
            self.run("fmtconv", [fmtconv / "configure", f"--prefix={self.install}", "--libdir=" + str(self.install / "lib"), "--disable-static", "--enable-shared"], cwd=fmtconv)
        self.run("fmtconv", ["make", f"-j{self.args.jobs}"], cwd=fmtconv)
        shutil.copy2(fmtconv / ".libs/libfmtconv.so", self.plugins / PLUGINS["fmtconv"])
        havsfunc = list(self.sources["havsfunc"].rglob("havsfunc.py"))
        if len(havsfunc) != 1:
            raise ValueError("The locked havsfunc module is missing or ambiguous.")
        shutil.copy2(havsfunc[0], self.python_modules / "havsfunc.py")
        shutil.copytree(self.sources["vsutil"] / "vsutil", self.python_modules / "vsutil", dirs_exist_ok=True)
        return [path for directory in (self.plugins, self.python_modules) for path in directory.rglob("*") if path.is_file()]

    def reader_build(self):
        original = self.env
        self.env = {**original, "PKG_CONFIG_PATH": str(self.prefix / "lib/pkgconfig"), "PKG_CONFIG_LIBDIR": str(self.prefix / "lib/pkgconfig")}
        try:
            SCORERS.Build.lsmash(self)
        finally:
            self.env = original
        return [self.plugins / PLUGINS["lsmash"]]

    def execution_environment(self):
        env = {name: value for name, value in os.environ.items() if name in {"HOME", "LANG", "TMPDIR"}}
        env.update(PATH=os.pathsep.join(map(str, (self.runtime, self.args.media_build / "delivery", self.python.parent, Path("/usr/bin"), Path("/bin")))), PYTHONPATH=os.pathsep.join(map(str, (self.python_modules, self.runtime.parent))), XDG_CONFIG_HOME=str(self.root / "config"), VSSCRIPT_PATH=str(self.runtime / "libvsscript.so"), VAPOURSYNTH_EXTRA_PLUGIN_PATH=str(self.plugins), JESSES_VAPOURSYNTH_PYTHON=str(self.python), JESSES_VAPOURSYNTH_PLUGIN_PATH=str(self.plugins), JESSES_FFMPEG=str(self.args.media_build / "delivery/ffmpeg"), JESSES_FFPROBE=str(self.args.media_build / "delivery/ffprobe"))
        return env

    def qualify(self):
        env = self.execution_environment()
        self.run("qualification", [self.python, "-m", "vapoursynth", "config"], env=env)
        source = self.root / "reader-proof.mkv"
        self.run("qualification", [self.args.media_build / "delivery/ffmpeg", "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=192x128:rate=4:duration=2", "-c:v", "ffv1", "-y", source], env=env)
        code = """import vapoursynth as vs, havsfunc, json, sys
core=vs.core
core.num_threads=2
assert 'Core R79' in str(core), str(core)
names={'znedi3':'znedi3','fmtconv':'fmtc','mvtools':'mv','miscfilters':'misc','removegrainvs':'rgvs','eedi3':'eedi3m','fft3dfilter':'fft3dfilter','temporalsoften2':'focus2','lsmash':'lsmas'}
for name,path in json.loads(sys.argv[1]).items():
    if not hasattr(core,names[name]): core.std.LoadPlugin(path=path)
reader=core.lsmas.LWLibavSource(source=sys.argv[2],cache=0)
assert reader.num_frames==8
for n in range(reader.num_frames): reader.get_frame(n)
proof={}
for preset in json.loads(sys.argv[3]):
    clip=core.std.BlankClip(width=192,height=128,length=8,format=vs.YUV420P8,fpsnum=30000,fpsden=1001)
    clip=core.std.SetFrameProps(clip,_FieldBased=2)
    output=havsfunc.QTGMC(clip,Preset=preset,TFF=True,FPSDivisor=1)
    assert (output.num_frames,output.fps_num,output.fps_den)==(16,60000,1001)
    for n in range(output.num_frames): output.get_frame(n)
    proof[preset]={'frames':output.num_frames,'fps':[output.fps_num,output.fps_den]}
print('JESSES_QTGMC_PROVIDER '+json.dumps(proof))
"""
        presets = json.loads((SCRIPTS / LOCK_NAMES[0]).read_text())["presets"]
        self.run("qualification", [self.python, "-c", code, json.dumps({name: str(self.plugins / filename) for name, filename in PLUGINS.items()}), source, json.dumps(presets)], env=env)
        log = (self.root / "qualification.log").read_text()
        proof = json.loads(next(line.removeprefix("JESSES_QTGMC_PROVIDER ") for line in reversed(log.splitlines()) if line.startswith("JESSES_QTGMC_PROVIDER ")))
        (self.root / "qualification.json").write_text(json.dumps({"schemaVersion": 1, "presets": proof, "readerFrames": 8}, indent=2) + "\n")
        with (self.root / "env.sh").open("w") as target:
            for name, value in env.items():
                if name not in {"HOME", "LANG", "TMPDIR", "PATH"}:
                    target.write(f"export {name}={shlex.quote(value)}\n")
            target.write(f"export PATH={shlex.quote(str(self.runtime) + ':' + str(self.args.media_build / 'delivery'))}:\"$PATH\"\n")
        return [self.root / "qualification.json", self.root / "env.sh"]

    def provenance(self):
        directory = self.root / "provenance"
        directory.mkdir(exist_ok=True)
        sources = directory / "sources"
        sources.mkdir(exist_ok=True)
        notices = directory / "licenses"
        notices.mkdir(exist_ok=True)
        for name, archive in self.archives.items():
            shutil.copy2(archive, sources / (name + ".source"))
            target = notices / name
            if not target.exists():
                SUPPORT.source_notices(archive, target)
            if not any(target.iterdir()):
                # Meson's outer source bundle contains an embedded upstream
                # archive; retain notices from that verified extracted source.
                for path in self.sources[name].rglob("*"):
                    if path.is_file() and path.name.upper().startswith(("COPYING", "LICENSE", "LICENCE", "NOTICE", "PATENT")) and path.stat().st_size <= 1024*1024:
                        shutil.copy2(path,target/(hashlib.sha256(str(path.relative_to(self.sources[name])).encode()).hexdigest()[:12]+"-"+path.name))
                # Some snapshots carry permission notices inside compiled
                # headers/source rather than a standalone license file.
                if not any(target.iterdir()):
                    text = []
                    for path in self.sources[name].rglob("*"):
                        if path.is_file() and path.suffix in {".c", ".cpp", ".h", ".hpp", ".rst"}:
                            value = path.read_text(errors="replace")[:10000]
                            if "copyright" in value.lower() or "permission is hereby granted" in value.lower():
                                text.append(str(path.relative_to(self.sources[name])) + "\n" + value.split("*/", 1)[0] + "*/\n")
                    if not text:
                        raise ValueError("No source-derived permission notices found: " + name)
                    (target / "source-notices.txt").write_text("\n".join(text))
        shared = []
        media = self.args.media_build / "delivery"
        receipt = json.loads((media / "build-provenance.json").read_text())
        for name, row in SCORERS.linked_codec_sources(receipt):
            target = sources / Path(row["path"]).name
            shutil.copy2(media / row["path"], target)
            shared.append({**row, "id": name, "path": target.relative_to(self.root).as_posix()})
            shutil.copytree(media / "licenses" / name, notices / ("media-" + name), dirs_exist_ok=True)
        for name in (*LOCK_NAMES, Path(__file__).name, "package-av1an-support.py", "stage-bundled-tools.py", "build-vapoursynth-linux-scorers.py", "build-package-ffmpeg-linux.py", "build-package-standalone-linux.py"):
            shutil.copy2(SCRIPTS / name, directory / name)
        logs = directory / "build-logs"
        logs.mkdir(exist_ok=True)
        for path in self.root.glob("*.log"):
            shutil.copy2(path, logs / path.name)
        versions = {}
        for tool in ("gcc", "g++", "cmake", "ninja", "make", "nasm", "pkg-config", "autoconf", "automake", "libtoolize"):
            result = subprocess.run([tool, "--version"], capture_output=True, text=True, check=True, timeout=15, env=self.env)
            versions[tool] = result.stdout.splitlines()[0]
        (directory / "toolchain.json").write_text(json.dumps(versions, indent=2) + "\n")
        files = [path for folder in (self.install, self.plugins, self.python_modules, self.root / "compat-pkgconfig", directory) for path in folder.rglob("*") if path.is_file() and not path.is_symlink()]
        receipt = {"schemaVersion": 1, "configuration": self.state["configuration"], "recipeRuns": self.state["recipeRuns"], "sources": self.rows, "sharedMediaSources": shared, "files": [{"path": path.relative_to(self.root).as_posix(), "sha256": digest(path)} for path in sorted(files)]}
        (self.root / "build-provenance.json").write_text(json.dumps(receipt, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--media-build", type=Path, required=True)
    parser.add_argument("--cache", type=Path, default=SCRIPTS.parent / "target/tool-source-cache")
    parser.add_argument("--python", type=Path, default=Path("/usr/bin/python3.12"))
    parser.add_argument("--jobs", type=int, default=2)
    parser.add_argument("--resume", action="store_true")
    args = parser.parse_args()
    args.media_build = args.media_build.absolute()
    if sys.version_info < (3, 14) or sys.platform != "linux" or platform.machine() not in {"x86_64", "AMD64"} or not 1 <= args.jobs <= 8:
        raise SystemExit("Use Python3.14+, Linux x64 and 1..8 build workers.")
    media = module("qtgmc_verified_media", "build-package-standalone-linux.py")
    media.verify_media(args.media_build / "delivery")
    build = Build(args)
    build.prepare()
    build.stage("runtime", build.runtime_build)
    build.runtime_paths()
    build.stage("plugins", build.plugin_build)
    build.stage("reader", build.reader_build)
    build.stage("qualification", build.qualify)
    build.provenance()
    print("Verified native R79/QTGMC provider: " + str(build.root), flush=True)


if __name__ == "__main__":
    main()
