# Explicit HDR and HLG to SDR

Qualified on Windows on 2026-09-13. Quick Convert and each standalone batch file
can opt into `toneMap: { sourcePeakNits, hdr10BaseLayer }`. Omission preserves old
jobs, drafts and the existing SDR/HDR10-preservation behavior. The signal peak
must be an integer from 100 through 10000 nits. Output is Hable-rendered BT.709,
limited-range 10-bit 4:2:0 SDR referenced to 100 nits. This is an explicit color
conversion; it does not claim to reproduce a studio-authored SDR grade.

## Source and rendering contract

Source video must have explicit BT.2020 primaries, BT.2020 non-constant-luminance
matrix, PQ or HLG transfer, limited range, 10-bit 4:2:0, and left/center/top-left
chroma placement. The original progressive, square-pixel, CFR source restrictions
remain. Every original source frame is still checked, including its color tags,
depth, dimensions, timing and side data. Valid PQ may omit static mastering
metadata when an explicit rendering peak is supplied. Any present mastering and
content-light metadata must still parse and remain consistent across the source.
HDR10-preserving output retains its existing requirement for mastering metadata.

Dynamic HDR requires the separate `hdr10BaseLayer` choice. It uses the existing
qualified Dolby Vision profile 7/compatibility 6 or profile 8/compatibility 1
HDR10 base layer and discards dynamic/enhancement information. Other Dolby
profiles, ambiguous profiles, unknown rendering side data, and HLG with HDR10
base-layer fallback are rejected. av1an tone mapping remains explicitly disabled
pending separate chunk/recovery qualification.

The filter linearizes HDR using zimg and `npl=100`, converts BT.2020 primaries to
BT.709 in floating-point RGB, and applies Hable with the explicit signal peak
divided by 100. HLG uses zimg's 1000-nit reference display transfer. Tonemap's
desaturation calculation receives a BT.709 matrix tag even though the physical
samples are linear RGB; the subsequent zscale explicitly declares RGB input.
This avoids the GBR coefficient behavior caught by the independent neutral-ramp
test. Output conversion uses error-diffusion dithering and explicit 10-bit legal
sample clamps (luma 64–940, chroma 64–960), then removes source frame side data
and applies the SDR tags. Later lossy encoding may ring beyond those sample
limits; it retains the validated limited-range output contract.

Frame selection occurs first, followed by tone mapping, source-space bitmap
graphics, crop/resize, text graphics and black borders. Subtitle/font assets and
audio keep their established validation. Output stream and every output frame
must be SDR; stale mastering, content-light, Dolby Vision or HDR10+ side data
fails the output check. A synthetic installed-tool preflight exercises the full
rendering pipeline before the expensive source scan.

Primary references: [FFmpeg tonemap](https://ffmpeg.org/ffmpeg-filters.html#tonemap)
requires linear floating-point input; [zimg transfer implementation](https://github.com/sekrit-twc/zimg/blob/master/src/zimg/colorspace/gamma.cpp)
defines PQ and reference HLG scaling; [FFmpeg 9.0.1 tonemap implementation](https://github.com/FFmpeg/FFmpeg/blob/n9.0.1/libavfilter/vf_tonemap.c)
defines Hable and the desaturation color-tag dependency.

## Actual gates

`cargo test -p media-runtime --test tone_map_jobs -- --include-ignored` passed all
four gates in 12.85 seconds:

- Six PQ/HLG jobs across standalone x264, FFmpeg x265 and VP9 retain exact
  24-frame intervals, resize/borders, converted FLAC, clipped text subtitles,
  chapter timing and font hashes. Every output frame has SDR color and no HDR
  side data.
- Mainline SVT-AV1, 5fish and SVT-AV1-HDR receive SDR pixels and produce SDR AV1
  without source HDR metadata, retaining the exact count.
- Neutral PQ and HLG patches compare independently calculated transfer/Hable
  output to lossless x264 decoded samples, within three 10-bit code values.
- SDR input, unsupported backend and invalid signal peaks cannot publish output.

The actual subtitle bitmap gate also passed both SDR and PQ-tone-map variants in
2.87 seconds. White PGS graphics remain white and retain their intended placement
through crop/resize/borders, establishing that graphics follow tone mapping.
Twenty-four existing source/plan validation tests passed. Two browser cases
passed in 18.8 seconds: explicit rendering, peak validation, draft restoration,
immutable queued settings, per-file batch choices and preview invalidation.

The Linux CI invocation is added after the three SVT builds are installed; no
Linux result is claimed for this uncommitted head. Native desktop interaction
is qualified separately by the retained desktop build.

## Original movie excerpt

A 75,754,629,962-byte original 2160p movie with Dolby Vision profile 7 and HDR10+
was used read-only. A separately packet-copied four-GOP excerpt retains source
video frames `[0, 96)`, presentation interval `[0, 4.004)` seconds, and its DTS-HD
MA 5.1(side) audio. This is a short opening-logo excerpt, not a full-film test.
An earlier arbitrary end cut was rejected for two incomplete-GOP tail timestamp
gaps; extraction was corrected at the next keyframe's 3.879-second decode boundary.
The runtime timing rules were not widened.

The production request explicitly used the compatible HDR10 base layer, signal
peak 1000 nits, x265 CRF 24/fast, resize to 960 × 540, 16-pixel borders, and FLAC
5.1(side) at 48 kHz. Independent checks confirmed 96 frames, 992 × 572 10-bit
BT.709 limited output, maximum video timestamp error 0.500 ms, and no HDR side
data in any output frame. All 186,368 audio samples per channel decode byte-exact
to the excerpt. Source length, modification time and first/middle/last 1 MiB
SHA-256 samples remained unchanged; this is not a full-file digest claim.

Private requests, logs, scripts, independent JSON, source identity samples and
the output/capture remain under `Videos/Jesses-tone-map-validation-20260913`.

## Nested av1an tool selection

The shared launcher now places selected FFmpeg/FFprobe directories before the
original av1an and inherited PATH directories, while retaining selected encoder
precedence. A concrete Windows descendant test verifies both nested tool names
and SVT route past competing siblings of the original av1an host, and verifies
owned staged-host cleanup. That test passed in 0.37 seconds.

# Native Windows tone-map qualification

Frozen executable `49c84b2425f90efa7842acc5d297fd2dd4c23458550d575a5843e1e744fb6777`
imported the closed-GOP 96-frame original HDR excerpt through the native picker.
Quick Convert used x265 CRF28/medium, explicit 1000-nit peak and compatible HDR10
base layer, 960-pixel width, and FLAC24 with the original 5.1 layout. The job was
queued behind the full-episode encode and completed afterward.

Independent checks confirm 96 HEVC frames at 960x540, limited-range 10-bit BT.709,
no HDR/Dolby Vision metadata on any decoded output frame, at most 1ms timestamp
rounding, and 186,368 audio samples per channel with byte-identical 24-bit PCM.
The source excerpt's identity stayed stable during checking. Output SHA-256 is
`70568bf3c3294e0dbcca6d43dbc2b328ea716ab733e9b6af7f92f3365286e024`.
The short opening-title image is a pipeline check; the separate PQ/HLG arithmetic
fixtures cover tone-curve behavior. Receipts and the extracted output image are
under `Videos/Jesses-complete-native-20260913`, including
`native-hdr-output-independent.json` and `native-hdr-sdr-frame.png`.
The working-source encoder guidance was corrected after this build to explain
tone mapping for native HDR input. Later packages require their own checks.
