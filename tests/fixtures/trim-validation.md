# Frame interval validation

Qualified on Windows on 2026-09-13 with actual FFmpeg/FFprobe and standalone
x264, x265 and VP9. Linux CI runs the same opt-in gate against its pinned FFmpeg
source build; the added CI invocation has not yet run on this uncommitted head.

## Contract

`EncodeSettings.trim` and per-file `BatchEncodeInput.trim` are optional. An
interval uses zero-based `startFrame` and exclusive `endFrameExclusive`. Omitted
fields preserve old full-source jobs and histories. The runtime scans every
original source frame before resolving the interval against its validated frame
rate and exact count. It does not weaken full-source CFR, color, depth or geometry
validation. The decoder selects pictures by frame index, rebases timestamps to
zero, and the output receives the original exact-count and cadence checks against
the selected interval. Crop, resize and borders follow frame selection.

Selected audio requires explicit conversion. The runtime first validates the
complete decoded source timeline, intersects sample presentation times with the
video interval, and uses explicit sample indices in `atrim`. It retains a
positive audio offset when the selected interval starts before the audio does;
it neither inserts silence nor silently drops a source gap. Output validation
checks decoded audible start, sample count and continuity after codec delay. AAC
may retain at most its established final-frame padding. Other supported codecs
keep the established exact/resampling-rounding bounds. Explicit gain follows
sample selection; gain derived from a measurement uses the complete source track.

ASS, SubRip and WebVTT cues intersecting the interval are clipped and rebased,
keeping their original text and styles. ASS timing is quantized to its 10 ms
format precision; SubRip/WebVTT use 1 ms. A selected zero-cue stream is retained
by mapping a direct empty text asset into the final video mux. Chapters are
intersected and rebased. Subtitle text is re-exported and compared after muxing,
and selected font attachments retain their hashes. Assets have owned create-new
paths and are removed on completion, failure or cancellation.

Copy audio, bitmap subtitles and av1an intervals are explicitly unsupported.
ASS animation, karaoke or effects intersected by a boundary are rejected because
clipping a cue changes its relative effect clock. Timed WebVTT inline markup and
unsupported metadata blocks are rejected. A surviving overlap shorter than its
format's timestamp precision is rejected. Empty/reversed or out-of-source frame
intervals cannot publish output. Selected audio with no samples in the interval
must be excluded or the interval changed.

## Actual runtime and browser gates

`cargo test -p media-runtime --test trim_jobs -- --include-ignored` covers:

- x264, x265 and VP9 select frames 12–35 from a 48-frame 24000/1001 source,
  including alternate video selection and reordered streams. All output counts
  are 24. Lossless x264's decoded pictures are byte-exact to an independent
  source trim, and FLAC's 48,048 samples are byte-exact to the original sample
  interval `[24024, 72072)`.
- ASS, SubRip and WebVTT normal overlap and zero-cue output cases retain selected
  tracks, clip chapters and preserve font bytes.
- Copy audio, out-of-range frames and clipped animated ASS fail without output.
- Batch preview, queue and reopened history preserve the interval. Active VP9
  cancellation leaves no published output or partial assets.
- AAC, Opus and MP3 each run two intervals on a source with a 12 ms audio offset.
  Audible start remains 12 ms or rebases to zero as appropriate, sample counts
  satisfy codec padding bounds, FFprobe sample counts match independently
  decoded PCM, and video remains exactly 24 frames.

The first four gates passed together in 13.03 seconds after subtitle pipeline
integration. The additional six codec-delay jobs passed in 10.80 seconds.
Two Playwright cases passed in 10.8 seconds: single-job interval validation and
draft restoration, plus per-file batch settings and preview invalidation.

## Original real-media qualification

A full original 720p episode was read directly by the production job. The source
contains 34,047 frames, two media tracks, ASS subtitles and 24 font attachments.
Its declared 24000/1001 cadence reconciles to the pre-existing supported decimal
rate 2997/125; all source timestamps satisfy that rate before trimming.

The request selects frames `[2880, 3360)`, corresponding to
`[120.12012012012012, 140.14014014014015)` seconds at the validated rate. It encodes
x265 CRF 28/medium, resizes to 960 × 540, adds 16-pixel borders to 992 × 572,
and converts stereo audio to FLAC at 44.1 kHz. Production validation succeeded.
Independent FFprobe/FFmpeg checks confirmed:

- Exactly 480 frames; maximum video timestamp error 0.4995 ms.
- 882,883 decoded samples per channel, byte-exact to original source sample
  interval `[5297298, 6180181)`.
- Five ASS cues clipped/rebased as expected, unchanged cue payloads, all 24 font
  attachment hashes preserved, and 27 output streams.
- The original 740,796,287-byte source remained unchanged, SHA-256
  `FB2EBAC77D66DE606E632A1F937957D23EB256772B3F89D50EE950E1EFDF79B7`.

Private receipts, request, verification script, JSON and output are retained in
the separate `Jesses-trim-validation-20260913` folder under Videos. Native desktop
trim interaction remains separate from the browser and production runtime gates.
