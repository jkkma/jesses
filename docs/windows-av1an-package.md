# Windows portable av1an

The Windows package recipe builds av1an, VapourSynth and L-SMASH Works from
pinned source archives. Its private Python installation is packaged alongside
the engine. It does not register a frameserver or modify another Python install.

| Component              | Pinned source                                                 |
| ---------------------- | ------------------------------------------------------------- |
| av1an                  | `805dad69143fa0a81cfe2fb89c0b9e90a828ea72`, 0.5.2 unstable    |
| VapourSynth            | R79, `acabf605b2205b32d65859bb2736405719d2fafd`               |
| L-SMASH Works          | `7e63c8400a252f4dbbeea80cc3bfec895abf12cf`                    |
| L-SMASH decoder FFmpeg | Its required fork, `39c24f36247f370864ce86ebdf2aa936151f4bfc` |
| Python                 | Official CPython 3.14.7 embedded runtime and complete source  |

`scripts/package-av1an-windows-lock.json` also pins zimg, graphengine, libp2p,
Cython, L-SMASH's submodules, the Python development package, its published SBOM
and every non-CPython source archive listed by that SBOM. Cargo resolves the
pinned av1an lockfile, vendors its full dependency sources, and compiles offline.
The delivery includes that vendor archive and its checksum metadata. The exact
shared dav1d, libvpx, zlib and compiler-runtime sources are referenced from the
verified FFmpeg delivery rather than duplicated in every tool directory.

The retained compatibility patch resets timestamps and uses FFmpeg's passthrough
mode for select-generated chunks. The upstream sampled quality probe already
uses passthrough mode. The patch also corrects Julek's case-sensitive Butteraugli
function name. L-SMASH metric probes explicitly use software decoding, avoiding
hardware decoder failures on short AV1 probes. VMAF and every-frame XPSNR
clear only matrix negotiation tags before both metric scale inputs, preventing
FFmpeg from inserting a color conversion between tagged encoded frames and an
untagged Y4M reference. Independent decoded-file comparisons match every frame
and aggregate metric after this correction. Separate version markers identify
these behaviors. These are explicit
patches to the pinned source; the engine is not represented as an unchanged
upstream release.

## Build

Use Python 3.14 or later, Rust 1.98.1 for native Windows x64, and Visual Studio's
MSVC x64 tools and Windows SDK. First build the verified media compiler and
FFmpeg delivery described in [packaging](packaging.md), then run:

```sh
python scripts/build-package-av1an.py --destination target/package-av1an --ffmpeg-build target/package-ffmpeg/delivery --msys-root target/package-media-compiler/msys64
```

The recipe uses a fresh destination and retains build logs when a step fails.
Compiler and library configuration are isolated from ambient compiler flags,
Python/module paths and shell startup files. MSVC is selected ahead of MSYS
utilities; its version and compiler hash are recorded. The locked MSYS comparison
utility used during FFmpeg configuration is unpacked into the build directory.
No MSYS package hooks or global installation commands are executed.

Add `--av1an-build target/package-av1an/delivery` when staging bundled tools.
The stager requires the exact shared FFmpeg receipt and verifies every runtime,
source, build and license file before invoking the engine. Binary input pinning
and source retention do not establish byte-identical builds across compilers or
hosts; native Linux av1an packaging remains a separate qualification task.

The Windows workflow additionally builds the CPU scorer pair with
`package-av1an-scorers.py` and merges it into a fresh delivery with
`add-package-av1an-scorers.py`. The scorer lock pins vszip 22.1.0, Julek r3,
their complete Zig/JPEG XL dependencies and the build tools. Julek uses a static
MSVC runtime; native import checks require every remaining import to be a
Windows system DLL. The merge retains the original source archives, notices,
compiler recipe and hashes and refuses to replace an existing runtime file.

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
compare the complete runtime inventory before and after execution. GPU VShip,
FFMS2 and BestSource remain optional external components. Consult a package's
manifest and capability result before selecting an optional source reader or
metric. A successful development-machine test with external plugins does not
qualify their absence from a package.
