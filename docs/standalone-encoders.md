# Standalone encoder drivers

Quick Convert owns direct encoder execution. av1an has a separate tab and owns
scene detection and parallel chunk execution. The current implementation offers
standalone SVT-AV1 and x264, FFmpeg libx265 HEVC and libvpx VP9, and SVT-AV1
chunks in av1an. Encoder identity is separate from workflow identity, including
batch and saved jobs. x264, x265, and VP9 use timed Matroska intermediates and
preserve SDR source depth. FFmpeg library drivers require the advertised source
pixel format and disable automatic pixel-format conversion. The consumer restores
validated color tags omitted by Y4M without transforming picture samples.

SVT-AV1-HDR remains the default; 5fish and mainline SVT remain selectable. Crop,
resize, borders, and per-track audio are available across these drivers, including
av1an. See the [framing record](../tests/fixtures/framing-validation.md),
[av1an framing/audio record](../tests/fixtures/av1an-framing-audio-validation.md),
and [x265/VP9 record](../tests/fixtures/ffmpeg-video-validation.md).

Mainline SVT-AV1 requires version 2.0.0 or newer for two-pass bitrate and target
size. Earlier versions use a different three-pass VBR protocol and are rejected
before source processing for these modes; CRF and one-pass bitrate remain
available. Linux CI builds the same pinned mainline version as the packages.

## Remaining drivers

Separate `aomenc`, `vpxenc`, and `x265` executable drivers are deferred. Preserved
HDR10 output is currently supported only in the SVT family. Other standalone
drivers accept the explicit HDR/HLG-to-SDR processing path; extending their HDR
output requires static metadata round trips and deliberate handling of dynamic
metadata. All future drivers must keep the shared guarantees below.

## Qualification requirements

- Keep Matroska output and the current selected-track, attachment, chapter, and
  no-overwrite guarantees while adding drivers; other containers pass the
  separate conversion and complete decoded-output checks.
- Preserve explicit selection of exactly one video stream, including alternate
  video tracks in the standalone workflow.
- Retain constant-rate, square-pixel, 4:2:0 input constraints. Uniformly interlaced
  input requires the separately qualified BWDIF plan; frame-rate conversion keeps
  the source timeline and requires its exact rational output count. Other input
  formats need their own implementation and tests.
- Check the installed binary's bit-depth support. HDR must not silently become
  8-bit SDR. Enable each driver's HDR output only after validating static metadata
  round trips and the existing explicit dynamic-HDR fallback policy.
- Keep grain controls specific to the encoder. SVT's synthesis strength and
  arguments do not establish equivalent behavior in other encoders.
- Test B-frame presentation order, fractional cadence, zero start, complete decoded
  frame counts, copied tracks, Unicode paths, cancellation, and source preservation
  through a real encode, mux, and decoded-output check for every driver.

The main seams are `media-core` job/batch contracts, runtime tool discovery,
`jobs/encode.rs`, `jobs/encode_plan.rs`, HDR/metadata validation, intermediate-file
creation, and the shared single-file form. Encoder support must reach these seams
before a selector promises it in the interface.
