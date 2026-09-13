# Standalone bitrate and target file size

Qualified on Windows on 2026-09-13. Quick Convert and standalone Batch support
all six video encoders: x264, FFmpeg libx265, FFmpeg libvpx-vp9, mainline SVT-AV1,
SVT-AV1 5fish, and SVT-AV1-HDR. Existing jobs omit `rateControl` and retain CRF.
av1an retains its existing CRF workflow.

## Request and execution

`rateControl` is either `{mode:"bitrate",bitrateKbps,twoPass}` or
`{mode:"targetSize",targetSizeMib}`. Bitrate uses decimal kb/s (1–100000),
target size uses whole MiB (1–1048576). Target size always uses two passes.
No filenames or arbitrary options enter these scalar contracts.

Each pass starts a fresh supervised FFmpeg decoder and a fresh encoder using the
same validated frame selection, trim, tone map, framing, and subtitle rendering.
The first pass writes to a separate owned output handle. Encoder statistics use
fixed ASCII filenames in a create-new private directory; Unicode output paths
and x265's colon-delimited parser never receive interpolated stats paths.
Pass two starts only after successful pass-one exit and nonempty stats. Sources
are checked again between passes. Cancellation kills the owned pipeline and
cleans stats, first-pass output, final video intermediate, and unpublished mux.

x264 uses `--bitrate`, `--pass`, and `--stats`. x265 uses FFmpeg `-b:v` and
`pass`/`stats` x265 parameters. VP9 uses `-b:v`, `-pass`, and `-passlogfile`.
SVT uses VBR `--rc 1 --tbr`, with explicit external `--pass`/`--stats` in two-pass
mode and `--passes 1` for one pass. CRF arguments are absent in bitrate modes.
FFmpeg output options remain after its Y4M input. Y4M input chroma is explicitly
declared from the validated plan, matching output chroma and preserved pixels.

## Target-size budgeting

Before video encoding, the normal selected-media mux runs with a conservative
one-frame FFV1 placeholder. It applies actual per-track conversion, gain, audio
trim, subtitle clipping/conversion, selected attachments, and chapter settings.
Its entire byte length is subtracted from the target. This measures copied audio
and lossless FLAC rather than estimating them from missing or stale bitrates.
Another 1% of the target plus 65536 bytes is reserved for video packet framing,
indexes and container overhead. Remaining bytes are divided by the validated
frame interval duration, using 1000 bits per kb/s. Impossible budgets fail before
either video pass. Selected audio is encoded again during final mux; this keeps
the existing full output validation and original stream mapping intact.

Targets are estimates, not hard file-size limits. Encoder content decisions can
undershoot or exceed the budget, and a final MP4/MOV/WebM conversion may alter
overhead. History reports actual Matroska bytes, requested bytes, and percentage
difference before any final container conversion. The size-measurement log and
first-pass log are retained beside the normal second-pass and mux logs.

## Retained gates

`cargo test -p media-runtime --test rate_control_jobs -- --include-ignored`
passes three native tests:

- Twelve 12-second jobs: six encoders × one/two passes at 300 kb/s. All outputs
  decode to exactly 288 frames and 576000 copied PCM samples. Audio is byte exact.
  Two-pass video plus small mux overhead is within 10% of 300 kb/s. Source bytes
  remain exact and all owned temporary files/directories are removed.
- Six 1 MiB targets with FLAC and frame trim `[24,264)`: exactly 240 frames,
  480000 FLAC samples byte exact to the corresponding source interval. Outputs
  were 970119 / 968170 / 976198 / 971614 / 983808 / 984928 bytes for x264 / x265 /
  VP9 / SVT / 5fish / HDR respectively. A 1 MiB target with 12 seconds of copied
  PCM is rejected because selected media already exceeds the budget.
- Cancellation during each actively producing VP9 pass on a 60-second fixture.
  The gate waits for both producer diagnostics and consumer frame progress, with
  no end marker, before canceling; no output or stats directory remains.

Two unit tests cover omitted old settings, wire round trips, backend/range
rejection, and exact byte-budget arithmetic. Two browser tests cover bitrate and
size validation, per-encoder draft restoration, immutable queued settings,
per-file batch propagation, stale-preview invalidation, and av1an's CRF boundary.
The browser rerun used a command-only 60-second timeout after a cold Vite load
exceeded 30 seconds; assertions were unchanged. Both cases passed in 19.9s.

Receipts: `target/rate-control-gates2-20260913.log` (3 tests, 24.12s),
`target/rate-active-cancel-gates-20260913.log` (strengthened active cancellation,
6.58s), and `target/rate-chroma-video-gates-20260913.log` (all four existing
FFmpeg video qualification tests, 15.47s). Runtime all-target Clippy passed.
The new native gate is wired into Linux CI after SVT fork installation; that
wiring alone is not evidence of a completed remote Linux run.

An independent retained-runner replay passed all three suites with the staged,
source-built FFmpeg/FFprobe/x264 and staged SVT forks, plus explicitly selected
existing mainline SVT. Receipt: `target/source-tools-rate-control-20260913/gate.log`.
The final progress-confirmed cancellation replay, including the active pass log
link in canceled history, passed in 7.47s (`target/rate-active-cancel-gates2-20260913.log`).

## User-media evidence

Retained folder: `Videos/Jesses-rate-control-validation-20260913`.
The source is the previously verified lossless video/audio excerpt of the real
720p episode: original frames `[2880,3360)`, nominal interval `[120.12,140.14)`
seconds, 480 frames at 24000/1001. It is not a full-episode rate-control run.

Both production jobs request 4 MiB, resize to 960×540, and add 16-pixel black
borders, producing 992×572 8-bit limited-range output with left chroma:

| Video/audio | Actual bytes | Difference | Decoded audio                                          |
| ----------- | -----------: | ---------: | ------------------------------------------------------ |
| x265 / FLAC |      4042682 |     -3.61% | 882930 samples/channel at 44.1 kHz, byte exact         |
| VP9 / Opus  |      3611103 |    -13.90% | 961013 samples/channel at 48 kHz, correlation 0.996843 |

Both decode to exactly 480 frames; maximum timestamp error is 0.500 ms, and
sampled border-corner luma is exactly 16. The FLAC/Opus difference deliberately
demonstrates why target sizes remain approximate. Source and excerpt were read
only. The original full-file SHA-256 remains
`FB2EBAC77D66DE606E632A1F937957D23EB256772B3F89D50EE950E1EFDF79B7`.
Requests, pass logs, output files, `real-verification.json`, verifier source, and
representative 10-second PNGs are retained in the folder.
