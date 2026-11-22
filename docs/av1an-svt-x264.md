# av1an encoder routes

The av1an workflow uses the selected mainline, 5fish, or HDR SVT-AV1 build,
x264, aomenc, vpxenc for VP9, or x265. It does not substitute another build
when the configured executable has the wrong identity. x264 and x265 write raw
chunks and require mkvmerge to make a timed video intermediate. vpxenc also
uses mkvmerge to assign exact rational timing to its VP9 chunks. SVT and AOM
use IVF with FFmpeg concatenation by default. Before publication, Jesses checks
the planned streams and uses the movie track's declared frame count when
available. Otherwise it uses the final muxer's packet count or counts the movie
track's packets directly. AV1 and VP9 can store hidden frames in different
packets, so a packet count by itself can leave the exact frame count unconfirmed.

Normal AV1AN startup uses stream headers and a bounded first-frame inspection,
without a separate whole-video packet or decoded-frame scan. Header frame counts
are provisional: the engine's validated scene/chunk queue supplies the actual
count for recovery, progress and final output checks. Exact-boundary operations
(including trims, temporal processing and MP4 presentation edits) keep the
planning they need. Missing duration/count metadata also falls back to packet
planning.

Resumable AV1AN jobs bind ordinary sources to their file identity, size,
modification and change times, and SHA-256 samples. Completed chunks and copied
cuts retain full SHA-256 receipts. Jesses also keeps full hashes for older
receipts and uses them when the file change time is unavailable or too recent.
The actual encode reports unsupported options or missing runtime dependencies.

The av1an controls include encoder threads (0 asks the encoder to choose),
1–10 chunk attempts, FFmpeg or mkvmerge concatenation where supported, and
explicit output pixel format. SVT accepts 8-bit or 10-bit 4:2:0; AOM and VP9
also accept 4:4:4, and AOM additionally accepts 4:2:2. x264 and x265 accept
4:2:2 or 4:4:4 where the installed binary advertises the selected depth and
chroma format. The per-chunk quality target uses SVT's
CRF 1–63, AOM/VP9's quantizer 0–63, or x264/x265's CRF 0–51 bounds. Saved jobs
retain the original settings and selected tool identities. The advanced
catalog is filtered against the selected executable's help.
It supports signed values, decimals, paired values, and listed tune combinations.
Input/output paths and timing remain owned by the workflow. Preserved HDR10
with tagged, limited-range 10-bit BT.2020/PQ input is supported by x265 and
AOM. x265 emits mastering and content-light SEI; AOM's static HDR metadata
is attached to Matroska using mkvmerge, so that tool is required for the AOM
HDR route. Other HDR sources still require an explicit SDR tone map.

Quality targeting requires SDR output; scoring preserved HDR remains
unqualified. Custom encoder paths and file-reading filter expressions are not
accepted as advanced overrides. Scaling and pixel conversion use the verified
FFmpeg preparation path.

Scene detection can divide a sufficiently long L-SMASH source into 2–16
parallel slices before encoding. Each slice needs at least 1,200 frames. When
planning uses the file headers, the slices tile the frame count of the L-SMASH index
that the prepass builds and av1an then reuses; a counted source must match that
index exactly. Jesses validates and merges the scene lists into av1an's durable
receipt path; shorter inputs, other readers, resumed jobs and fixed-chunk
splitting let av1an index and detect scenes during its own launch. A failed
optional prepass is reported and falls back to that. The Segment reader requires
an installed av1an advertising the FFmpeg 9 compatibility fix. The rebuilt,
pinned engine passed a native Segment job; older engines receive an actionable
compatibility error before encoding.

SVT and AOM can use a validated inline film-grain table or numeric grain synthesis.
SVT uses one grain model for the whole video, even in a standalone encode. AOM
honors table timestamps in a whole-input encode, but each AV1AN chunk starts
at zero. For SVT or parallel AOM, use one table segment starting at zero and
covering the complete video. For timed AOM tables, use standalone encoding or
explicit fixed AV1AN splitting with maximum chunk frames set to zero.
When the frame count or rate isn't known until encoding, Jesses checks coverage
against the reconciled video before publishing the output.
The built-in film-stock presets generate one grain model for the full duration.
The table is saved as part of the immutable request. AV1AN stages it in the owned
workspace and checks it again on recovery; standalone encodes stage a guarded
temporary table for the encoder's lifetime. Optional denoising before an SVT table
uses hqdn3d in AV1AN; AOM uses its own denoise-noise-level option when selected. Custom pixel
filters are restricted to an allowlist and run once into a verified lossless
prepared source shared by scene detection, chunks, and quality references.
Matroska output can include a JSON attachment of the encode settings. The
generated attachment is hash-checked and source attachments remain subject to
the normal copied-track comparison. Resource estimates are guidance, not a
promise of memory use.

Selecting an advanced synthetic-noise override together with film-grain
synthesis or a table reports a conflict, since SVT otherwise silently chooses
one grain mode. High-bit-depth mode decision (`hbd-mds` 1 or 2) needs a 10-bit
picture, so those jobs always encode at 10 bits: an 8-bit source is converted
to 10-bit before encoding, and an 8-bit output format is raised to 10-bit.

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

Additional native checks on 2026-09-26 used generated 16-frame 24000/1001 fps
sources. AV1AN AOM, VP9, and x265 jobs each retained all frames and selected
source tracks. Standalone and AV1AN AOM 4:2:2/4:4:4, and VP9 4:4:4, produced
the requested 10-bit decoded formats. Standalone and AV1AN AOM/x265 preserved
tagged HDR10 color plus readable mastering and content-light metadata on the
first decoded frame. Standalone AOM numeric grain and AOM/SVT table encodes
produced readable AV1 film-grain headers. Each check kept source bytes intact.
