"""Build pinned Linux L-SMASH, vszip and Julek for an existing R79 runtime.

Requires Linux x64, Python3.12+ development headers, GCC/G++, CMake3.28+,
Ninja, make, nasm, pkg-config and the complete FFmpeg package build prefix.
The destination is exclusive. Failed work is retained; no automatic cleanup or
resume is attempted. --verify-existing only rechecks supplied artifacts.
"""

import argparse
import concurrent.futures
import datetime
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile

sys.dont_write_bytecode = True
SCRIPTS = Path(__file__).resolve().parent
PLUGIN_FILES = {
    "lsmash": "liblsmashsource.so",
    "vszip": "libvszip.so",
    "julek": "libjulek.so",
}


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def inputs():
    common = json.loads((SCRIPTS / "package-av1an-windows-lock.json").read_text())["inputs"]
    scorers = json.loads((SCRIPTS / "package-av1an-scorers-lock.json").read_text())["inputs"]
    linux = json.loads((SCRIPTS / "vapoursynth-linux-scorers-lock.json").read_text())
    if linux["schemaVersion"] != 1 or linux["target"] != "x86_64-unknown-linux-gnu":
        raise ValueError("Unsupported Linux plugin lock.")
    result = {name: common[name] for name in ("lsmash", "xxhash", "obuparse", "liblsmash", "decoderFfmpeg", "vapoursynth")}
    result.update({name: scorers[name] for name in ("vszip", "vsbindings", "zigimg", "julek", "libjxl", "brotli", "highway", "skcms")})
    result["zigLinux"] = linux["zigLinux"]
    return result


def linked_codec_sources(receipt):
    lock = json.loads((SCRIPTS / "package-ffmpeg-linux-lock.json").read_text())
    result = []
    for name in ("dav1d", "libvpx", "zlib"):
        expected = next(row for row in lock["libraries"] if row["id"] == name)
        matches = [row for row in receipt["additionalSources"] if Path(row["path"]).name == expected["filename"] and row["sha256"] == expected["sha256"]]
        if len(matches) != 1:
            raise ValueError("The exact shared codec source closure is incomplete.")
        result.append((name, matches[0]))
    return result


def plugin_inventory(root, receipt):
    """Verify names, containment and hashes before loading an existing plugin."""
    result = {}
    records = receipt.get("plugins", [])
    if len(records) != len(PLUGIN_FILES):
        raise ValueError("The plugin receipt must contain all three plugins.")
    for item in records:
        name = item["name"]
        if name not in PLUGIN_FILES or name in result:
            raise ValueError("Unknown or duplicate plugin identity.")
        expected = root / "plugins" / PLUGIN_FILES[name]
        path = Path(item["path"])
        path = path if path.is_absolute() else root / path
        if path != expected or path.is_symlink() or not path.is_file():
            raise ValueError("A plugin path is missing or redirected.")
        if not path.resolve().is_relative_to(root.resolve()) or digest(path) != item["sha256"]:
            raise ValueError("A plugin differs from its receipt.")
        result[name] = path
    for item in receipt.get("files", []):
        path = root / item["path"]
        if path.is_symlink() or not path.is_file() or not path.resolve().is_relative_to(root.resolve()) or digest(path) != item["sha256"]:
            raise ValueError("A retained provenance file differs from its receipt.")
    return result


def runtime_environment(python, runtime, config_home, plugins):
    # Preserve only ordinary process context; explicit task-local paths replace
    # loader/configuration hooks. The user's installed VS config is not edited.
    env = {name: value for name, value in os.environ.items() if name in {"HOME", "LANG", "LC_ALL", "TMPDIR", "DISPLAY", "WAYLAND_DISPLAY"}}
    env.update(
        PATH=str(python.parent) + ":/usr/bin:/bin",
        PYTHONPATH=str(runtime.parent),
        XDG_CONFIG_HOME=str(config_home),
        VSSCRIPT_PATH=str(runtime / "libvsscript.so"),
        VAPOURSYNTH_EXTRA_PLUGIN_PATH=str(plugins),
        JESSES_VAPOURSYNTH_PYTHON=str(python),
        JESSES_VAPOURSYNTH_PLUGIN_PATH=str(plugins),
    )
    return env


def qualify(root, python, runtime, ffmpeg, plugins, log):
    with tempfile.TemporaryDirectory(prefix="jesses-vs-probe-") as temporary:
        work = Path(temporary)
        env = runtime_environment(python, runtime, work / "config", root / "plugins")
        commands = [
            [str(python), "-m", "vapoursynth", "config"],
            [str(ffmpeg), "-v", "error", "-nostdin", "-f", "lavfi", "-i", "testsrc2=s=128x128:r=4:d=1", "-c:v", "ffv1", "-n", str(work / "reader.mkv")],
        ]
        for command in commands:
            log.write("COMMAND " + json.dumps(command) + "\n")
            log.flush()
            subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=60)
        code = """import vapoursynth as vs, math, json, sys
c=vs.core
c.num_threads=2
for path in json.loads(sys.argv[1]).values():
    if path.endswith('liblsmashsource.so') and hasattr(c,'lsmas'): continue
    if path.endswith('libvszip.so') and hasattr(c,'vszip'): continue
    if path.endswith('libjulek.so') and hasattr(c,'julek'): continue
    c.std.LoadPlugin(path=path)
assert 'Core R79' in str(c), str(c)
reader=c.lsmas.LWLibavSource(source=sys.argv[2], cache=0)
assert reader.num_frames==4
for n in range(4): reader.get_frame(n)
r=c.std.BlankClip(width=64,height=64,length=2,format=vs.YUV420P8,color=[64,128,128])
d=c.std.BlankClip(width=64,height=64,length=2,format=vs.YUV420P8,color=[72,128,128])
r=c.std.SetFrameProps(r,_Matrix=1,_Transfer=1,_Primaries=1)
d=c.std.SetFrameProps(d,_Matrix=1,_Transfer=1,_Primaries=1)
s=c.vszip.SSIMULACRA2(reference=r,distorted=d) if hasattr(c.vszip,'SSIMULACRA2') else c.vszip.Metrics(reference=r,distorted=d,mode=0)
b=c.julek.Butteraugli(reference=c.resize.Bicubic(r,format=vs.RGBS,matrix_in_s='709'),distorted=c.resize.Bicubic(d,format=vs.RGBS,matrix_in_s='709'),distmap=1,intensity_target=203.0)
result={}
for name,clip,keys in [('SSIMULACRA2',s,['SSIMULACRA2','_SSIMULACRA2']),('Butteraugli',b,['_FrameButteraugli'])]:
    values=[]
    for n in range(2):
        frame=clip.get_frame(n)
        value=next(float(frame.props[key]) for key in keys if key in frame.props)
        assert math.isfinite(value)
        values.append(value)
    result[name]=values
print('JESSES_PLUGIN_PROOF '+json.dumps(result))
"""
        command = [str(python), "-c", code, json.dumps({name: str(path) for name, path in plugins.items()}), str(work / "reader.mkv")]
        log.write("COMMAND " + json.dumps(command) + "\n")
        log.flush()
        result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=120)
        log.write(result.stdout + result.stderr)
        log.flush()
        result.check_returncode()
        line = next(line for line in result.stdout.splitlines() if line.startswith("JESSES_PLUGIN_PROOF "))
        return json.loads(line.removeprefix("JESSES_PLUGIN_PROOF "))


class Build:
    def __init__(self, args):
        self.args = args
        self.root = args.destination.absolute()
        self.root.mkdir(parents=True, exist_ok=False)
        self.plugins = self.root / "plugins"
        self.plugins.mkdir()
        self.sources = {}
        self.archives = {}
        self.rows = inputs()
        self.prefix = args.media_build.resolve(strict=True) / "prefix"
        self.runtime = args.vapoursynth_root.resolve(strict=True)
        self.env = {
            "PATH": "/usr/bin:/bin", "HOME": os.environ.get("HOME", "/nonexistent"), "LC_ALL": "C", "TZ": "UTC",
            "CC": "/usr/bin/gcc", "CXX": "/usr/bin/g++",
            "PKG_CONFIG_PATH": str(self.prefix / "lib/pkgconfig"),
            "PKG_CONFIG_LIBDIR": str(self.prefix / "lib/pkgconfig"),
            "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
        }

    def run(self, name, command, cwd=None, env=None):
        with (self.root / (name + ".log")).open("a", encoding="utf-8") as log:
            log.write("COMMAND " + json.dumps(list(map(str, command))) + "\n")
            log.flush()
            subprocess.run(list(map(str, command)), cwd=cwd or self.root, env=env or self.env, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=2400)

    def prepare(self):
        support = module("vs_source_support", "package-av1an-support.py")
        scorers = module("vs_source_scorers", "package-av1an-scorers.py")
        stager = module("vs_source_downloads", "stage-bundled-tools.py")
        self.args.cache.mkdir(parents=True, exist_ok=True)
        for name, row in self.rows.items():
            archive = self.args.cache / row["path"]
            if Path(row["path"]).name != row["path"]:
                raise ValueError("A source cache filename is not a leaf name.")
            if not archive.exists():
                downloaded = stager.download(row["url"], row["sha256"], self.args.cache)
                if downloaded != archive:
                    try:
                        os.link(downloaded, archive)
                    except FileExistsError:
                        pass
            if archive.is_symlink() or not archive.is_file() or digest(archive) != row["sha256"]:
                raise ValueError("A retained source input differs from its lock.")
            self.archives[name] = archive
            destination = self.root / (name + "-source")
            if name == "zigLinux":
                with tarfile.open(archive) as bundle:
                    members = bundle.getmembers()
                    if len(members) > 100_000 or sum(item.size for item in members) > 2 * 1024**3:
                        raise ValueError("The pinned Zig input exceeds extraction bounds.")
                    for item in members:
                        path = support.member_path(item.name)
                        if path.parts[0] != row["root"]:
                            raise ValueError("Unexpected Zig input root.")
                    bundle.extractall(destination, filter="data")
                self.sources[name] = destination / row["root"]
            else:
                self.sources[name] = scorers.unpack_jxl(archive, destination) if name == "libjxl" else support.unpack(archive, destination)
        for header in ("VapourSynth4.h", "VSScript4.h", "VSHelper4.h", "VSConstants4.h"):
            if digest(self.runtime / "include" / header) != digest(self.sources["vapoursynth"] / "include" / header):
                raise ValueError("The supplied runtime SDK differs from the pinned R79 source.")

    def zip(self):
        source = self.sources["vszip"]
        manifest = source / "build.zig.zon"
        text = manifest.read_text()
        for key, name in (("vapoursynth", "vsbindings"), ("zigimg", "zigimg")):
            shutil.copytree(self.sources[name], source / "deps" / key)
            text, count = re.subn(rf'(\.{key}\s*=\s*\.\{{).*?(\n        \}},)', rf'\1\n            .path = "deps/{key}",\2', text, flags=re.S)
            if count != 1:
                raise ValueError("The pinned Zig dependency layout changed.")
        manifest.write_text(text)
        env = {**self.env, "ZIG_GLOBAL_CACHE_DIR": str(self.root / "zig-global-cache"), "ZIG_LOCAL_CACHE_DIR": str(self.root / "zig-local-cache")}
        self.run("vszip", [self.sources["zigLinux"] / "zig", "build", "-Doptimize=ReleaseFast", "-Dtarget=x86_64-linux-gnu", "-Dcpu=x86_64", f"-j{self.args.jobs}", "--prefix", self.root / "vszip-install"], cwd=source, env=env)
        shutil.copy2(self.root / "vszip-install/lib/libvszip.so", self.plugins / PLUGIN_FILES["vszip"])

    def julek(self):
        source = self.sources["julek"]
        jxl = self.sources["libjxl"]
        for name in ("brotli", "highway", "skcms"):
            shutil.copytree(self.sources[name], jxl / "third_party" / name, dirs_exist_ok=True)
        shutil.copytree(jxl, source / "thirdparty/libjxl", dirs_exist_ok=True)
        installed = source / "thirdparty/libjxl_build/install"
        common = ["-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release", "-DCMAKE_POSITION_INDEPENDENT_CODE=ON", "-DCMAKE_INSTALL_LIBDIR=lib", "-DCMAKE_DISABLE_FIND_PACKAGE_Git=TRUE"]
        disabled = ("BENCHMARK", "EXAMPLES", "FUZZERS", "JNI", "MANPAGES", "OPENEXR", "SJPEG", "TOOLS", "JPEGLI", "PLUGINS", "VIEWERS")
        flags = ["-DBUILD_SHARED_LIBS=OFF", "-DBUILD_TESTING=OFF", *[f"-DJPEGXL_ENABLE_{name}=OFF" for name in disabled], "-DJPEGXL_ENABLE_SKCMS=ON", "-DJPEGXL_BUNDLE_SKCMS=ON", "-DCMAKE_DISABLE_FIND_PACKAGE_PNG=TRUE", "-DCMAKE_DISABLE_FIND_PACKAGE_JPEG=TRUE", "-DCMAKE_DISABLE_FIND_PACKAGE_OpenEXR=TRUE", f"-DCMAKE_INSTALL_PREFIX={installed}"]
        self.run("julek", ["cmake", "-S", source / "thirdparty/libjxl", "-B", self.root / "jxl-build", *common, *flags])
        self.run("julek", ["cmake", "--build", self.root / "jxl-build", "--parallel", self.args.jobs])
        self.run("julek", ["cmake", "--install", self.root / "jxl-build"])
        # libjxl is C++11 while this plugin is C++17. GCC's GNU-unique inline
        # constexpr tables otherwise conflict with libjxl's strong definitions.
        self.run("julek", ["cmake", "-S", source, "-B", self.root / "julek-build", *common, "-DCMAKE_CXX_FLAGS=-fno-gnu-unique", f"-DVS_INCLUDE_DIR={self.runtime / 'include'}", "-DVCS_TAG=r3-3f2780d"])
        self.run("julek", ["cmake", "--build", self.root / "julek-build", "--parallel", self.args.jobs])
        shutil.copy2(self.root / "julek-build/libjulek.so", self.plugins / PLUGIN_FILES["julek"])

    def lsmash(self):
        decoder = self.sources["decoderFfmpeg"]
        prefix = self.root / "decoder-prefix"
        flags = [f"--prefix={prefix}", "--disable-autodetect", "--disable-programs", "--disable-doc", "--disable-debug", "--disable-shared", "--enable-static", "--enable-pic", "--disable-muxers", "--disable-avdevice", "--disable-avfilter", "--disable-encoders", "--disable-filters", "--disable-devices", "--disable-network", "--enable-libdav1d", "--enable-libvpx", "--enable-zlib", "--disable-bzlib", "--disable-iconv", "--disable-lzma", "--enable-pthreads", "--pkg-config-flags=--static", f"--extra-cflags=-I{self.prefix}/include", f"--extra-ldflags=-L{self.prefix}/lib", "--extra-libs=-lm -lpthread -lstdc++"]
        self.run("lsmash", [decoder / "configure", *flags], cwd=decoder)
        self.run("lsmash", ["make", f"-j{self.args.jobs}"], cwd=decoder)
        self.run("lsmash", ["make", "install-libs", "install-headers"], cwd=decoder)
        source = self.sources["lsmash"]
        for sub, name in (("xxHash", "xxhash"), ("obuparse", "obuparse"), ("l-smash", "liblsmash")):
            shutil.copytree(self.sources[name], source / sub, dirs_exist_ok=True)
        env = {**self.env, "PKG_CONFIG_PATH": str(prefix / "lib/pkgconfig") + ":" + str(self.prefix / "lib/pkgconfig"), "PKG_CONFIG_LIBDIR": str(prefix / "lib/pkgconfig") + ":" + str(self.prefix / "lib/pkgconfig")}
        flags = ["-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release", "-DCMAKE_POSITION_INDEPENDENT_CODE=ON", "-DBUILD_AVS_PLUGIN=OFF", "-DBUILD_VS_PLUGIN=ON", "-DBUILD_AU2_PLUGIN=OFF", "-DENABLE_MFX=OFF", "-DENABLE_XML2=OFF", "-DBUILD_INDEXING_TOOL=OFF", "-DBUILD_SHARED_LIBS=OFF", "-DCMAKE_DISABLE_FIND_PACKAGE_Git=TRUE", f"-DFFMPEG_ROOT={prefix}", f"-DCMAKE_PREFIX_PATH={prefix};{self.prefix}", f"-DZLIB_LIBRARY={self.prefix}/lib/libz.a", f"-DZLIB_INCLUDE_DIR={self.prefix}/include", "-DCMAKE_C_STANDARD_LIBRARIES=-lm -lpthread -ldl -lstdc++", "-DCMAKE_MODULE_LINKER_FLAGS=-Wl,--no-undefined -Wl,-Bsymbolic"]
        self.run("lsmash", ["cmake", "-S", source, "-B", self.root / "lsmash-build", *flags], env=env)
        self.run("lsmash", ["cmake", "--build", self.root / "lsmash-build", "--target", "LSMASHSource", "--parallel", self.args.jobs], env=env)
        shutil.copy2(self.root / "lsmash-build/libLSMASHSource.so", self.plugins / PLUGIN_FILES["lsmash"])

    def finish(self):
        delivery = self.root / "provenance"
        delivery.mkdir()
        (delivery / "sources").mkdir()
        (delivery / "licenses").mkdir()
        source_records = []
        for name, archive in self.archives.items():
            target = delivery / "sources" / archive.name
            shutil.copy2(archive, target)
            source_records.append({**self.rows[name], "name": name, "path": str(target.relative_to(self.root))})
            notices = delivery / "licenses" / name
            notices.mkdir()
            for path in self.sources[name].iterdir():
                if path.is_file() and path.name.upper().startswith(("COPYING", "LICENSE", "PATENTS", "COPYRIGHT")) and path.stat().st_size <= 1024 * 1024:
                    shutil.copy2(path, notices / path.name)
        # L-SMASH statically links these three exact codec dependencies from the
        # media source delivery. Retain their source and notice closure too.
        media = self.args.media_build / "delivery"
        receipt = json.loads((media / "build-provenance.json").read_text())
        for name, item in linked_codec_sources(receipt):
            target = delivery / "sources" / Path(item["path"]).name
            shutil.copy2(media / item["path"], target)
            source_records.append({**item, "name": name, "path": str(target.relative_to(self.root))})
            shutil.copytree(media / "licenses" / name, delivery / "licenses" / ("media-" + name))
        for name in (Path(__file__).name, "package-av1an-windows-lock.json", "package-av1an-scorers-lock.json", "vapoursynth-linux-scorers-lock.json", "package-av1an-support.py", "package-av1an-scorers.py", "stage-bundled-tools.py", "build-package-standalone-linux.py", "build-package-ffmpeg-linux.py", "package-ffmpeg-linux-lock.json"):
            shutil.copy2(SCRIPTS / name, delivery / name)
        records = [{"name": name, "path": "plugins/" + filename, "sha256": digest(self.plugins / filename)} for name, filename in PLUGIN_FILES.items()]
        data = {"schemaVersion": 1, "target": "x86_64-unknown-linux-gnu", "sharedMediaProvenanceSha256": digest(media / "build-provenance.json"), "mediaPrefixLibraries": {name: digest(self.prefix / "lib" / name) for name in ("libdav1d.a", "libvpx.a", "libz.a")}, "plugins": records, "sources": source_records, "files": [{"path": str(path.relative_to(self.root)), "sha256": digest(path)} for path in sorted(delivery.rglob("*")) if path.is_file()]}
        (self.root / "build-provenance.json").write_text(json.dumps(data, indent=2) + "\n")
        return plugin_inventory(self.root, data)


def parser():
    result = argparse.ArgumentParser(description=__doc__)
    mode = result.add_mutually_exclusive_group(required=True)
    mode.add_argument("--destination", type=Path)
    mode.add_argument("--verify-existing", type=Path)
    result.add_argument("--media-build", type=Path, required=True)
    result.add_argument("--vapoursynth-root", type=Path, required=True, help="R79 Python package directory containing libvsscript.so and include/")
    result.add_argument("--python", type=Path, default=Path(sys.executable))
    result.add_argument("--cache", type=Path, default=SCRIPTS.parent / "target/tool-download-cache")
    result.add_argument("--jobs", type=int, default=2)
    return result


def main():
    args = parser().parse_args()
    if sys.platform != "linux" or platform.machine() not in {"x86_64", "AMD64"} or not 1 <= args.jobs <= 16:
        raise SystemExit("Linux x64 and 1..16 build workers are required.")
    args.media_build = args.media_build.resolve(strict=True)
    args.vapoursynth_root = args.vapoursynth_root.resolve(strict=True)
    args.python = args.python.absolute()
    media = module("linux_media_receipt", "build-package-standalone-linux.py")
    media.verify_media(args.media_build / "delivery")
    if args.verify_existing:
        root = args.verify_existing.resolve(strict=True)
        receipt_path = root / "build-provenance.json"
        receipt_path = receipt_path if receipt_path.exists() else root / "qualification.json"
        plugins = plugin_inventory(root, json.loads(receipt_path.read_text()))
        for row in inputs().values():
            archive = args.cache / row["path"]
            if not archive.is_file() or archive.is_symlink() or digest(archive) != row["sha256"]:
                raise ValueError("A retained source input differs from its current lock.")
        with tempfile.TemporaryFile(mode="w+", encoding="utf-8") as log:
            scores = qualify(root, args.python, args.vapoursynth_root, args.media_build / "delivery/ffmpeg", plugins, log)
            log.seek(0)
            print(log.read())
    else:
        build = Build(args)
        build.prepare()
        with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
            futures = [pool.submit(operation) for operation in (build.zip, build.julek, build.lsmash)]
            for future in futures:
                future.result()
        root = build.root
        plugins = build.finish()
        with (root / "qualification.log").open("x", encoding="utf-8") as log:
            scores = qualify(root, args.python, args.vapoursynth_root, args.media_build / "delivery/ffmpeg", plugins, log)
        env = runtime_environment(args.python, args.vapoursynth_root, root / "config", root / "plugins")
        with (root / "env.sh").open("x", encoding="utf-8") as target:
            for name in ("JESSES_VAPOURSYNTH_PYTHON", "JESSES_VAPOURSYNTH_PLUGIN_PATH", "VAPOURSYNTH_EXTRA_PLUGIN_PATH", "PYTHONPATH", "VSSCRIPT_PATH", "XDG_CONFIG_HOME"):
                target.write(f"export {name}={shlex.quote(env[name])}\n")
            target.write(f"export JESSES_FFMPEG={shlex.quote(str(args.media_build / 'delivery/ffmpeg'))}\n")
            target.write(f"export JESSES_FFPROBE={shlex.quote(str(args.media_build / 'delivery/ffprobe'))}\n")
            target.write(f"export PATH={shlex.quote(str(args.media_build / 'delivery'))}:\"$PATH\"\n")
        # Write only inside this new build's config tree.
        subprocess.run([str(args.python), "-m", "vapoursynth", "config"], env=env, check=True, timeout=30)
    print(json.dumps({"timestamp": datetime.datetime.now().astimezone().isoformat(), "plugins": {name: str(path) for name, path in plugins.items()}, "scores": scores, "verified": True}, indent=2))


if __name__ == "__main__":
    main()
