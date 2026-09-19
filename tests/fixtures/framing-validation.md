# Crop and resize validation

The subsequent [black border milestone](borders-validation.md) extends this
historical crop/resize checkpoint. SVT-AV1 and x264 are now the active encoder
priorities; other video encoders are deferred.

Windows x64 development qualification on 2026-09-12 covers manual crop and
aspect-preserving width resize in Quick Convert and standalone Batch encode.
SVT-AV1-HDR is the default (CRF 30, preset 2, film grain retention tune, dynamic
HDR fallback off), 5fish is the anime option, and mainline SVT remains available.
Existing x264 functionality shares the framing stage. Further encoder development
is lower priority than the SVT workflows.

## Behavior

- Four nonnegative even crop edges, followed by optional Lanczos width resize.
- Cropped and output dimensions are even and between 64 and 8192 pixels. Height
  is calculated from the cropped aspect ratio and rounded to the nearest even
  pixel, rounding halfway values upward. An explicit larger width upscales.
- Output pixels stay square. The UI shows source, cropped, and output sizes;
  it rejects invalid edits without submitting a job.
- Each source/build/workflow keeps its draft. Batch framing is per file;
  changing it invalidates the reviewed preview. Untouched framing preserves
  the existing per-file reporting of unsupported sources in mixed batches.
- Queue snapshots and saved history retain framing. Missing framing in older
  history means zero crop and no resize. Existing encoder identities remain
  readable without being changed to the new UI default.
- Source frames are checked at original dimensions; encoded stream metadata
  and every decoded output frame must match the planned dimensions. Timing,
  frame count, depth, range, chroma, color, HDR and copied-track checks remain
  active before publication. No framing leaves the decoder filter path unchanged.

Cropping occurs before resizing. Range, matrix and supported chroma positions
are passed explicitly to the scaler; square-pixel output is explicit. The filter
chain is assembled from validated numbers. See the primary
[FFmpeg filter documentation](https://ffmpeg.org/ffmpeg-filters.html).

## Automated checks

- 145 frontend tests passed, including request settings, invalid values,
  dimensions, draft restoration, encoder defaults, mixed batches, preview
  invalidation and immutable history summaries.
- The ordinary Rust workspace suite passed on the pinned Rust 1.98.1 MSVC
  toolchain: 7 core, 123 runtime library and 1 additional integration test.
  Opt-in native tests remain explicitly ignored in that ordinary run.
- All four opt-in `framing_jobs` gates passed on MSVC. They exercise actual
  5fish and HDR encoding of 8/10-bit SDR, HDR10 static metadata, per-file batch
  dimensions/history, and cancellation cleanup. An 8-bit lossless x264 case
  independently compares every output frame's pixel hash with a separately
  filtered reference, checking crop offsets and filter order.
- Clippy with warnings denied, Rust formatting, generated contracts, Svelte
  checking, and the embedded-frontend Windows desktop build passed.
- The Linux CI lane at this checkpoint included the framing gate after installation
  of the pinned SVT forks. Linux native qualification is now deferred. The results
  below were recorded locally.

## Real media

Inputs from the user's Videos folder were opened read-only. Each result passed
the runtime's full source/output checks and a separate FFprobe/FFmpeg verifier.
The source and output timestamps, selected stream order, dispositions, track
tags, copied audio/subtitle packet fingerprints, attachments and chapters were
checked independently. Converted audio was also checked with decoded sample
counts and waveform alignment.

| Job                                                                   | Framing result                                              | Decoded frames | Tracks | Output bytes |
| --------------------------------------------------------------------- | ----------------------------------------------------------- | -------------: | -----: | -----------: |
| Complete 720p episode, 5fish CRF 30/preset 8, biases 5/4              | Crop 8 top/bottom; 1280 × 720 → 960 × 528                   |         34,047 |     27 |  181,722,007 |
| 1080p anime excerpt, 5fish CRF 30/preset 8, biases 5/4, Opus 128 kb/s | Crop 12 top/bottom, 16 left/right; 1920 × 1080 → 1280 × 716 |            236 |     27 |   11,162,975 |
| 4K HDR movie excerpt, HDR CRF 30/preset 8, grain-retention tune       | Crop 8 top/bottom, 16 left/right; 3840 × 2160 → 1920 × 1082 |            114 |      5 |    3,299,507 |

The complete episode's decoded timestamps matched exactly. The short excerpts
differed by at most 1 ms because of container timestamp rounding. The movie job
explicitly enabled the existing HDR10 fallback for its Dolby Vision/HDR10+
source; it retained validated static HDR10. This is a test setting, not the
application default. An earlier excerpt with a timestamp discontinuity was
correctly rejected before encoding; the supported closed-GOP excerpt above
completed without relaxing validation.

The four source files checked retained their SHA-256 hashes, sizes and UTC
modification times. Completed jobs left no partial outputs or media processes.
Evidence and resulting videos are retained in a local qualification directory,
including request JSON, job logs, independent reports, tool versions, and
source-integrity records.

## Qualification limits

The production desktop executable builds successfully. A native Windows pass
imported the actual anime excerpt through the file picker, verified HDR defaults,
set a 12-pixel top crop and 1280-pixel width, and started the encode through the
UI. The native job succeeded at 1280 × 712 with all 236 frames and 27 tracks;
its saved request and independently checked output are in `native-job.json`,
`native-request.json`, and `native-independent.json`. HDR tune remained film
grain, CRF 30/preset 2, and dynamic-HDR fallback remained off. The app closed
normally after completion.

An initial input-desktop access failure was resolved before the successful
native pass. This pass does not establish clean-machine/package qualification.

av1an rejected nondefault framing at this checkpoint. Subsequent
[border qualification](borders-validation.md) and
[av1an framing/audio qualification](av1an-framing-audio-validation.md)
record those extensions. Automatic crop, trim, frame-rate conversion, arbitrary output aspect ratios,
additional resize modes and non-square-pixel inputs remain pending. Strict even
crop input, fixed Lanczos and explicit-width upscaling are deliberate choices
for this bounded implementation.
