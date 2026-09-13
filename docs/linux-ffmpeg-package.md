# Linux FFmpeg package build

`scripts/build-package-ffmpeg-linux.py` builds FFmpeg and FFprobe 9.0.1 for Linux
x86-64. It compiles the linked codec, display-conversion and compression libraries
from the exact source archives in `scripts/package-ffmpeg-linux-lock.json`.
Codec libraries are static; the Linux C/C++ runtimes remain ordinary system
dependencies. This is an Ubuntu 24.04 build lane, not a universal Linux binary.

The lock contains complete MSYS2 source bundles for bzip2, dav1d, LAME, libogg,
libvorbis, libvpx, x264, Opus, x265, XZ, zimg, zlib, libass, VMAF, FreeType,
HarfBuzz, FriBidi, fontconfig, Expat, libunibreak, Brotli and libpng. Meson 1.12.0
also runs from a pinned complete source bundle; the host's older Meson is not used.
The builder verifies both
the outer bundle and each embedded upstream archive. For x264 it checks out the
locked commit from the bundled complete local Git repository. It does not run
MSYS2 package recipes. The recorded x265 missing-include patch is applied to the
upstream source; its bytes and checksum are retained in the source bundle.

x265 is built at 8, 10 and 12 bits and merged into one static library. FFmpeg has
automatic optional-library detection disabled. The final executable dependency
check rejects codec shared libraries, missing libraries and dependencies on the
temporary build prefix. Version and actual short encode/display checks run before
the delivery receipt is written. A real VMAF v0.6.1 model run detects missing
embedded model data, and an original geometric font tests actual libass rendering.
FreeType includes compressed and PNG font support. Fontconfig uses standard Linux
font/configuration paths; an isolated staged install does not write to `/etc`.
The packaging workflow also runs media-runtime
fixture gates using the newly built pair.

## Build and retained sources

Use Python 3.14 or later and the Linux prerequisites listed at the top of the build
script. A new build directory is required:

```sh
python scripts/test-package-ffmpeg-linux.py
python scripts/build-package-ffmpeg-linux.py --build-directory /tmp/jesses-ffmpeg-build --cache /tmp/jesses-source-cache
```

`/tmp/jesses-ffmpeg-build/delivery` contains the two executables, official FFmpeg
source, all linked-library source bundles, exact source-derived license/patent
texts, the build script and lock, executed build commands, compiler/tool versions,
build log, ELF dependency reports and `build-provenance.json`. Every retained
file has a SHA-256 entry in the delivery receipt. Tool staging checks this exact
inventory before copying it into app resources.

The delivered build script locates its lock beside itself. The retained source
archives can serve as its cache, so a rebuild can avoid network downloads:

```sh
python delivery/build/build-package-ffmpeg-linux.py --build-directory /tmp/jesses-rebuild --cache delivery/sources
```

Build commands and source revisions are recorded, but identical binary hashes
across different compiler/system versions have not been established. Rebuild
with the recorded toolchain when comparing outputs.

## Current evidence

On Windows, `--verify-only` completed for all 24 locked archives (FFmpeg, 22 linked
libraries and Meson), their source licenses and the exact local x264 Git checkout. The cross-platform safety
tests cover archive traversal/duplicates/links, empty Git directories, tampered
cache retention, exact source/license hashes, refusing an existing build directory,
compiler/shell environment isolation, and rejection of unbundled dynamic codecs.

The complete font/rendering/VMAF/Meson closure has a source verification receipt.
The seven cross-platform safety tests passed. Font-pixel and built-in VMAF smoke
commands also passed on installed Windows FFmpeg; this checks the commands, not
the Linux build. Native Linux build and pixel/model gates require workflow execution.

These checks do not compile or run Linux binaries. Native Linux compilation,
codec fixture results and final AppImage/DEB resource discovery remain separate
workflow qualification. macOS remains deferred.

## Linux standalone x264 and mainline SVT-AV1

After the FFmpeg delivery has passed its native gates, build the two standalone
encoders from the exact retained source revisions:

```sh
python scripts/build-package-standalone-linux.py \
  --build-directory target/package-standalone-linux \
  --ffmpeg-build target/package-ffmpeg/delivery \
  --cache target/tool-download-cache --jobs 4
```

Use the actual FFmpeg delivery directory from the preceding build. The outputs
are `target/package-standalone-linux/x264/delivery` and
`target/package-standalone-linux/svt-av1/delivery`, accepted by the stager's existing
`--x264-build` and `--svt-build` options. The driver requires the same Linux x86-64
GCC/G++, binutils, make, nasm, CMake, Ninja and Git toolchain as the media build;
it does not install packages or run a package manager. The workflow uses its
pinned runner image and records the installed build-tool versions. This retains
exact codec sources and recipes, without claiming byte-identical binaries across
different compiler or system-runtime versions.

The entire shared FFmpeg delivery is hash-verified before use. x264 checks out
`b35605ace3ddf7c1a5d67a2eb553f034aef41d55` from its retained, bounded source archive;
it has no optional lavf, swscale, AviSynth or OpenCL dependency. Mainline SVT-AV1
4.2.0 is built from pinned commit `9292ec8e32bce26f781f277ec8739b53426c4300` with a
static codec library, tests and PGO disabled, baseline CPU flags and disconnected
CMake dependency fetching. Both binaries must have the expected identity, no
RPATH/RUNPATH, and only the declared standard Linux C/compiler shared libraries.
No private codec library is left in a temporary build prefix. Each encoder then
performs actual eight-frame 8-bit and 10-bit encoding followed by full frame
count, dimensions and pixel-depth decoding checks using the built FFmpeg pair.

Each delivery keeps its exact source reference, source-derived notices, recipes,
locks, command log, compiler versions and smoke receipt. The x264 source archive
is shared with the FFmpeg delivery by exact record and provenance hash. SVT's
complete source archive is included in its own delivery. Standard glibc/compiler
shared libraries are declared system dependencies; they are not silently bundled
or replaced with an unrelated Windows runtime source list.

To rebuild from a delivery, copy its `build` recipe/lock files into one directory,
provide the exact accompanying FFmpeg delivery, and put the retained SVT source
archive in the cache under its SHA-256 filename (or allow the pinned HTTPS download).
The x264 source is read directly from the verified FFmpeg delivery. A fresh output
build directory is required; previous or failed build files are never replaced.

The cross-platform `--verify-only` option validates the pinned source archives,
local Git revision and source notices without producing executables. On
2026-09-13 that check passed for both source archives. Three integrity tests passed
for clean build environments, complete shared delivery inventories and preservation
of an existing build directory. The exact 8-/10-bit smoke command sequence also
passed against already-built Windows tools; that is command validation only.
**Linux compilation, native standalone smoke, packaging and GUI execution remain
pending a Linux build host.** The script never labels source-only verification as
a completed Linux build.
