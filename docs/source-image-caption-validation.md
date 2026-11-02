# Fixed-image captions: native validation

Resumed qualification on 2026-11-01 after reboot cleared the previous `/tmp`
toolchain. The initial image gates use the installed Linux FFmpeg/FFprobe 6.1.1
pair and x264 0.164.3108. The pinned production FFmpeg/FFprobe 9.0.1 pair has
since been rebuilt and passes the full sequence suite. A native R79/QTGMC
provider now passes all eight presets and all four caption-export paths. This
does not establish Windows/macOS qualification. Tool hashes and durable logs are under
`target/migration-validation/image-native-20261101/`; the production delivery is
`target/toolchains/ffmpeg-full-20261101/delivery`.

## Implemented corrections

- Image timestamp scans, reference-pixel scans and output-frame verification
  retain `JOB_CANCELED` when the owned streaming tool is canceled. Other image
  failures keep their previous error class. A real FFmpeg scan produces its
  first PNG marker, remains active, and is canceled before completing; its
  returned code is asserted. The complete image-export mutation/cancel test now
  asserts that code as well as source preservation and no publication.
- Sequence source guards use the existing `files::WorkspaceLock`. On Unix,
  dropping the unique owner explicitly unlocks its open file description;
  closing a descriptor alone can leave a concurrent fork holding that same
  lock until exec. The Linux regression forks a child that retains the lease
  without executing another program. The live source still blocks deletion;
  after its owner is dropped, guarded cache release succeeds while the child
  remains alive. No retry, sleep, or deletion-guard relaxation is used.
- Full-range JPEG materialization explicitly uses
  `scale=in_range=pc:out_range=pc` when converting deprecated `yuvj*` layouts to
  FFV1's modern planar layout. FFmpeg 6.1 otherwise rescales samples to limited
  range before tagging the cache as full range. The original complete frame
  hashes remain the acceptance criterion. A new real-media test compares all
  three decoded frames in each of JPEG 4:2:0, 4:2:2 and 4:4:4, then checks the
  original file bytes and guarded cache release.

## Positive media evidence

`image_subtitle_jobs` has six passing gates against the restored provider
(`qualified-image-captions.log`, 9.24 seconds):

- Six PNG/JPEG/GIF renders use primary and external text captions, external
  offset, frame trim, crop/resize/borders and attached font extraction. Paths
  include quotes, spaces and filter punctuation. Both sources remain unchanged.
- Eight PGS renders cover primary and external bitmaps, external offsets of
  zero and ±250 ms, and zero or +5-second picture origins. A generated white
  source rectangle is cropped and resized before borders; every retained
  frame checks the independently calculated half-open cue interval, rectangle
  coordinates and pixel count. The fixture mux preserves its PGS timestamps
  explicitly, so expected times do not depend on input-demuxer rebasing.
- Twelve VFR text renders cover primary/external sources, zero/+5-second
  origins and full/frame/time intervals. The decoded source timestamps are
  asserted as `[0,.25,.5,.75,1,1.5,2,2.5]` plus the origin. Trims start at the
  actual 1.5-second retained frame, which differs from the average-FPS estimate;
  expected caption visibility is checked for every output image.
- Resolution-to-publication collision preserves the later owner's bytes;
  dotted original folder names survive managed cache indirection.
- Picture/subtitle mutation and active cancellation publish nothing, remove
  owned temporary caption/image assets and preserve original inputs.

The prior `image_jobs` suite passes all eight gates, including the full 10,001
PNG export, depth/alpha, source order, rational rates, GIF palette limits,
JPEG quality/chroma layouts, shared trim/geometry/FPS/tone/filter processing,
both FFmpeg deinterlacers and no-clobber/cancel cleanup.

## Remaining provider qualification

### Actual QTGMC captions: passed

The runnable gate is
`qtgmc_fixed_images_render_text_and_bitmap_before_verified_intermediate_export`
in `crates/media-runtime/tests/image_subtitle_jobs.rs`. It exercises external
text and PGS captions in both single-frame and bob modes with a source-frame
trim, verifies every exported image's cue visibility/frame count, and checks
source bytes and owned-asset cleanup. All four paths now pass against the real
native provider at `target/toolchains/qtgmc-linux-20261101`; no stub, missing-tool
skip or user deferral establishes this result. Its `qualification.json` also
records actual execution of all eight QTGMC presets and eight L-SMASH FFV1
reader frames. `build-provenance.json` retains the source/plugin/runtime hashes;
`env.sh` selects its private configuration, interpreter and qualified media pair.

The checked-in pins and recipes are:

- `scripts/package-av1an-windows-lock.json`: VapourSynth R79 at
  `acabf605b2205b32d65859bb2736405719d2fafd`, Cython 3.3.0, zimg and graphengine;
  L-SMASH Works at `7e63c8400a252f4dbbeea80cc3bfec895abf12cf` and its private
  FFmpeg at `39c24f36247f370864ce86ebdf2aa936151f4bfc`.
- `scripts/qtgmc-windows-lock.json`: havsfunc
  `5baf9c2a1dac9da4de182cf3a4e12041c3e0d48b`, vsutil 0.8.0, MVTools v24,
  znedi3 r3.3, fmtconv r30, RemoveGrain R1, Misc R2, FFT3DFilter R2,
  EEDI3 r8, TemporalSoften2 v1 and FFTW 3.3.5. The existing Windows delivery
  includes all these dependencies. Fast-mode runtime probing specifically
  exercises havsfunc, MVTools, Misc, znedi3, RGVS and fmtconv, and actual source
  rendering additionally uses `core.lsmas.LWLibavSource`.
- `scripts/build-package-av1an-qtgmc.py` and
  `scripts/package-av1an-qtgmc.py` build/package that **Windows** extension.
  They are Windows recipes. The new native counterpart is
  `scripts/build-qtgmc-linux.py`, with `qtgmc-linux-lock.json` pinning znedi3's
  source gitlinks; see `linux-qtgmc-provider.md` for reproduction.
- `scripts/build-vapoursynth-linux-scorers.py` and
  `docs/linux-vapoursynth-scorers.md` rebuild the pinned Linux L-SMASH/scorers
  closure, but require an existing native R79 SDK/runtime and full FFmpeg build;
  they do not construct the Linux QTGMC plugin set. The new native QTGMC recipe
  builds that full closure, including R79 and its exact SDK. The Python 3.14 executable
  at `~/.local/bin/python3.14` survives; the default interpreter is
  3.12. Its presence alone does not provide VSPipe or native plugins.

With the restored provider selected, reproduce with:

```sh
. target/toolchains/qtgmc-linux-20261101/env.sh
cargo test -p media-runtime --test image_subtitle_jobs qtgmc_fixed_images \
  --locked -- --ignored --nocapture
```

### Qualified FFmpeg sequence/encode regression: passed

The actual sequence mutation/cancel and stop/restart/recovery gates pass on the
installed tools. Full-range JPEG import now also passes its complete decoded
sample comparison. However, the larger
`nonsequential_png_alpha_and_jpeg_names_keep_order_fps_and_normal_previews`
gate initially reached a later standalone x264 trim and failed its existing metadata
validator on FFmpeg 6.1.1: the decoded output reports `yuvj444p`, full range and
chroma location `left`, while the validated plan expects `yuv444p`, full range
and `center`. No destination was published. The validator was preserved.

The qualified production pair was restored using
`scripts/build-package-ffmpeg-linux.py` and
`scripts/package-ffmpeg-linux-lock.json` (FFmpeg 9.0.1), following
`docs/linux-ffmpeg-package.md`. With `JESSES_FFMPEG`/`JESSES_FFPROBE` and the
fixture helper's `PATH` bound to that same pair, all **four sequence gates pass
in 15.40 seconds**, retaining the same system x264 and unchanged validator.
The log is `qualified-image-sequence.log`. This includes high-depth/alpha and
JPEG import, original names/metadata/previews, the JPEG layout hash gate,
mutation/cancellation, and actual stop/restart/recovery/cache protection. The
old 6.1 metadata report is retained as provider compatibility evidence; it is
not an unresolved failure on the qualified production pair.

## Reproduction

```sh
cargo test -p media-runtime --test image_subtitle_jobs --locked -- \
  --ignored --nocapture --skip qtgmc_fixed_images
cargo test -p media-runtime --test image_jobs --locked -- \
  --ignored --nocapture --test-threads=1
cargo test -p media-runtime --test image_sequence_source \
  jpeg_full_range_samples_survive_every_supported_chroma_layout \
  --locked -- --ignored --nocapture
cargo test -p media-runtime --test image_sequence_source --locked -- \
  --ignored --nocapture --test-threads=1
cargo test -p media-runtime --lib --locked -- --include-ignored --nocapture \
  jobs::images::tests jobs::sequence::tests output_resolution::tests
```

The complete sequence command requires the restored pair in both discovery
variables and `PATH`; the default system pair retains the documented metadata
failure. The real QTGMC provider and its explicit caption gate now supply the
additional qualification rather than relying on the other positive image gates.
