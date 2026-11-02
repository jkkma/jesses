"""Build the fork's exact grav1synth with private compatible FFmpeg/dav1d.

Python3.14+, native Rust1.98.1, C compiler, make, meson, ninja, nasm,
pkg-config and libclang are required. Windows uses the verified isolated UCRT64
compiler plus the Rust x86_64-pc-windows-gnu target. No system tool is installed,
no existing build directory is reused, and no FFmpeg9 runtime is substituted.
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
import tarfile

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
LOCK = HERE / "grav1synth-lock.json"
CONFIGURE = ["--disable-autodetect", "--disable-programs", "--disable-doc", "--disable-debug", "--disable-network", "--disable-encoders", "--disable-avdevice", "--disable-avfilter", "--disable-swscale", "--disable-swresample", "--disable-shared", "--enable-static", "--enable-pic", "--enable-libdav1d", "--disable-x86asm"]

def module(path):
    spec = importlib.util.spec_from_file_location("grav_source_helper", path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result

MEDIA = module(HERE / "build-package-ffmpeg-linux.py")
STAGER = module(HERE / "stage-bundled-tools.py")

def environment(home):
    # Keep network trust/proxy and Rust installation locations; remove compiler,
    # Cargo feature/profile, loader and shell injection from the parent session.
    keep = {"SYSTEMROOT", "WINDIR", "TEMP", "TMP", "PATHEXT", "USERPROFILE", "PROGRAMFILES", "PROGRAMFILES(X86)", "PATH", "RUSTUP_HOME", "SSL_CERT_FILE", "SSL_CERT_DIR", "CURL_CA_BUNDLE", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"}
    env = {key: value for key, value in os.environ.items() if key.upper() in keep}
    env.update(HOME=str(home), CARGO_HOME=str(home / "cargo"), LC_ALL="C", TZ="UTC", SOURCE_DATE_EPOCH="1789257600", GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_SYSTEM=os.devnull, CONFIG_SITE=os.devnull)
    return env

def inventory(root):
    result = []
    for path in sorted(root.rglob("*")):
        if path.is_symlink() or (hasattr(path, "is_junction") and path.is_junction()):
            raise ValueError("A grav1synth delivery cannot contain redirected paths.")
        if path.is_file():
            if len(result) >= 100_000:
                raise ValueError("The grav1synth delivery exceeds inventory bounds.")
            result.append({"path": path.relative_to(root).as_posix(), "sha256": MEDIA.digest(path)})
        elif not path.is_dir():
            raise ValueError("A grav1synth delivery contains a non-regular entry.")
    return result

def verify_delivery(delivery, target):
    receipt_path = delivery / "build-provenance.json"
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    if receipt.get("schemaVersion") != 1 or receipt.get("target") != target or receipt.get("sourceCommit") != lock["source"]["commit"]:
        raise ValueError("The grav1synth receipt has a different source or platform.")
    executable = "grav1synth.exe" if target == "x86_64-pc-windows-msvc" else "grav1synth"
    if receipt["tool"]["id"] != "grav1synth" or receipt["tool"]["path"] != executable or receipt["source"]["sha256"] != lock["source"]["sha256"] or receipt["qualification"]["presets"] != lock["presets"]:
        raise ValueError("The grav1synth source or preset capability differs from the pin.")
    expected = {"build-provenance.json": MEDIA.digest(receipt_path)}
    records = [receipt["tool"], receipt["source"], *receipt["additionalSources"], *receipt["licenses"], *receipt["buildInputs"], *receipt["supportFiles"], *receipt["qualificationFiles"]]
    for record in records:
        name = MEDIA.relative(record["path"]).as_posix()
        if name in expected:
            raise ValueError("The grav1synth receipt repeats a payload.")
        expected[name] = record["sha256"]
    if not receipt["licenses"] or not receipt["buildInputs"] or not {"ffmpeg", "dav1d", "cargo-vendor"}.issubset({record.get("id") for record in receipt["additionalSources"]}):
        raise ValueError("The grav1synth delivery lacks corresponding sources or build inputs.")
    for name in ["ffmpeg", "dav1d"]:
        records = [item for item in receipt["additionalSources"] if item.get("id") == name]
        if len(records) != 1 or records[0]["sha256"] != lock[name]["sha256"]:
            raise ValueError("The private grav1synth media source differs from its pin.")
    locks = [item for item in receipt["buildInputs"] if item["path"] == "build/grav1synth-lock.json"]
    if len(locks) != 1 or locks[0]["sha256"] != MEDIA.digest(LOCK):
        raise ValueError("The retained grav1synth source lock differs from the active pin.")
    if target == "x86_64-pc-windows-msvc":
        compiler_lock = json.loads((HERE / "package-ffmpeg-windows-lock.json").read_text(encoding="utf-8"))
        for pin in [item for item in compiler_lock["linkedSources"] if item["base"] in {"mingw-w64-gcc", "mingw-w64-crt", "mingw-w64-headers", "mingw-w64-winpthreads"}]:
            actual = [item for item in receipt["additionalSources"] if item.get("id") == pin["base"]]
            if len(actual) != 1 or actual[0]["sha256"] != pin["sha256"]:
                raise ValueError("The Windows compiler runtime sources differ from the active pin.")
    if {item["path"]: item["sha256"] for item in inventory(delivery)} != expected:
        raise ValueError("The grav1synth delivery differs from its complete hash receipt.")
    return receipt

def stage(delivery, tools_root, target):
    receipt = verify_delivery(delivery, target)
    # Verify the copied provider itself, with no alternate executable search.
    output = subprocess.check_output([str(delivery / receipt["tool"]["path"]), "presets"], text=True, timeout=20)
    if output != (delivery / "qualification/presets.txt").read_text(encoding="utf-8"):
        raise ValueError("The staged grav1synth preset listing differs from its qualification.")
    shutil.copytree(delivery, tools_root / "grav1synth")
    def prefix(item):
        return {**item, "path": "grav1synth/" + item["path"]}
    return {**prefix(receipt["tool"]), "sourceCommit": receipt["sourceCommit"], "source": prefix(receipt["source"]), **{key: [prefix(item) for item in receipt[key]] for key in ["additionalSources", "licenses", "buildInputs", "supportFiles", "qualificationFiles"]}, "buildProvenance": {"path": "grav1synth/build-provenance.json", "sha256": MEDIA.digest(delivery / "build-provenance.json")}}

class Build:
    def __init__(self, destination, cache, jobs, msys=None):
        self.root = destination.absolute()
        self.root.mkdir(parents=True, exist_ok=False)
        self.inputs = self.root / "build-inputs"
        self.inputs.mkdir()
        for name in ["build-package-grav1synth.py", "grav1synth-lock.json", "build-package-ffmpeg-linux.py", "stage-bundled-tools.py", "bundled-tool-sources.json", "svt-forks.json", "package-ffmpeg-windows-lock.json", "build-package-ffmpeg.py", "bootstrap-package-toolchain.py"]:
            shutil.copy2(HERE / name, self.inputs / name)
        self.home = self.root / "empty-home"
        self.home.mkdir()
        self.cache = cache.absolute()
        self.cache.mkdir(parents=True, exist_ok=True)
        self.jobs = jobs
        self.env = environment(self.home)
        self.windows = sys.platform == "win32"
        self.target = "x86_64-pc-windows-msvc" if self.windows else ("aarch64-apple-darwin" if platform.machine() == "arm64" else "x86_64-apple-darwin") if sys.platform == "darwin" else "x86_64-unknown-linux-gnu"
        self.rust_target = "x86_64-pc-windows-gnu" if self.windows else self.target
        self.lock = json.loads((self.inputs / "grav1synth-lock.json").read_text(encoding="utf-8"))
        self.cargo = Path(subprocess.check_output(["rustup", "which", "--toolchain", self.lock["rustVersion"], "cargo"], text=True, timeout=20).strip())
        self.env["PATH"] = str(self.cargo.parent) + os.pathsep + self.env.get("PATH", "")
        self.msys = msys.resolve(strict=True) if msys else None
        if self.windows:
            if not self.msys:
                raise ValueError("The verified isolated Windows compiler is required.")
            module(HERE / "build-package-ffmpeg.py").verify_compiler(self.msys)
            self.env["PATH"] = os.pathsep.join(map(str, [self.cargo.parent, self.msys / "ucrt64/bin", self.msys / "usr/bin", Path(os.environ["SystemRoot"]) / "System32"]))
            self.env.update(MSYSTEM="UCRT64", MSYS2_PATH_TYPE="strict", CHERE_INVOKING="1", CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=str(self.msys / "ucrt64/bin/gcc.exe"), PKG_CONFIG_ALLOW_CROSS="1")
            clang = Path(os.environ.get("LIBCLANG_PATH", str(Path(os.environ["ProgramFiles"]) / "LLVM/bin")))
            if not (clang / "libclang.dll").is_file():
                raise ValueError("Set LIBCLANG_PATH to the native LLVM directory containing libclang.dll.")
            self.env["LIBCLANG_PATH"] = str(clang.resolve())
        self.env.update(CARGO_BUILD_JOBS=str(jobs), CARGO_TARGET_DIR=str(self.root / "target"))

    def run(self, argv, cwd=None, capture=False, timeout=1200):
        with (self.root / "build.log").open("a", encoding="utf-8") as log:
            log.write(json.dumps({"argv": list(map(str, argv)), "cwd": str(cwd or self.root)}) + "\n")
            log.flush()
            result = subprocess.run(list(map(str, argv)), cwd=cwd or self.root, env=self.env, stdout=subprocess.PIPE if capture else log, stderr=log, text=True, check=True, timeout=timeout)
            return result.stdout if capture else None

    def compile(self):
        archives = {name: MEDIA.download(record, self.cache) for name, record in self.lock.items() if name in {"source", "ffmpeg", "dav1d"}}
        source = MEDIA.extract(archives["source"], self.root / "source", self.lock["source"]["sourceRoot"])
        media = MEDIA.extract(archives["ffmpeg"], self.root / "ffmpeg", self.lock["ffmpeg"]["sourceRoot"])
        with tarfile.open(archives["dav1d"]) as outer:
            member = outer.getmember(self.lock["dav1d"]["embeddedArchive"])
            if not member.isfile() or not 0 < member.size <= 64 * 1024**2:
                raise ValueError("The embedded dav1d source exceeds bounds.")
            payload = outer.extractfile(member).read()
        if hashlib.sha256(payload).hexdigest() != self.lock["dav1d"]["embeddedSha256"]:
            raise ValueError("The embedded dav1d source differs from its pin.")
        embedded = self.root / "dav1d.tar.xz"
        embedded.write_bytes(payload)
        decoder = MEDIA.extract(embedded, self.root / "dav1d-source", self.lock["dav1d"]["sourceRoot"])
        prefix = self.root / "private-ffmpeg8"
        self.run(["meson", "setup", self.root / "dav1d-build", decoder, f"--prefix={prefix.as_posix()}", "--libdir=lib", "--default-library=static", "--buildtype=release", "--wrap-mode=nodownload", "-Db_staticpic=true", "-Denable_tools=false", "-Denable_tests=false"])
        self.run(["meson", "compile", "-C", self.root / "dav1d-build", "-j", self.jobs])
        self.run(["meson", "install", "-C", self.root / "dav1d-build"])
        self.env.update(PKG_CONFIG_PATH=str(prefix / "lib/pkgconfig"), PKG_CONFIG_LIBDIR=str(prefix / "lib/pkgconfig"))
        flags = [*CONFIGURE, f"--prefix={prefix.as_posix()}", "--pkg-config-flags=--static"]
        if self.windows:
            flags.extend(["--target-os=mingw32", "--disable-pthreads", "--enable-w32threads", "--extra-ldflags=-static-libgcc"])
            self.run([self.msys / "usr/bin/bash.exe", "--noprofile", "--norc", "-c", "./configure " + shlex.join(flags)], media)
        else:
            self.run([media / "configure", *flags], media)
        self.run(["make", f"-j{self.jobs}", "install"], media)
        cargo_config = source / ".cargo/config.toml"
        cargo_config.parent.mkdir(exist_ok=True)
        config = self.run([self.cargo, "vendor", "--locked", "vendor"], source, capture=True, timeout=600)
        cargo_config.write_text(config, encoding="utf-8")
        if self.windows:
            self.env["RUSTFLAGS"] = "-C target-feature=+crt-static -C link-arg=-static-libgcc -C link-arg=-static-libstdc++"
        self.run([self.cargo, "build", "--release", "--frozen", "--offline", "--target", self.rust_target, "--features", "ffmpeg/static"], source)
        self.deliver(source, media, decoder, archives)

    def deliver(self, source, media, decoder, archives):
        delivery = self.root / "delivery"
        delivery.mkdir()
        executable = "grav1synth.exe" if self.windows else "grav1synth"
        shutil.copy2(self.root / "target" / self.rust_target / "release" / executable, delivery / executable)
        sources = delivery / "sources"
        sources.mkdir()
        records = {}
        for name, archive in archives.items():
            path = sources / self.lock[name]["filename"]
            shutil.copy2(archive, path)
            records[name] = {**self.lock[name], "path": path.relative_to(delivery).as_posix()}
        vendor = sources / "cargo-vendor.tar.gz"
        with tarfile.open(vendor, "w:gz", dereference=True) as bundle:
            bundle.add(source / "vendor", arcname="vendor")
            bundle.add(source / ".cargo/config.toml", arcname=".cargo/config.toml")
        records["vendor"] = {"id": "cargo-vendor", "path": vendor.relative_to(delivery).as_posix(), "sha256": MEDIA.digest(vendor)}
        licenses = delivery / "licenses"
        licenses.mkdir()
        for name, origin in [("grav1synth-MIT", source / "LICENSE"), ("ffmpeg-LGPL", media / "COPYING.LGPLv2.1"), ("ffmpeg-license", media / "LICENSE.md"), ("dav1d-BSD", decoder / "COPYING"), ("dav1d-patents", decoder / "doc/PATENTS")]:
            shutil.copy2(origin, licenses / (name + ".txt"))
        # The untouched Cargo vendor includes every crate's original licenses.
        build = delivery / "build"
        build.mkdir()
        for original in self.inputs.iterdir():
            shutil.copy2(original, build / original.name)
        shutil.copy2(source / "Cargo.lock", build / "Cargo.lock")
        qualification = delivery / "qualification"
        qualification.mkdir()
        listed = self.run([delivery / executable, "presets"], capture=True, timeout=20)
        help_text = self.run([delivery / executable, "apply", "--help"], capture=True, timeout=20)
        if not all(f"  {name}  (" in listed for name in ["Super8", "MaxMid", "16mm", "Classic35", "Modern35"]) or not all(value in help_text for value in ["--preset", "--iso", "--replace"]):
            raise ValueError("The built grav1synth lacks the source-pinned command capabilities.")
        (qualification / "presets.txt").write_text(listed, encoding="utf-8")
        (qualification / "apply-help.txt").write_text(help_text, encoding="utf-8")
        dependencies = self.run(([self.msys / "ucrt64/bin/objdump.exe", "-p", delivery / executable] if self.windows else ["otool", "-L", delivery / executable] if sys.platform == "darwin" else ["ldd", delivery / executable]), capture=True, timeout=20)
        if "avcodec" in dependencies or "avformat" in dependencies or "dav1d" in dependencies:
            raise ValueError("The provider unexpectedly depends on external media libraries.")
        if self.windows:
            imports = {line.split("DLL Name:", 1)[1].strip().lower() for line in dependencies.splitlines() if "DLL Name:" in line}
            allowed = {"advapi32.dll", "bcrypt.dll", "crypt32.dll", "gdi32.dll", "iphlpapi.dll", "kernel32.dll", "msvcrt.dll", "ntdll.dll", "ole32.dll", "oleaut32.dll", "secur32.dll", "shell32.dll", "shlwapi.dll", "user32.dll", "userenv.dll", "version.dll", "winmm.dll", "ws2_32.dll", "ucrtbase.dll", "normaliz.dll", "psapi.dll"}
            if not imports or any(name not in allowed and not name.startswith("api-ms-win-") for name in imports):
                raise ValueError(f"The Windows provider imports an unqualified runtimeDLL: {sorted(imports)}")
            compiler_lock = json.loads((HERE / "package-ffmpeg-windows-lock.json").read_text(encoding="utf-8"))
            runtime = [item for item in compiler_lock["linkedSources"] if item["base"] in {"mingw-w64-gcc", "mingw-w64-crt", "mingw-w64-headers", "mingw-w64-winpthreads"}]
            if len(runtime) != 4:
                raise ValueError("The Windows compiler runtime sources are incomplete.")
            for item in runtime:
                archive = STAGER.download(item["url"], item["sha256"], self.cache)
                shutil.copy2(archive, sources / item["filename"])
                records[item["base"]] = {**item, "id": item["base"], "path": "sources/" + item["filename"]}
            for original in (self.msys / "ucrt64/share/licenses").rglob("*"):
                if original.is_file() and any(part in {"gcc", "gcc-libs", "crt", "headers", "winpthreads"} for part in original.parts):
                    shutil.copy2(original, licenses / ("runtime-" + original.relative_to(self.msys / "ucrt64/share/licenses").as_posix().replace("/", "__")))
        (qualification / "dependencies.txt").write_text(dependencies, encoding="utf-8")
        shutil.copy2(self.root / "build.log", qualification / "build-commands.log")
        receipt = {"schemaVersion": 1, "target": self.target, "rustTarget": self.rust_target, "sourceCommit": self.lock["source"]["commit"], "tool": {"id": "grav1synth", "path": executable, "sha256": MEDIA.digest(delivery / executable), "version": "source " + self.lock["source"]["commit"]}, "source": records["source"], "additionalSources": [{**records["ffmpeg"], "id": "ffmpeg"}, {**records["dav1d"], "id": "dav1d"}, records["vendor"], *[item for name,item in records.items() if name.startswith("mingw-")]], "licenses": [{**item, "path": "licenses/" + item["path"]} for item in inventory(licenses)], "buildInputs": [{**item, "path": "build/" + item["path"]} for item in inventory(build)], "supportFiles": [], "qualificationFiles": [{**item, "path": "qualification/" + item["path"]} for item in inventory(qualification)], "qualification": {"presets": self.lock["presets"], "mediaWorkflows": "Run the application grain_provider and utility_containers tests against this delivery; CLI qualification alone does not establish media correctness."}, "configure": CONFIGURE}
        (delivery / "build-provenance.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
        verify_delivery(delivery, self.target)
        print(f"Source-complete grav1synth delivery: {delivery}", flush=True)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--cache", type=Path, default=HERE.parent / "target/tool-download-cache")
    parser.add_argument("--jobs", type=int, default=4)
    parser.add_argument("--msys-root", type=Path)
    args = parser.parse_args()
    if not 1 <= args.jobs <= 16:
        parser.error("jobs must be between1and16")
    Build(args.destination, args.cache, args.jobs, args.msys_root).compile()

if __name__ == "__main__":
    main()
