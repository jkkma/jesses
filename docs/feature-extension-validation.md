# Feature extension validation

This record covers the September 15, 2026 feature implementation in the Windows
working tree. It is not a release or a Windows/Linux parity declaration. Browser
checks use mocked desktop IPC; actual-tool checks execute the Rust runtime and
installed media tools.

AOM and x265 follow-up work is paused. Their existing changes and receipts are
retained; the latest edits do not have final combined qualification.

The final pre-commit checks pass formatting, generated contracts, strict workspace
Clippy and 261 ordinary Rust tests. The 108 opt-in tests are excluded from that
ordinary count; actual-tool results are described separately below. The latest
complete browser run passed 235 cases, and frontend type checking and build passed.

## Encoder controls and lossless verification

Mainline SVT CRF 64.25 with preset -1 and SVT-HDR CRF 64.5 with preset -3 each
passed a four-frame, 128×72, 10-bit actual-runtime case. An installed 5fish build
without advertised quarter-step support rejected the same fractional request
before encoding. These cases qualify argument/capability behavior, not a broad
quality recommendation for research presets.

An additional pixel comparison exposed a lossless problem in the installed
mainline SVT build: its dedicated lossless mode changed decoded samples at some
presets on that small fixture, despite reporting lossless coding. The runtime now
compares every decoded output pixel against the actual post-filter encoder input
before publication, including after recovery. The integrated check accepts the
fixture with preset 4 and rejects preset 12 with `LOSSLESS_VALIDATION_FAILED`,
leaving the requested destination absent. Its bounded streaming decoder also has
actual-tool checks for changed pixels, truncated frame coverage and cancellation.

The AMD host's actual H.264 NVENC initialization rejects the requested mode with
`NVENC_HARDWARE_UNAVAILABLE` before source encoding and publishes no output.
This verifies the unavailable-hardware diagnostic, not NVIDIA output quality.

## Processing with original media

An original 1080p episode was inspected through its complete 34,047-frame
timeline. A nonzero 120–124 second interval was encoded through av1an/SVT with
12 fps, width 640 and a requested 4:3 display aspect. The completed output passed
an independent full video/audio decode: 48 frames, 640×360 coded pixels, 3:4
sample aspect and all 27 selected streams, including subtitles and font
attachments. The original source's nominal 24000/1001 rate was reconciled to
its measured 2997/125 cadence before resolving the trim.

A fresh closed-GOP excerpt from an original 2160p HDR/Dolby Vision movie passed
explicit HDR10 base-layer tone mapping and 12 fps conversion through av1an/SVT.
Independent inspection found 48 progressive 640×360 10-bit BT.709 frames and
six-channel FLAC. The decoded PCM SHA-256 exactly matched the excerpt's DTS
audio. The short opening logo provides limited visual evidence; this does not
qualify the entire film or Dolby Vision preservation.

Frame-changing av1an processing now creates one verified lossless FFV1 input for
scene detection, chunk encoding and quality references. Complete decoded frame
timestamps, dimensions, sample aspect, pixel format and color are checked before
use. A separate pixel oracle proves that a 24→12 fps conversion after trimming
frames [6,30) retains precisely source frames 6,8,…,28. Recovery binds the original
source, tools, settings and decoded prepared pixels before reusing chunks.

## Utilities, images and inspection

The original 720p episode passed a keyframe-cut qualification retaining all 27
streams. A derived AV1/FLAC/ASS clip passed grain apply, extract, header rewrite
and removal, with complete decoded frame coverage and checks on nonvideo packet
payloads, order and timestamps. PNG, JPEG, a four-frame GIF, a four-PNG sequence
and a lossless FFV1 sequence round trip also passed. Outputs and verification
receipts were saved separately from the originals.

After these checks, all three originals retained their exact SHA-256 hashes,
byte lengths and modification timestamps, including the complete 75.75 GB HDR
source. The checks read the originals and wrote artifacts to a separate directory.

Synthetic actual-tool cases additionally cover concat, CRF ladder measurements,
color metadata and bitmap subtitle OCR. OCR uses an explicitly discovered
Subtitle Edit console/Tesseract installation. The installer recipe and dependency
hashes are documented in [media utilities](media-utilities.md).

Inspector thumbnail qualification checks decoded pixels for a rotated,
anamorphic input; the display preview follows sample aspect and rotation while
the encoder's coded-pixel preview retains its original geometry. Source pinning
and canceled/stale preview handling have browser coverage. See
[source selection and previews](source-selection-and-previews.md).

## Finish actions and saved requests

State-machine tests cover successful queues, failure/stop/cancel disarming,
new-work countdown reset, active inspection reservations, cancellation between
expiry and dispatch, dispatch failure, and rejection of work after dispatch.
The final action shares an admission lock with new jobs and inspections and
revalidates current work before launching the OS command. Browser checks cover
the persistent cancel banner and form synchronization after backend disarming.
No test physically shut down the computer or closed the user's app.

Saved requests are bounded, versioned data. Imported requests start a new job;
durable recovery uses verified history and workspace receipts. Foreign saved
commands are never executed. See [completion and saved requests](completion-actions.md).

The command preview includes the actual FFV1 preprocessing stage and uses its
prepared input for av1an while retaining the original input for the selected-track
mux. Its opt-in actual-tool test checks resolved nonzero time trim, changed frame
rate, no duplicate filtering and cleanup without publishing an output.

## Qualification boundaries

- NVIDIA hardware execution still requires an NVIDIA machine. This Windows
  host has an AMD GPU; negative capability checks do not qualify NVENC output.
- FFMS2, BestSource and QTGMC require their optional external runtime. Their
  successful runtime tests do not imply inclusion in existing packages.
- Managed Vulkan scoring requires the documented matching plugin/runtime setup.
- Native interaction for every new control, Linux actual-tool execution, fresh
  installation, packaged dependency delivery and full release checks remain
  separate from these Windows runtime and browser results.
- Lossless prepared video and recovery checkpoints consume additional disk
  space. Complete source/frame validation may take substantial time before
  encoding begins.
