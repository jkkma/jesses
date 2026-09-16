# Standalone encoder drivers

Quick Convert and Batch encode expose encoder identity separately from workflow
identity. Existing saved `x265` and `vp9` jobs keep their FFmpeg `libx265` and
`libvpx-vp9` routes. The direct executable routes have distinct saved values and
labels, so upgrading Jesses never changes which tool an old job runs.

| Choice        | Executable path     | Elementary output | Exact timing step                                                          |
| ------------- | ------------------- | ----------------- | -------------------------------------------------------------------------- |
| AOM AV1       | `aomenc`            | IVF AV1           | encoder header validated against the plan                                  |
| VPX VP9       | `vpxenc`            | IVF VP9           | mkvmerge applies the exact rational cadence                                |
| x265 HEVC     | `x265`              | raw HEVC          | mkvmerge applies the exact rational cadence and B-frame presentation order |
| x265 · FFmpeg | FFmpeg `libx265`    | timed Matroska    | retained legacy route                                                      |
| VP9 · FFmpeg  | FFmpeg `libvpx-vp9` | timed Matroska    | retained legacy route                                                      |

Each direct route receives a bounded Y4M or raw pixel stream from the validated
FFmpeg decoder plan and writes to an owned temporary file. No encoder receives the
destination pathname. The completed elementary stream, timing wrapper, selected
track mux and final container are checked before the app publishes with a
no-replace operation. A pre-existing destination is never overwritten.

The installed executable must advertise every requested input depth, rate-control
mode, pass-statistics switch and lossless setting. x265 boolean switches such as
`--[no-]lossless` are recognized from its full help. The direct AOM, VPX and x265
routes currently preserve validated SDR 4:2:0 input; HDR input requires the
explicit HDR/HLG-to-SDR plan. Dynamic HDR is never silently discarded.

## Rate control and lossless

Constant quality remains the default. Bitrate and target-size jobs start a fresh
decoder and encoder for every pass and retain only complete pass statistics.
Dedicated **Lossless** mode uses the encoder's explicit lossless switch rather
than treating a quality value as a promise. Before publication, Jesses decodes the
entire result and compares its pixel SHA-256 with the exact frames supplied to the
encoder after trim, temporal processing, tone mapping and framing. An encoder that
claims lossless but changes one decoded sample fails with
`LOSSLESS_VALIDATION_FAILED` and publishes no output.

Extended SVT controls are build-gated in the same capability phase. Quarter-step
CRF values through 70 and research presets down to -3 are submitted only when the
selected mainline or HDR build advertises them. The installed 5fish build used for
Windows qualification correctly rejects values it does not advertise.

## NVENC

H.264 NVENC and HEVC NVENC use FFmpeg's NVIDIA encoders. Discovery alone is not
enough: admission performs a bounded one-frame encoder initialization with the
selected depth and lossless mode. Missing hardware, an incompatible driver, or an
unsupported codec/depth combination returns `NVENC_HARDWARE_UNAVAILABLE` with the
driver's final diagnostic before source encoding begins. The qualification host
has an AMD Radeon 880M, so the retained evidence covers this negative gate; a
successful NVIDIA encode still requires qualification on compatible hardware.

## Exact timing

Some `vpxenc` builds coerce a fractional IVF rate even when both the Y4M header and
`--fps` carry the requested rational value. Jesses therefore wraps direct VP9 with
mkvmerge `--default-duration` and scans every decoded timestamp before committing
the timing phase. Direct x265 uses the same wrapper because raw HEVC has no
container clock and may reorder B-frames. The final selected-track output is scanned
again for zero start, exact planned count and complete rational cadence.

## Stop, restart and resume

Standalone recovery resumes whole verified phases, never a partially written
frame. Durable phase receipts cover first-pass statistics, encoded video, the
timing wrapper and final mux validation. The manifest binds the canonical
job/output directory, original source fingerprint, encoder and helper binaries,
settings, resolved filter/timing plan, frame count and artifact identities and
hashes.

**Stop and keep progress** waits for the owned process tree and keeps the newest
complete phase. After relaunch, **Resume** rechecks every binding before reuse and
continues with the next phase. If a crash commits a manifest before job history is
updated, restore may adopt that newer complete manifest only from the exact
job-bound directory after an exclusive lock and strict request/settings/layout
check; execution still performs the full source, tool, plan and artifact checks.
Unknown, replaced or modified workspace entries are preserved and reject cleanup.

**Cancel job** and **Stop queue** also retain completed phase checkpoints for
explicit **Resume**. They remove partial output and transient pass statistics;
canceling before the first checkpoint leaves no saved progress. A retained
checkpoint is bound to the job and must pass the same validation before reuse.

The Windows real-media qualification stopped a two-pass direct x265 job after
`PassOneComplete`, reopened persisted history in a separate process, resumed pass
two, and produced 48 decoded HEVC frames at exactly 2997/125 fps. The recovery
workspace was removed only after successful publication. See the
[feature-extension validation record](feature-extension-validation.md) for the
broader evidence and remaining platform boundaries.

## Qualification requirements

- Preserve explicit selection of exactly one video stream and the requested copied
  or converted audio/subtitle/attachment/chapter set.
- Preserve source depth, dimensions, range, color, chroma placement, sample aspect,
  exact frame count and exact rational timestamps.
- Keep every command supervised and cancellable, and verify each producer and
  consumer exit before accepting its output.
- Test Unicode paths, cancellation, no-overwrite publication, source preservation,
  corrupted recovery artifacts and actual decoded output for every shipped tool
  build.
- Treat Linux execution, NVIDIA-positive NVENC, fresh installation and packaged
  dependency delivery as separate release qualification gates.
