# Third-party notices

jesses is created by jkkma and licensed under GPL-3.0-only.

The Button source in `src/lib/components/ui/button` was generated from
[shadcn-svelte](https://github.com/huntabyte/shadcn-svelte), version 1.6.1,
and adapted for the jesses theme. Its MIT notice is preserved in
`licenses/shadcn-svelte-MIT.txt`.

Frontend and Rust dependency versions are recorded in `pnpm-lock.yaml` and
`Cargo.lock`. Their respective licenses continue to apply.

The native HDR metadata parsers `dolby_vision` 3.4.0 and `hdr10plus` 2.1.5
are Copyright (c) 2026 and 2025 quietvoid, respectively, and licensed under MIT.
Their notices are preserved in `licenses/dolby-vision-MIT.txt` and
`licenses/hdr10plus-MIT.txt`.
These exact Rust dependency versions are pinned in Cargo.toml and Cargo.lock;
metadata imports require no separate runtime CLI.

The packaged SVT-AV1 5fish v2.3.261118 and SVT-AV1-HDR v4.3.1 executables
come from their respective upstream projects. The Linux recipe uses the same
SVT-AV1-HDR release.
Their BSD 3-Clause Clear, BSD 2-Clause and patent notices are
included beside each executable under `resources/tools`, together with the
complete source archive for the pinned upstream commit. The packaged
`resources/tools/manifest.json` records executable/archive/source hashes,
versions and upstream URLs. Build inputs are pinned in `scripts/svt-forks.json`
and `scripts/bundled-tool-sources.json`.

The optional packaged FFmpeg and FFprobe 9.1 pair is built from the official
release source with GPL components enabled. Its delivery includes the complete
FFmpeg and linked codec-library source archives, original license and patent
notices, exact build input hashes, and build recipes under
`resources/tools/ffmpeg`. Linked libraries include x264, x265, libaom, libvpx,
SVT-AV1, Opus, LAME, Vorbis/Ogg, dav1d, zimg, zlib, bzip2, XZ, libass and its
font and text libraries, libplacebo and VMAF. Each component retains its own
copyright, license and any applicable runtime exception. Windows builds also
include corresponding GCC and MinGW runtime sources and notices.
VMAF's default scoring models are compiled from the same retained source tree.
The source and license inventory lists every linked font and text library.

The optional Windows standalone x264 executable is compiled separately from the
same pinned x264 sources. It retains the GNU GPL version 2 or later notice, and
its manifest references the exact shared x264 and compiler/runtime sources and
notices. Its version and build receipt distinguish it from the library in FFmpeg.

The optional Windows mainline SVT-AV1 4.3.0 executable is built from a pinned
upstream source archive. Its complete source, BSD 3-Clause Clear and BSD 2-Clause
licenses and patent notice accompany the binary. The build recipe and pinned
CMake input record are retained, along with the shared compiler/runtime sources.

The standalone `aomenc`, `vpxenc` and `x265` packages retain their original
notices and use source and build-input hashes from the media package records.

The optional Windows portable av1an bundle contains a source-built patched
Av1an 0.6.0, VapourSynth R82 and L-SMASH Works with its matching FFmpeg decoder
fork. Its private CPython 3.15.2 runtime is accompanied by complete CPython source,
the official runtime SBOM and source archives for its listed dependencies.
The package retains complete Cargo dependency sources and license notices,
VapourSynth's zimg, graphengine, libp2p, glslang and Vulkan-Headers sources,
L-SMASH's exact submodules,
the compatibility patch and build recipes. Shared decoder and compiler/runtime
sources reference the same verified media delivery. Each component retains its
original license; notices and exact source hashes are indexed in the manifest.
See `docs/windows-av1an-package.md` and `scripts/package-av1an-windows-lock.json`.

The Windows CPU scorer extension retains vszip 24.0.0, Julek r3 and their complete
vapoursynth-zig, zigimg, JPEG XL, Brotli, Highway and skcms sources and notices.
The pinned Zig 0.17.1 compiler source and build input are also recorded. Scorer DLLs
are compiled locally with only Windows system DLL imports; the scorer delivery
and merged av1an receipt retain their source, build and payload hashes. See
`scripts/package-av1an-scorers-lock.json` for exact component identities.

The Windows QTGMC extension retains its filter and Python module sources,
including the QTGMC-capable `havsfunc` revision, and the source for FFTW 3.3.11.
Its manifest records the upstream wheel hashes for EEDI3, MVTools and fmtconv,
the source for MVTools' statically linked FFTW build, and the locally built
FFT3DFilter and MiscFilters DLLs. See `scripts/qtgmc-windows-lock.json`.

The packaged Windows GPU scorer is Vship 5.1.2's prebuilt x64 Vulkan
plugin under the MIT license. Its binary and complete tagged source archive are
hash-pinned by `scripts/vship-windows-lock.json`. The package retains the archive
and upstream license, and loads the plugin only after a successful per-job GPU
probe. CPU scorers remain available when Vulkan scoring is unavailable.

The Windows package includes MKVToolNix mkvmerge 104.0 under GPL version 2 or
later. The recipe builds mkvmerge from the pinned upstream source archive and
ships its complete MSYS2 runtime DLL closure,
corresponding source packages, MSYS2 build recipes, and original notices under
`resources/tools/mkvmerge`. These include Qt, Boost, libEBML, libMatroska,
compression, text and audio libraries, and the GCC/MinGW runtime sources.
Each component retains its own license and any applicable runtime exception.
Exact package versions, payload hashes, source hashes and dependency checks
are recorded in `scripts/package-mkvmerge-windows-lock.json` and the bundled
build provenance.

The optional Windows subtitle OCR installer pins SeConv 5.4.0, Tesseract 5.5.4
and its English model in `scripts/subtitle-ocr-windows-lock.json`. It installs
these separately from the media package.

The Linux package recipe compiles standalone x264 and mainline SVT-AV1 from the
same pinned sources and retains their notices. Ordinary glibc/compiler shared
libraries are declared system dependencies. Linux native build qualification
and av1an packaging are deferred.
Builds without the optional
tools manifest distribute no media executables. The manifest is the inventory of
what a particular package actually contains.

Image-sequence filename ordering uses ICU4X `icu_collator` and its compiled
collation data, pinned to 2.3.0 alongside the workspace's existing ICU 2.3 locale
infrastructure. These Unicode-3.0-licensed components retain their notice in
`licenses/icu4x-Unicode-3.0.txt`; exact transitive versions are recorded in Cargo.lock.

The optional packaged grav1synth provider is MIT-licensed rust-av/grav1synth at
commit `b3022cffeb24498e6a58f14d4b3114e9976b22a9`, including its embedded
film-stock resources. It statically links private LGPL FFmpeg 9.1 and BSD
dav1d 1.5.5.
Complete pinned sources, locked Cargo vendor with original crate licenses,
readable notices, build inputs and payload hashes accompany the provider.
Windows also retains matching compiler/runtime sources and notices. See
`scripts/grav1synth-lock.json` and `docs/grav1synth-provider.md`.
