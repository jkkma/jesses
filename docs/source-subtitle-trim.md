# Source subtitle trim consumers

Quick Convert source `QuickConvert.cs:693–701` seeks each original final-mux input
and applies the source duration while converting subtitles. Text burn-in is a
separate reader: `QuickConvertUi.cs:1943–1979` renders the intact original events,
restoring the original picture clock for input-seek mode. Bitmap overlay occurs
before crop/resize and shares the original demux input when the subtitle belongs
to that picture. Source frame, input-seek and decode-from-start modes are retained.

The `sourceTrim` profile now keeps those consumers separate. Subtitle conversion
runs directly on original seeked mux inputs; an independent joint reference
includes the already encoded video and all copied tracks. Every converted packet
hash, size, count, order, duration, PTS/DTS and codec/style header is compared.
Copied packets additionally retain their contiguous original-payload/common-clock
proof. An intact ASS asset renders on the actual selected source-frame clock;
conversion plus burn, and Copy plus burn, do not share a clipped cue asset.
Animated, karaoke and effect cues crossing either boundary therefore remain
renderable without rewriting their internal event timing. Legacy explicit API
clipping/validation remains separate.

Source bitmap rendering overlays on the original clock before trimming/crop.
Input seeking applies to both original inputs; a primary bitmap reuses the same
demux input as its picture. This preserves the fork's initial blank canvas on an
aligned input seek. The historical/image consumer's one-microsecond correction
is unchanged. The native PGS matrix compares every output pixel to independent
source-command rendering and verifies geometry after crop.

ISO MOV_TEXT overlap needs particular care. The independently reproduced source
command can create movie duration1.999s, a3.5s subtitle duration declaration and
subtitle packets at-2.55s and0s ending2.55s. This is the source FFmpeg encoder/edit
list behavior, including its limitation when flattening overlapping cues. The
source profile preserves its actual packet result; it does not claim all original
ASS styles or overlapping readable events survive conversion to MOV_TEXT.
Only after complete independent packet proof is its misleading expected subtitle
duration cleared, while verified movie/video/audio clocks remain authoritative.

A second ISO demux/remux discards a negative DISCARD-marked packet. Source trimmed
MOV_TEXT output now writes the requested final MP4/MOV in the existing final mux,
including requested faststart, metadata/chapter donors, track flags/order and
cadence. An internal receipt binds the owned pathname, container, length and
complete SHA256 before/after all source proofs. Finalization rechecks that byte
receipt, full audio/video decode and every final video timestamp/count, then
publishes the same bytes through existing cancellation/no-clobber/source guards.
The receipt is not an IPC/settings field. Mutation and another owned pathname
are rejected; no extra source UI control is introduced. Generic legacy paths
retain their existing finalization checks.

The final cadence rewrap also preserves a proven fractional mux origin (0.979s
at24fps) rather than rounding it onto a zero-origin frame grid. Full decode uses
passthrough plus the demux timebase so the null output does not manufacture
duplicate DTS; independent strict frame-clock validation remains enabled.

Actual CPU proof uses the retained QTGMC/R79 environment with per-command
`JESSES_FFMPEG`/`JESSES_FFPROBE` overrides to
`target/toolchains/ffmpeg-vulkan-20261101/delivery/{ffmpeg,ffprobe}`. Phase receipts
and logs are under `target/migration-validation/source-av1an-trim-20261101/`:

- `subtitle-formats-final.log`: all three formats, including x264/MOV_TEXT with
  physical faststart box ordering and source-equivalent final packets,1/1 green.
- `converted-packet-negative-final.log`: equal movie/header duration and equal
  codec headers do not hide altered MOV_TEXT packet payload,1/1 green.
- `source-iso-receipt-negative.log`: changed bytes and another owned pathname
  fail the sealed-output check,1/1 green.
- `copied-helper-negative.log`: an actual100ms whole-output shift and an interior
  PCM packet deletion are rejected,1/1 green. Generic Copy verifier ownership
  transferred to the source Copy route owner; see `source-copy-trim.md`.
- `av1an-final-vulkan.log`:9/9 green, including automatic WebVTT/MovText from the
  verified copied cut, original/cut identity, copied ASS/PGS, Saved/New/cancel/
  hard-kill recovery and guarded cleanup. See `source-av1an-copied-cut.md`.
- `subtitle-phase-final.log`: final combined ASS/bitmap/format suite,3/3 green.
- `external-ass-source.log`: source-exposed external ASS burn with frame trim,
  intact animation and exact lossless pixels against an independent original-
  clock render,1/1 green. Signed API offsets were separately checked during
  investigation (`external-ass-offsets.log`); the source UI exposes no offset
  control, so those are not an additional source setting or completion gate.

The source ASS matrix covers3 seek modes × conversion, burn, conversion+burn and
Copy+burn with exact lossless pixels and packet comparisons. The bitmap matrix
covers all3 modes and crop. The VP9 ISO declaration regression and its exact
packet receipts are retained separately; its fix and positive/negative proof
are documented by the mapped-stream owner in `source-mapped-streams.md`.

Rotated AV1AN copied cuts are now qualified in
[the copied-cut note](source-av1an-copied-cut.md#rotated-copied-cuts); source None
split/max-chunk, typed FPS and New queue authority are documented in
[the chunk contract](source-av1an-chunks.md). This subtitle phase does not claim
every codec/container cross product or native Windows execution. The source UI
exposes no signed external offset control; its representative external ASS
burn+trim route is qualified above. Jesses exposes CPU video encoding only.
