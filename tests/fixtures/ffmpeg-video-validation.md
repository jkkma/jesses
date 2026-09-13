# FFmpeg x265 and VP9 qualification — 2026-09-13

Quick Convert and standalone batch offer FFmpeg `libx265` HEVC and `libvpx-vp9`.
x265 uses CRF 0–51 and named presets ultrafast through placebo (default 28/medium).
VP9 uses CRF 0–63, speed 0–5, zero bitrate and good deadline (default 32/2).
CRF 0 is not a separate promised lossless mode.

Both drivers retain the source's 8-bit or 10-bit progressive SDR 4:2:0 format,
square pixels, CFR timing, full/limited range and explicit left/center/top-left
chroma. BT.709, BT.470BG and SMPTE 170M are supported. HDR, unknown color/chroma,
SVT grain/fallback settings and av1an are rejected. Selected audio settings and
framing share the existing planners. Saved settings without these identities keep
their existing meaning.

FFmpeg help must advertise the exact library, pixel format and required options.
The consumer uses `+pix_fmt`, so missing depth support cannot trigger automatic
conversion. Y4M omits some colorimetry; `setparams` reattaches validated tags
without changing samples. Encoded video goes through the supervised pipe into an
owned Matroska handle. Full decoded-frame, track, metadata and source checks still
run before no-overwrite publication. See the primary
[FFmpeg codec documentation](https://ffmpeg.org/ffmpeg-codecs.html).

## Synthetic native gates

Windows FFmpeg/FFprobe 9.0.1 full build: all four `ffmpeg_video_jobs` gates passed
in 26.90 seconds. Eighteen successful encodes cover:

- Both encoders at 8/10-bit and full/limited range; 48 frames at 24000/1001 fps,
  alternate video selection, reordered tracks, exact copied audio/subtitle packet
  hashes, attachment hash, chapters and unchanged source bytes. x265 actually
  produces B-frames and retains their presentation timing.
- Both encoders with center/top-left chroma and BT.470BG/SMPTE 170M tags, checked
  on every decoded output frame by the production validator.
- Both encoders with crop + resize + borders, 10-bit 176×104 output, FLAC audio,
  48 frames, subtitle/attachment retention and decoded black corner Y=64 ±4.
- Two HDR rejections before output publication and two active pipeline
  cancellations, source preservation, bounded shutdown and no partial outputs.

Two focused unit checks cover quality/preset/backend limits and missing
library/depth rejection. Twenty-three browser checks passed, including six new
x265/VP9 cases for defaults, limits, independent drafts, immutable batch/queue
settings, HDR rejection and absence from av1an. `pnpm check` and runtime
all-target Clippy with warnings denied passed. The Linux CI tool build now enables
libvpx and includes this runtime gate; this is configuration, not a Linux run.

## Real media qualification

A read-only 720p episode in the user's Videos folder supplied frames 2880–3359:
480 frames spanning **[120.120, 140.140) seconds** at 24000/1001. Only video and
stereo audio were selected for a separate FFV1/24-bit PCM excerpt; its sources and
outputs stay outside the repository. The original file's SHA-256 and size were
unchanged before/after (740,796,287 bytes).

The public `encode_request` JobManager runner completed x265 CRF 28/medium + FLAC
and VP9 CRF 32/speed 4 + MP3 192 kb/s. Both resized the picture to 960×540 and
added 16-pixel borders, yielding 992×572 8-bit SDR. Independent decoding found:

- Exactly 480 frames, maximum timestamp error 0.500 ms and preserved color tags.
- Exactly 882,930 decoded stereo samples per channel at 44.1 kHz, start zero and
  no final padding. FLAC PCM matches byte-for-byte; MP3 correlation is 0.999278.
- The sampled 8×8 border corner has Y=16 throughout. A representative decoded
  frame was visually inspected and shows intact content with black borders.

Local receipts are in `Videos/Jesses-ffmpeg-video-validation-20260913`: immutable
requests, source manifest/hash, JobManager logs, retained excerpt and outputs,
`real-media-verification.json`, and decoded PNGs. This verifies the specified
20.02-second interval, not a full episode, original subtitle timing, native UI
interaction, HDR, packaged binaries or Linux execution.

```sh
cargo test -p media-runtime --test ffmpeg_video_jobs --locked -- --include-ignored
cargo clippy -p media-runtime --all-targets --locked -- -D warnings
```
