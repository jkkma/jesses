# Windows portable av1an

The Windows package recipe builds av1an, VapourSynth and L-SMASH Works from
pinned source archives. Its private Python installation is packaged alongside
the engine. It does not register a frameserver or modify another Python install.

| Component              | Pinned source                                                 |
| ---------------------- | ------------------------------------------------------------- |
| av1an                  | v0.6.0, `641ce62d70e34f438f3ee91ec50de77784a10705`            |
| VapourSynth            | R82, `e0fadcc1c8977e99b11ce2e478996a867a939874`               |
| L-SMASH Works          | `7e63c8400a252f4dbbeea80cc3bfec895abf12cf`                    |
| L-SMASH decoder FFmpeg | Its required fork, `39c24f36247f370864ce86ebdf2aa936151f4bfc` |
| Python                 | Official CPython 3.15.2 embedded runtime and complete source  |

`scripts/package-av1an-windows-lock.json` also pins zimg, graphengine, libp2p,
glslang, Vulkan-Headers, Cython, Meson, L-SMASH's submodules, the Python
development package, its published SBOM
and every non-CPython source archive listed by that SBOM. Cargo resolves the
pinned av1an lockfile, vendors its full dependency sources, and compiles offline.
The delivery includes that vendor archive and its checksum metadata. The exact
shared dav1d, libvpx, zlib and compiler-runtime sources are referenced from the
verified FFmpeg delivery rather than duplicated in every tool directory.

The compatibility patch resets timestamps and uses FFmpeg's passthrough mode
for select-generated chunks. Upstream already uses passthrough for keyframe
Segment splitting and sampled quality probes. The patch also corrects Julek's
case-sensitive Butteraugli function name. L-SMASH metric probes use software
decoding to avoid hardware decoder failures on short AV1 probes. VMAF and every-frame XPSNR
clear only matrix negotiation tags before both metric scale inputs, preventing
FFmpeg from inserting a color conversion between tagged encoded frames and an
untagged Y4M reference. Independent decoded-file comparisons match every frame
and aggregate metric after this correction. The version banner identifies the
source revision and the retained compatibility markers. The
`preload-scenes-v1` marker identifies an engine that can reuse precomputed
scenes for a fresh encode.

The `target-probe-filter-v1` patch also applies the final chunk's FFmpeg input
filters to quality probe encodes. This keeps cropped and resized probes aligned
with their VMAF scoring references. Both paths apply transforms before sampling,
so temporal filters retain the surrounding frames they need. The scoring-only
reference filter remains separate. Probe filter arguments are retained in the recovery queue and checked
before resuming. Older queues remain compatible when no input filter is needed;
filtered quality-target jobs require an engine carrying the new marker.
For non-VMAF targets with crop, scale, borders, tone-map or frame-mode
deinterlace transforms, Jesses first writes and verifies a lossless FFV1 source.
Scene detection, probe encodes, final chunks and metric reference readers then
use those same processed frames without applying the transforms again. Recovery
pins both the original source and the decoded processed pixels. The direct
`--ffmpeg` filtered-probe path remains VMAF-only because the other upstream
reference readers omit those filters; bypassing preparation is rejected.

## Build

Use Python 3.14 or later, Rust 1.100.0 for native Windows x64, and Visual Studio's
MSVC x64 tools and Windows SDK. First build the verified media compiler and
FFmpeg delivery described in [packaging](packaging.md), then run:

```sh
python scripts/build-package-av1an.py --destination target/package-av1an --ffmpeg-build target/package-ffmpeg/delivery --msys-root target/package-media-compiler/msys64
```

The recipe uses a fresh destination and retains build logs when a step fails.
Compiler and library configuration are isolated from ambient compiler flags,
Python/module paths and shell startup files. MSVC is selected ahead of MSYS
utilities, and the recipe records its version and compiler hash. The engine's
scene detection dependency uses the NASM bundled in the locked media compiler.
The locked MSYS comparison utility used during FFmpeg configuration is unpacked
into the build directory.
No MSYS package hooks or global installation commands are executed.

Add `--av1an-build target/package-av1an/delivery` when staging bundled tools.
The stager requires the exact shared FFmpeg receipt and verifies every runtime,
source, build and license file before invoking the engine. Binary input pinning
and source retention do not establish byte-identical builds across compilers or
hosts; native Linux av1an packaging is deferred.

The Windows workflow additionally builds the CPU scorer pair with
`package-av1an-scorers.py` and merges it into a fresh delivery with
`add-package-av1an-scorers.py`. The scorer lock pins vszip 24.0.0, Julek r3,
their complete Zig/JPEG XL dependencies and the build tools. Julek uses a static
MSVC runtime; native import checks require every remaining import to be a
Windows system DLL. The merge retains the original source archives, notices,
compiler recipe and hashes and refuses to replace an existing runtime file.

The next Windows extension supplies QTGMC's six supported presets:

```sh
python scripts/build-package-av1an-qtgmc.py --destination target/package-qtgmc --cache target/qtgmc-archive-cache --ninja target/package-media-compiler/msys64/ucrt64/bin/ninja.exe
python scripts/package-av1an-qtgmc.py --destination target/package-av1an-qtgmc --av1an-build target/package-av1an-scorers/delivery --qtgmc-build target/package-qtgmc/delivery --ffmpeg-build target/package-ffmpeg/delivery --msys-root target/package-media-compiler/msys64
```

Use the resulting `target/package-av1an-qtgmc/delivery` for `--av1an-build`
when staging. `scripts/qtgmc-windows-lock.json` pins the Python modules, native
plugins, FFTW and their corresponding sources. FFT3DFilter links statically
to FFTW 3.3.11, and both FFT3DFilter and MiscFilters use the static MSVC
runtime. EEDI3 r10, MVTools v29_2 and fmtconv r31 use the x64 DLLs from their
upstream wheels. The delivery also includes the MSYS2 source package for
the FFTW 3.3.11 library linked into the MVTools wheel. Source archives, license notices, recipes, compiler details
and output hashes accompany the extension.

The `havsfunc` and `vsutil` modules live in `python/qtgmc-deps.zip`, listed in
the embedded interpreter's search path. The existing `havsfunc` revision
provides the QTGMC function used by all six presets. Upstream removed
`havsfunc.QTGMC` before publishing r34. Importing the pinned modules leaves
the manifest-verified runtime unchanged.

VapourSynth R82 removed API3 plugin loading. The previous TemporalSoften2
bundle used only API3, and the six QTGMC presets do not call it.

## Runtime and native gates

The engine and frameserver use this package layout:

```text
resources/tools/av1an/av1an.exe
resources/tools/av1an/python/python.exe
resources/tools/av1an/python/Lib/site-packages/vapoursynth/vsscript.dll
resources/tools/av1an/python/Lib/site-packages/vapoursynth/vspipe.exe
resources/tools/av1an/python/Lib/site-packages/vapoursynth/plugins/LSMASHSource.dll
```

Runtime discovery verifies hashes and the complete Python/frameserver inventory.
An extra unlisted plugin, changed DLL, missing file or directory redirection is
rejected. For the bundled engine only, its owned child receives the explicit
frameserver path and selected tools, with external Python and plugin overrides
removed. Parent-process environment and explicit external-engine setups are
preserved. A readable initializer retained under `build` is compiled into the
package to prevent frameserver startup from writing Python bytecode into the
installed resources.

The builder checks native library imports, decodes actual FFV1 frames through
the packaged L-SMASH plugin, checks av1an's plugin discovery and compares the
complete runtime inventory before and after execution. The separate
`qualify-package-av1an.py` gate runs the actual encode/cancel tests with external
PATH tools unavailable and deliberately conflicting frameserver/Python settings.
It retains the test-runner hash, manifest hash, exit status and unchanged-runtime
result. Package workflows also check discovery from extracted installer resources.

The base frameserver includes L-SMASH. The CPU extension provides SSIMULACRA2,
sampled XPSNR and Julek Butteraugli; its native gates evaluate actual frames and
compare the complete runtime inventory before and after execution. FFMS2 and
BestSource remain optional external readers. The manifest-verified package
reports both absent and the app rejects either selection before encoding. A
separate compatible external runtime passed both readers through fixed-chunk
stop, reopen and resume; that result does not add them to the packaged inventory.

The QTGMC merge checks native DLL dependencies, evaluates real frames for Faster,
Fast, Medium, Slow, Slower and Very Slow, and repeats all three CPU scorer probes
after adding the plugins. Every probe must preserve the complete Python/runtime
inventory. These component gates supplement the application's native export
checks; they do not qualify optional external readers or GPU plugins.

Windows packages include the pinned Vship 5.1.2 x64 Vulkan plugin, its complete
source archive and MIT license. `build-package-vship.py` verifies the locked
downloads; `package-av1an-vship.py` merges the delivery after CPU scorers and
QTGMC. The DLL lives in `vapoursynth/optional-plugins`, outside automatic plugin
loading, and is covered by the complete runtime hash inventory.

For an SSIMULACRA2 or Butteraugli job, Jesses first loads Vship in an isolated
VSPipe process and requires a finite result from actual frames. A successful
probe enables the plugin only in that job's child environment. A missing Vulkan
loader, ABI mismatch, unsupported GPU, failed metric or probe timeout keeps the
CPU vszip/Julek scorer selected. Package builds and CPU-only machines do not
require Vulkan. No manual setup or system Python changes are needed.

Job logs identify the selected scorer. GPU and CPU implementations can produce
different values, so saved av1an searches keep their original scorer when resumed.
Existing CPU-scored jobs remain on CPU after an upgrade. A GPU-scored search
requires the same plugin bytes and a working GPU to resume. If GPU scoring fails
on an actual CRF ladder sample, Jesses discards the partial scores and rescores
every rung on CPU so the recommendation never mixes scoring implementations.

For developers using a separate external portable runtime, the optional
`scripts/install-vship.ps1` installer remains available from a source checkout:

```powershell
pwsh -File scripts/install-vship.ps1 -PortableRuntime <external-portable-vapoursynth-dir> -Activate
```

That external-runtime installer refuses reparse targets, destination escapes,
existing different plugins and manifest-verified application bundles. Packaged
Vship is supplied by the package recipe and requires no post-install mutation.
Consult a package's manifest before selecting an optional external reader.
