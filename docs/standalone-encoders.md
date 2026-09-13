# Standalone encoder drivers

Quick Convert owns direct encoder execution. av1an has a separate tab and owns
scene detection and parallel chunk execution. The current implementation offers
standalone SVT-AV1 and x264 in Quick Convert and SVT-AV1 chunks in av1an. Encoder
identity is separate from workflow identity, including batch and saved jobs.
x264 uses a timed Matroska intermediate and preserves SDR source depth. AOM,
VPX, and x265 are deferred while SVT-AV1 and x264 workflows are developed.

## Implementation order

SVT-AV1-HDR is the application default, 5fish is the anime option, and mainline
SVT remains selectable. SVT-AV1 and x264 are the active development priorities;
the remaining drivers below are low priority. Shared work should improve these
two workflows first: framing and preview, quality controls, presets, and reliable
execution. Manual crop, width resize, and black borders are available in the
standalone workflow; see the [framing record](../tests/fixtures/framing-validation.md)
and [border record](../tests/fixtures/borders-validation.md).

1. Retain an encoder identity independent of the workflow. Keep existing serialized
   SVT defaults and job history readable, and carry the encoder through batch
   requests, immutable queue snapshots, and generated TypeScript contracts.
2. Extend the output plan's codec, decoded pixel format, HDR policy,
   intermediate format, and encoder-specific quality settings. Current AV1-only
   checks and metadata precision rules must stay strict for existing jobs.
3. When additional encoders become a priority, extract direct drivers for discovery, capability checks, input/output arguments,
   and progress parsing. Reuse the supervised binary pipeline and owned output
   handles. Add `aomenc`, `vpxenc`, and `x265` individually.
4. Prove intermediate timing before exposing each driver. IVF provides a starting
   point for AOM/VPX. HEVC elementary streams require their own timing path;
   substituting the executable in the existing IVF pipeline is insufficient.
5. Expose qualified encoder choices and their settings in Quick Convert, batch
   requests, per-source drafts, and job summaries. av1an remains its own workflow.

## Qualification requirements

- Keep Matroska output and the current selected-track, attachment, chapter, and
  no-overwrite guarantees while adding drivers.
- Preserve explicit selection of exactly one video stream, including alternate
  video tracks in the standalone workflow.
- Retain progressive, constant-rate, square-pixel, 4:2:0 input constraints until
  additional formats have their own implementation and tests.
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
