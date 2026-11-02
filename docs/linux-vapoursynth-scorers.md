# Pinned Linux VapourSynth reader and CPU scorers

`scripts/build-vapoursynth-linux-scorers.py` builds the Linux x64 L-SMASH reader,
vszip SSIMULACRA2 scorer and Julek Butteraugli scorer from the same source revisions
as the Windows bundle. It uses the existing av1an/scorer source locks plus the
Linux Zig 0.16.0 binary pin in `scripts/vapoursynth-linux-scorers-lock.json`.

The build needs GCC/G++, CMake 3.28+, Ninja, make, nasm, pkg-config, Python 3.12+
and its development headers. It also requires two explicit, existing inputs:

- A complete [Linux FFmpeg package build](linux-ffmpeg-package.md), including its
  `prefix` development libraries and verified `delivery`. The reader uses the
  exact pinned dav1d, libvpx and zlib static libraries from that build.
- A native VapourSynth **R79** Python package/SDK, including `libvsscript.so`,
  `libvapoursynth.so.4` and `include/`. Its source revision and checksum are pinned
  in `scripts/package-av1an-windows-lock.json`; the builder compares the SDK
  headers with that source and checks the running core release. Supply the
  interpreter that imports this installation. Building R79 from its pinned source
  requires Meson 1.12.0, Cython 3.3.0 and Python development headers; its native
  `meson setup`, `meson compile` and `meson install` must use a task-specific prefix.
  Existing user runtimes and configuration are not overwritten.

```sh
python3 scripts/test-vapoursynth-linux-scorers.py
python3 scripts/build-vapoursynth-linux-scorers.py \
  --destination /tmp/jesses-linux-scorers \
  --media-build /tmp/jesses-ffmpeg-build \
  --vapoursynth-root /absolute/path/to/R79/python/package/vapoursynth \
  --python /absolute/path/to/compatible/python \
  --cache /tmp/jesses-source-cache --jobs 2
. /tmp/jesses-linux-scorers/env.sh
```

The destination must not exist. Failed builds remain available for inspection;
there is no implicit resume, deletion or replacement of existing outputs. The
three build streams have separate logs. Sources and original license/patent
notices are retained in `provenance/`, together with the complete build scripts,
locks, shared-codec source closure and hash receipt. Zig dependencies resolve to
retained local trees, so compilation does not fetch moving dependencies.

The Linux recipe enables PkgConfig for libjxl's CMake checks. Julek uses
`-fno-gnu-unique` to reconcile its C++17 inline constexpr tables with the pinned
C++11 libjxl definitions. L-SMASH binds its private static FFmpeg symbols locally
with `-Bsymbolic`, and keeps `--no-undefined`; linker errors are not suppressed.
No source versions are substituted.

Before reporting success, the builder loads all three plugins under R79, reads
four actual FFV1 frames through L-SMASH, and evaluates two differing generated
frames with each CPU metric. It requires finite scores. It writes only to its own
VapourSynth configuration tree. `env.sh` selects the supplied Python/runtime,
plugin directory and verified FFmpeg pair for this shell; sourcing it does not
change shell startup files or system installation paths.

Existing artifacts can be rechecked without compiling or modifying them:

```sh
python3 scripts/build-vapoursynth-linux-scorers.py \
  --verify-existing /tmp/jesses-linux-scorers \
  --media-build /tmp/jesses-ffmpeg-build \
  --vapoursynth-root /absolute/path/to/R79/python/package/vapoursynth \
  --python /absolute/path/to/compatible/python \
  --cache /tmp/jesses-source-cache
cargo test -p media-runtime --test utility_jobs portable_cpu_ladder_metrics \
  --locked -- --ignored --nocapture
```

Verification checks plugin/provenance hashes and the retained source cache before
loading plugins, and uses a temporary private configuration. The runtime test
also checks real encoded samples, every retained frame and source preservation.
These Linux results do not establish macOS, Windows or GPU qualification.
