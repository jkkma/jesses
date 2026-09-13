# Packaging qualification — 2026-09-13

This record distinguishes local native qualification from pending platform and
release gates. No release was published and no artifact was signed by these tests.

## Windows source toolchain

The isolated compiler and source-built FFmpeg/FFprobe pair passed their pinned
payload checks. The media build includes x264, x265, VP9, the supported audio
codecs, libass subtitle rendering, zimg and embedded VMAF models. Actual analysis,
bitrate, loudness, quality, trim, subtitle, mux, video-codec and tone-map gates were
run against the built pair. Separate source-built standalone x264 and mainline
SVT-AV1 also passed encoding checks, including the full staged rate-control tests.

The Windows av1an recipe builds the pinned patched engine, VapourSynth R79,
L-SMASH and its matching decoder fork. It retains 27 original/generated source
archives, seven exact shared static-library/runtime source references, and 542
source/runtime notice files in the base delivery. The complete Python/runtime
inventory contains 51 files. These counts describe the base frameserver before
optional quality plugins are added.

Native checks performed on the final base build:

- Native DLL import closure for the engine, Python, frameserver and decoder.
- Three FFV1 frames decoded by the packaged L-SMASH plugin with external tools
  unavailable; av1an reports the same packaged plugin.
- All seven tool IDs discovered from their verified bundled paths with PATH
  tools and managed user installations unavailable.
- Actual av1an output preserving frame rate, colors, audio and its immutable
  request, followed by cancellation of running workers and output-handle release.
- Those two media tests ran with deliberately invalid external frameserver,
  Python and plugin paths; the owned child selected the packaged runtime.
- Before/after runtime inventories were identical. No bytecode, cache or plugin
  files appeared inside the packaged Python directory.

The final base engine SHA-256 is
`b8e0b102123e806e6e0ddd19a9bb594430ea1f9fc40dd205f2d6f94d89bae6f8`.
Its staged manifest SHA-256 is
`d9bfd887bbfdf213500fe8807b66338a156255a4e383a88885463005e81f5772`.
The retained native runner SHA-256 is
`25dc9f329563821a53c6987bd7abbcf84076f026028c169433c32cc8cfc0b84d`.
The receipt is `target/portable-av1an-final-recipe-native-gate-20260913.json`,
with its sibling log; the stage remains immutable for comparison.

## Windows CPU scorer extension

The subsequent CPU stage adds source-built vszip and Julek with ten complete
source archives, sixty notice files and static native dependencies. Its av1an
engine also carries the retained L-SMASH software-probe fix; short AV1 probe
decoding was independently shown to fail when hardware decoding was requested
and to succeed in software.

The merged runtime evaluates actual SSIMULACRA2, Butteraugli and XPSNR frames
with finite results, verifies every native DLL import, and leaves its complete
53-file Python/frameserver inventory unchanged. All seven native tool paths
resolve inside the stage with external PATH tools and managed installations
unavailable. The actual av1an encode/cancel pair passes again against this
combined stage in 26.43 seconds with conflicting global VS/Python settings.

The patched engine SHA-256 is
`db8a56645fe1ed6f2685542855acf820244b141c866cc056f468908f1c358046`.
The CPU stage manifest SHA-256 is
`f979bcaad4a317a75485cb14678ec550647bedf1a1f1f504dd6bf003df46840b`.
The receipt is `target/portable-av1an-cpu-scorers-native-gate-20260913.json`.
Complete target-score recovery and real-excerpt qualification use this separate
immutable stage; their results must be recorded independently of plugin loading.

Numeric inspection subsequently found that FFmpeg's matrix-tag negotiation
changed the pixels used by av1an VMAF and every-frame XPSNR probes. The retained
48-frame reproduction gave VMAF 48.544283 before correction and 98.975676 after
correction, matching the independent decoded-file comparison. Every XPSNR
per-frame and aggregate stats byte also matches its independent comparison
after clearing only matrix negotiation tags before both metric scale inputs.
Earlier VMAF/every-frame XPSNR target numbers are superseded by this finding;
their lifecycle/source-integrity evidence remains separate. CPU VS metrics use
a different path. The final engine requires `ffmpeg-metric-matrix-v1` for the
affected modes. This required a fresh engine and staged qualification.

The corrected final stage uses engine SHA-256
`2f28430da17604523f539ba87a733459a19f9d16784e2e13bfefc18dda91b7a6`
and manifest SHA-256
`533dac29023d8f469426dd3a9455e3c13f94237c475fec900094485e42241541`.
The real 480-frame VMAF and every-frame XPSNR encodes now pass independent
decode/inspection with normal probe scores around 96–97 and 45–47 respectively.
Both run with external tools unavailable and deliberately conflicting global
VS/Python settings. Source hash, size and mtime and all 53 runtime file hashes
remain unchanged. Corrected-stage encode/cancel tests pass again; their receipt
is `target/portable-av1an-corrected-final-native-gate-20260913.json`.
The strengthened recovery suite passes all nine tests in 569.71 seconds. It
checks finite numerical scores and rejects gross VMAF/XPSNR corruption, alongside
stop/reopen/resume, source/receipt tampering, corrected select/hybrid readers,
framed audio, live pause and cancellation. All 53 runtime files remain unchanged.
Only the configured FFMS2/BestSource reader test is filtered because those
optional readers are external. The receipt is
`target/scorers-qualification-20260913/packaged-recovery-corrected.json`; its
retained runner SHA-256 is
`530f078211f430ee7054b6566fc6d86e114050fe2bc34fc267ee69b4b47d2a7c`.

## Final local Windows artifacts

The unsigned debug NSIS installer and portable ZIP were rebuilt from snapshot
`79b3df4bad170f0425a68f29e1509bb8479f5e87a497b5a36f4c205be531b00e`.
Both post-build application-source comparisons were empty. The native checkpoint
and final packaged executable differ only in PE timestamps and the CodeView build
GUID; the complete comparison is retained in `executable-identity.json`.

| Artifact       |     Bytes | SHA-256                                                            |
| -------------- | --------: | ------------------------------------------------------------------ |
| NSIS installer | 938455050 | `65b0737e842a692966908e8e65dc50c0275a602d172f15b0ac026fcb2f635f1b` |
| Portable ZIP   | 943814225 | `582e237966438c79c8c72d6f6aa558abb4b7aaddc61860cef77ff13286b2214b` |
| ZIP executable |  22859264 | `b64b919b3effcec330f2e967b7cfc312a179285597885b6bfb8860ad1fe53ffd` |

The retained directory is `target/unsigned-windows-final-corrected-20260913`.
ZIP CRC and every archived file hash match the assembled portable directory.
The extracted NSIS payload passes every source/license/runtime hash and discovers
all seven tools natively with external tools unavailable. These results are in
`bundle-resource-qualification.json`, `artifact-manifest.json` and `SHA256SUMS`.
The exact final ZIP executable also passes native UI smoke: all seven tools
resolve to its final portable resource directory, all four av1an compatibility
markers are visible, and config/history/cache/log paths use its own
`jesses-data` directory. The two SVT fork overrides were set for this child only
because managed installs intentionally precede bundled fallback; other tool
overrides were cleared and the parent/user environment was unchanged. No media
jobs were launched during this final smoke and the app closed normally. Native
execution tests for earlier checkpoints remain separately identified.

The final UI evidence is retained in
`Videos/Jesses-final-native-20260913/final-zip-tools-tree.txt`,
`final-zip-tools.png` and `final-zip-storage.png`.

## Invariant tests

All 37 Python package assembly/inventory, locked compiler, Linux source,
standalone encoder staging and portable av1an tests pass. They cover traversal/redirection rejection, changed or
unlisted runtime dependencies, missing corresponding sources, explicit Linux
system-library contracts and rejecting mismatches before native execution.
Rust tests cover the same runtime manifest boundary and child environment
replacement, preserving Windows Unicode and hidden drive-directory state.
A native descendant test verifies that selected encoder/media tools and scoped
environment changes reach the grandchildren without changing the parent.

## Remaining evidence boundaries

Linux media and standalone source archives, licenses, local x264 Git identity and
build-invariant checks passed on this Windows host. The workflow includes actual
Linux compilation, native media tests and extracted AppImage/DEB resource checks.
Those gates have not run here; cross-platform source checks do not establish a
Linux executable or desktop pass. Linux portable av1an remains separate work.

Earlier unsigned Windows installer/ZIP native app smoke evidence applies to
those exact older artifacts. Final source snapshots, hashes and extracted-resource
checks are recorded above; native UI evidence identifies its exact executable.
A clean-machine
installation, offline WebView2 delivery, signing and exact-head hosted CI remain
separate release gates. GPU VShip remains optional.
