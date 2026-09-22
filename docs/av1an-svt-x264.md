# av1an with SVT-AV1 and x264

The av1an workflow uses the selected mainline, 5fish, or HDR SVT-AV1 build, or
the selected x264 executable. It does not substitute another build when the
configured executable has the wrong identity. x264 chunks are raw H.264 and
require mkvmerge to make the timed video intermediate; SVT chunks use IVF with
FFmpeg concatenation by default. The final mux retains selected tracks and
publishes only after complete frame, timing, color, audio, and metadata checks.

The av1an controls include encoder threads (0 asks the encoder to choose),
1–10 chunk attempts, FFmpeg or mkvmerge concatenation where supported, and
explicit output pixel format. SVT accepts 8-bit or 10-bit 4:2:0. x264 accepts
8-bit or 10-bit 4:2:0, 4:2:2, or 4:4:4 when the installed binary advertises
the selected depth and chroma format. x264 always uses mkvmerge because raw
H.264 chunks have no packet timestamps for FFmpeg's concat path. The per-chunk
quality target uses SVT's CRF 1–63 or x264's CRF 0–51 bounds. Saved jobs retain
the original settings and selected tool identities. The advanced catalog has
43 SVT and 31 x264 controls, filtered against the selected executable's help.
It supports signed values, decimals, paired values, and listed tune combinations.
Input/output paths and timing remain owned by the workflow.

Quality targeting requires SDR output; scoring preserved HDR remains
unqualified. Custom encoder paths and file-reading filter expressions are not
accepted as advanced overrides. Scaling and pixel conversion use the verified
FFmpeg preparation path.

Scene detection can divide a sufficiently long L-SMASH source into 2–16
parallel slices. Each slice needs at least 1,200 frames. Jesses validates and
merges the scene lists into av1an's durable receipt path; shorter inputs and
fixed-chunk splitting use ordinary in-run detection. A failed optional prepass
is reported and falls back to ordinary detection. The Segment reader requires
an installed av1an advertising the FFmpeg 9 compatibility fix. The rebuilt,
pinned engine passed a native Segment job; older engines receive an actionable
compatibility error before encoding.

SVT can use a validated inline film-grain table or numeric grain synthesis.
The table is saved as part of the immutable request, staged in the owned
workspace, and checked again on recovery. Optional denoising before a table
uses hqdn3d; it is separate from the encoder's grain controls. Custom pixel
filters are restricted to an allowlist and run once into a verified lossless
prepared source shared by scene detection, chunks, and quality references.
Matroska output can include a JSON attachment of the encode settings. The
generated attachment is hash-checked and source attachments remain subject to
the normal copied-track comparison. Resource estimates are guidance, not a
promise of memory use.

Selecting an advanced synthetic-noise override together with film-grain
synthesis or a table reports a conflict, since SVT otherwise silently chooses
one grain mode. High-bit-depth mode-decision controls identify their lack of
effect on 8-bit output.

Film-stock presets include the supported stock modifiers. Audio codec,
bitrate, and channel choices are remembered for new sources; measured gain and
source-specific selections stay with their original source. The history size
budget accommodates a full 100-job queue containing large inline grain tables.

**Stop and keep progress** saves verified completed chunks. Resume rechecks
the source, tools, processing plan, queue, and chunk bytes before reuse. A
stopped, failed, or interrupted av1an job can instead discard its saved
progress after its worker exits. Discard requires the job-bound workspace
identity and lock, rejects links and replacements, and does not require the
original source or tools to remain available. Neither action overwrites an
existing output.

Validation on 2026-09-22 used read-only source media and separate outputs:

| Check                                                                            | Result                                                                                                                                                     |
| -------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 720p source, 48-frame trim, x264 default 4:2:0                                   | Three chunks published H.264 with AAC; every frame and the output timing were checked.                                                                     |
| Same trim, explicit x264 8-bit 4:4:4                                             | Published H.264 `yuv444p`; FFprobe independently counted 48 frames at 2997/125 fps.                                                                        |
| Same trim, SVT-AV1-HDR                                                           | Published AV1 with AAC after complete decoded-output validation.                                                                                           |
| SVT grain table, custom EQ filter, resize/borders, Opus 7.1, settings attachment | A 240-frame output retained 24 selected font attachments and added one verified settings attachment; output audio timing and padding were checked.         |
| SVT 8-bit output, weighted XPSNR, Opus 5.1                                       | The target search completed and a 48-frame output passed decoded format, timing, and audio checks.                                                         |
| HDR-to-SDR Mobius processing                                                     | A 48-frame 24000/1001 fps PQ excerpt derived from 4K media produced validated 640×376 BT.709 output.                                                       |
| x264 stop, resume, and discard                                                   | Focused 288-frame jobs resumed from verified chunks or removed only their owned saved workspace; source bytes and a neighboring file were preserved.       |
| Segment reader with the rebuilt pinned engine                                    | A 96-frame job published every expected frame at 24000/1001 fps with copied audio and an unchanged source.                                                 |
| Parallel scene detection                                                         | A native two-slice test produced a validated durable scene list covering all 2,400 frames. A separate 2,400-frame real-media excerpt encoded successfully. |

Final complete-job checks combined x264 `film,fastdecode` tuning with
`psy-rd=1.2:0.15`, and SVT `aq-mode=2` with `enable-tf=0`. Both real-media
outputs decoded 24/24 frames with validated AAC and timing.

The 720p source's full SHA-256 and byte count were unchanged after the
real-media runs. Rust workspace tests and the exercised native integration
tests passed. These checks do not qualify every pixel format, metric, plugin, SVT
build, package installation, or long-duration recovery scenario.
