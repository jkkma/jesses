# Desktop packages and data locations

The manual **Unsigned desktop packages** workflow builds a Windows x64 installer
and portable ZIP, plus Linux x64 AppImage and DEB packages. It uploads artifacts
to that workflow run and does not publish a release. macOS remains deferred.

The Windows installer uses zlib compression. Most source payloads are already
compressed archives; zlib keeps complete-source package iteration practical
while retaining the same verified installer contents.

The workflow includes the pinned SVT-AV1 5fish and SVT-AV1-HDR binaries, their
matching complete source archives and license/patent notices. These upstream
builds require an x86-64-v3 CPU. Each native packaging run checks their versions
before bundling them. Binary and source downloads have pinned SHA-256 hashes.

Windows package builds also compile the official FFmpeg/FFprobe 9.0.1 source
release with explicit x264, x265, VP9, Opus, MP3, Vorbis, dav1d, zimg, libass
subtitle rendering and VMAF support.
All codec libraries are linked statically; native qualification checks that the
executables import only Windows system DLLs. The isolated compiler is assembled
from SHA-256 pinned official MSYS2 payloads without changing the user's tools,
executing package hooks or consulting a moving package index. The package retains
the exact codec/runtime sources, notices, input lock and build scripts.
VMAF is built with its default models embedded. The build requires the pinned
model generator and rejects a library that silently omits those models. Windows
subtitle rendering uses the operating system's DirectWrite font provider.

The [Linux FFmpeg build](linux-ffmpeg-package.md) compiles the same media pair
and static codec libraries from pinned sources, with the ordinary Linux C/C++
runtime supplied by the target system. Its recipe includes native dependency
and media gates. Source extraction and license checks run on Windows too; they
do not establish a Linux compilation or execution pass.

Windows package builds additionally compile standalone x264 from the pinned
complete Git source already retained with FFmpeg. The standalone command supports
8-bit and 10-bit encoding and imports only Windows system libraries. Its receipt
references the shared source and compiler-runtime notices by exact hashes.

Windows packages also compile mainline SVT-AV1 4.2.0 from its pinned complete
upstream source archive. Its source, BSD and patent notices, pinned CMake input
and build recipe accompany the executable, with the shared compiler-runtime sources.

Windows builds include a [portable av1an frameserver](windows-av1an-package.md):
the pinned engine, VapourSynth R79, L-SMASH and private CPython runtime, with
complete sources, license notices, build receipts and per-child environment
selection. The workflow adds source-built vszip and Julek CPU scorers in a
separate verified merge. Linux also has separate standalone x264/mainline SVT source recipes
and native package gates, described in the Linux build document. Their source
verification passed locally; Linux compilation and execution remain unverified.
Linux av1an packaging is still separate work.
Install the remaining tools described in
[standalone encoders](standalone-encoders.md) and
[SVT forks](svt-forks.md), then check **Tools & settings**.

Explicit `JESSES_<TOOL_ID>` executable overrides take priority, with hyphens in
tool IDs replaced by underscores; for example `JESSES_FFMPEG` or
`JESSES_SVT_AV1_HDR`. Forks retain their existing managed-install priority. Next,
the runtime verifies any corresponding bundled executable's SHA-256, then uses
the permitted PATH fallback for tools that are not bundled. A corrupt bundled
entry fails explicitly. No PATH mutation or alternate encoder substitution is
performed. The selected tool is validated again before each media job.

The Windows installer uses a per-user installation and downloads the WebView2
bootstrapper if the runtime is missing. That first installation requires internet
access. The portable ZIP requires an already installed WebView2 runtime. An offline
runtime bundle, signing and clean-machine installation trials remain open gates.
Linux packages are built on Ubuntu 24.04; other distributions and older system
libraries need separate native qualification.

## Storage

An ordinary installed executable preserves the existing platform data directory
and its `jobs` history subdirectory. Logs remain under the platform log directory.
Application resources are never used as writable storage.

The Windows portable ZIP contains a regular `jesses.portable` file with `1` as its
version. Its presence beside the executable selects `jesses-data` in that same
directory, with separate `config`, `data`, `cache` and `logs` subdirectories. Saved
jobs go to `jesses-data/data/jobs`; job logs go to `jesses-data/logs/jobs`. Place the
portable folder somewhere writable. An invalid marker, redirected data directory
or write failure is reported; the app does not silently switch to another history
store. Portable mode never imports or changes installed history.

Keep the complete `jesses-data` directory when moving a portable installation.
Recovery records retain absolute source/output paths, which are validated before
resume. Do not move source media or saved recovery workspaces while a job is kept
for resume. Cache cleanup must not delete `data` or job recovery folders.

## Local packaging

Use the pinned Node, pnpm and Rust versions and normal native build prerequisites.
Run the packaging invariant checks first:

```sh
python scripts/test-package-desktop.py
python scripts/test-package-toolchain.py
python scripts/test-package-av1an.py
cargo test -p jesses --lib --locked paths::tests
```

Build the platform bundle from a fresh target output. For Windows:

```sh
python scripts/stage-bundled-tools.py --destination target/packaged-tools --target x86_64-pc-windows-msvc
pnpm tauri build --target x86_64-pc-windows-msvc --bundles nsis --config target/packaged-tools/tauri-tools.conf.json
python scripts/package-desktop.py collect --build-directory target/x86_64-pc-windows-msvc/release --destination target/unsigned-windows-package --target x86_64-pc-windows-msvc --profile release --tool-resources target/packaged-tools/resources/tools
```

To include the source-built Windows FFmpeg pair, run the following before staging
and add `--ffmpeg-build target/package-ffmpeg/delivery` to the staging command.
Python 3.14 or later is required for the pinned Zstandard-compressed inputs.

```sh
python scripts/bootstrap-package-toolchain.py --destination target/package-media-compiler
python scripts/build-package-ffmpeg.py --destination target/package-ffmpeg --msys-root target/package-media-compiler/msys64
python scripts/build-package-x264.py --destination target/package-x264 --ffmpeg-build target/package-ffmpeg/delivery --msys-root target/package-media-compiler/msys64
python scripts/build-package-svt.py --destination target/package-svt --ffmpeg-build target/package-ffmpeg/delivery --msys-root target/package-media-compiler/msys64
python scripts/build-package-av1an.py --destination target/package-av1an --ffmpeg-build target/package-ffmpeg/delivery --msys-root target/package-media-compiler/msys64
```

Add `--x264-build target/package-x264/delivery --svt-build target/package-svt/delivery --av1an-build target/package-av1an/delivery`
when staging the Windows tools.
The x264 build requires that exact FFmpeg source delivery so the final package
can retain a single shared copy of the corresponding sources. Its build receipt
also records the Git version used for the verified local source checkout.

The build scripts retain failed build directories and logs. A compiler assembled
with the bootstrap script is required: the build checks its input receipt before
compiling. Configuration disables optional library auto-detection, and the local
x265 pkg-config override selects GCC's static unwinder consistently. It does not
modify the upstream codec binary or source. Input pinning provides repeatable
build inputs; byte-for-byte reproducibility across hosts remains a separate gate.

For Linux, use `--target x86_64-unknown-linux-gnu --bundles appimage,deb` and the
corresponding target directory. Every packaging destination must be new: scripts
do not overwrite existing artifacts. If multiple old installers exist in the
build's bundle directory, collection refuses the ambiguous result.

`collect` copies installers and license/dependency records, produces the Windows
portable ZIP, and records SHA-256 hashes in `SHA256SUMS` and JSON manifests.
The manifest records the packaging checkout and whether it had local changes;
the binary hash identifies the supplied executable. This is not an attestation
that an independently supplied binary was built from that checkout.

To inspect an assembled portable directory:

```sh
python scripts/package-desktop.py verify target/unsigned-windows-package/portable
python scripts/package-desktop.py diagnostics target/unsigned-windows-package/portable --sanitized-path
```

The native tool probe exercises bundled discovery with external tool overrides,
managed installations and the ordinary tool search path removed. After building
the probe, verify both the staging directory and the resources extracted from the
actual installer. Extraction does not install the application.

```sh
cargo build -p media-runtime --example package_tools --locked
python scripts/qualify-package-tools.py --probe target/debug/examples/package_tools.exe --resources target/packaged-tools --require-media
python scripts/verify-bundle-resources.py --packages target/unsigned-windows-package --probe target/debug/examples/package_tools.exe --destination target/extracted-windows-package --require-media
```

Omit `--require-media` for an intentionally fork-only package. On Linux the probe
filename has no `.exe` suffix, and the artifact verifier extracts both AppImage
and DEB payloads before checking their native tool discovery.
Use `--require-tool x264 --require-tool svt-av1` with both verification commands
for the Windows package.

Verification checks the complete file inventory, sizes, hashes, marker and required
resources, including the bundled executable/source/license hashes. Diagnostics
reports the bundled inventory and dependencies visible on a restricted PATH. It does
not launch the native app, inspect its managed encoder installs, install tools,
touch job history, or establish clean-machine/runtime qualification. Complete
release qualification still requires native import, media output, cancellation,
recovery and platform resource discovery from the final installed artifacts.
