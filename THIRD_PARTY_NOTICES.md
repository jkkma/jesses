# Third-party notices

jesses is created by jkkma and licensed under GPL-3.0-only.

The Button source in `src/lib/components/ui/button` was generated from
[shadcn-svelte](https://github.com/huntabyte/shadcn-svelte), version 1.6.1,
and adapted for the jesses theme. Its MIT notice is preserved in
`licenses/shadcn-svelte-MIT.txt`.

Frontend and Rust dependency versions are recorded in `pnpm-lock.yaml` and
`Cargo.lock`. Their respective licenses continue to apply.

The optional packaged SVT-AV1 5fish and SVT-AV1-HDR executables are separate
upstream programs. Their BSD 3-Clause Clear, BSD 2-Clause and patent notices are
included beside each executable under `resources/tools`, together with the
complete source archive for the pinned upstream commit. The packaged
`resources/tools/manifest.json` records executable/archive/source hashes,
versions and upstream URLs. Build inputs are pinned in `scripts/svt-forks.json`
and `scripts/bundled-tool-sources.json`.

The optional packaged FFmpeg and FFprobe 9.0.1 pair is built from the official
release source with GPL components enabled. Its delivery includes the complete
FFmpeg and linked codec-library source archives, original license and patent
notices, exact build input hashes, and build recipes under
`resources/tools/ffmpeg`. The codec libraries include x264, x265, libvpx, Opus,
LAME, Vorbis/Ogg, dav1d, zimg, zlib, bzip2, XZ, libass, its font-rendering
dependencies and VMAF. Each component retains its own
copyright, license and any applicable runtime exception. Windows builds also
include corresponding GCC and MinGW runtime sources and notices.
VMAF's default scoring models are compiled from the same retained source tree.
The source and license inventory lists every linked font and text library.

The optional Windows standalone x264 executable is compiled separately from the
same pinned x264 sources. It retains the GNU GPL version 2 or later notice, and
its manifest references the exact shared x264 and compiler/runtime sources and
notices. Its version and build receipt distinguish it from the library in FFmpeg.

The optional Windows mainline SVT-AV1 4.2.0 executable is built from a pinned
upstream source archive. Its complete source, BSD 3-Clause Clear and BSD 2-Clause
licenses and patent notice accompany the binary. The build recipe and pinned
CMake input record are retained, along with the shared compiler/runtime sources.

The optional Windows portable av1an bundle contains a source-built patched
engine, VapourSynth R79 and L-SMASH Works with its matching FFmpeg decoder fork.
Its private CPython 3.14.7 runtime is accompanied by complete CPython source,
the official runtime SBOM and source archives for its listed dependencies.
The package retains complete Cargo dependency sources and license notices,
VapourSynth's zimg/graphengine/libp2p sources, L-SMASH's exact submodules,
the compatibility patch and build recipes. Shared decoder and compiler/runtime
sources reference the same verified media delivery. Each component retains its
original license; notices and exact source hashes are indexed in the manifest.
See `docs/windows-av1an-package.md` and `scripts/package-av1an-windows-lock.json`.

The Windows CPU scorer extension retains vszip, Julek and their complete
vapoursynth-zig, zigimg, JPEG XL, Brotli, Highway and skcms sources and notices.
The pinned Zig compiler source and build input are also recorded. Scorer DLLs
are compiled locally with only Windows system DLL imports; the scorer delivery
and merged av1an receipt retain their source, build and payload hashes. See
`scripts/package-av1an-scorers-lock.json` for exact component identities.

The optional per-user Windows GPU scorer is Vship 5.1.1's prebuilt x64 Vulkan
plugin under the MIT license. Its binary and complete tagged source archive are
hash-pinned by `scripts/vship-windows-lock.json`; the installer retains the
archive and upstream license beside the managed binary before activating it in
one selected portable VapourSynth runtime.

The Linux package recipe compiles standalone x264 and mainline SVT-AV1 from the
same pinned sources and retains their notices. Ordinary glibc/compiler shared
libraries are declared system dependencies. Linux native build qualification
and av1an packaging are deferred.
Builds without the optional
tools manifest distribute no media executables. The manifest is the inventory of
what a particular package actually contains.
