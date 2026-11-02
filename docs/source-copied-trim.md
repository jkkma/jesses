# Source copied-track trim qualification

The source UI sets additive `EncodeSettings.sourceTrim` whenever a trim is
configured. The flag selects the fork's copied-track seek rules; it does not
turn trimming on or add a UI control. Omitted/false preserves the historical
explicit API's sample-accurate audio and clipped-text validation. Batch preview,
held command review, persisted jobs and recovery snapshots retain the flag.

## Qualified Direct encoder behavior

Source `Nmkoder/Data/TrimSettings.cs:155–206` and
`UI/Tasks/QuickConvert.cs:693–701` seek each original mux input, while the
encoded picture is already cut. Time modes use the requested milliseconds.
Frame mode seeks half a source frame early and uses a duration half a frame
longer, both truncated to the milliseconds emitted by `GetTimeString`.
Copied compressed audio therefore retains demuxer preroll; it is not promised
sample-accurate cutting. Copied ASS events retain their original payload,
including boundary-crossing animation. They are not rewritten as clipped cues.
FFmpeg's subtitle PTS clamp and shared mux shift remain visible.

The native path prepares an independent copied-track seek reference and then
uses source-equivalent seek arguments in the real final mux. Bounded streaming
checks compare the complete reference packet selection, original packet SHA-256,
size, order, duration, PTS and DTS under one common clock. The encoded video is
independently compared against its intermediate with that clock, and every
final video frame is decoded and validated. Mixed converted audio retains its
separate full decoded sample/timing proof, shifted by the common mux clock.
FFprobe's AAC header start may include CodecDelay; after the complete packet
proof, packet starts are authoritative for synchronization metadata checks.

`crates/media-runtime/tests/source_trim_jobs.rs` passed **5/5** real Linux gates
with the qualified FFmpeg 9 provider. The green run is
`target/migration-validation/source-trim-20261101/native-final.log` (12.76s).
It covers both time strategies and frame mode, reordered tracks, AAC preroll,
animated ASS, external copied audio at ±250ms alongside converted FLAC, positive
MKV and negative MPEG-TS source origins, held unchanged/edited programs and an
actual immutable batch job. The three-mode gate compares complete output
packet timelines to independently generated source-equivalent FFmpeg commands.
The historical explicit API trim suite also passed **5/5** (24.19s), in
`legacy-trim.log`. The previously pending browser copied-audio trim test now
passes with `sourceTrim:true` asserted (`browser.log`); Svelte and generated
contract checks pass. The earlier 252-test full UI run remains historical until
its next complete rerun.

## AAC transport framing correction

Negative-origin AAC in MPEG-TS exposed a canonical-source verifier mismatch:
ADTS headers were hashed as if they were AAC access-unit payload. Matroska stores
ASC plus raw AAC access units, so correct FFmpeg output failed the old raw packet
hash check. Canonical preparation now explicitly applies `aac_adtstoasc` for
AAC transport sources. A bounded FFmpeg framehash stream proves every resulting
access-unit hash, size, count, order and timestamp against the canonical file.
An independent full decoded PCM SHA-256, byte/sample count, sample rate, channel
layout and decoded timeline prove that the framing change did not alter audio.
All source identity, metadata, owned workspace and no-clobber checks remain.
Positive and negative MPEG-TS AAC origins are included in the five real gates.

## Remaining distinctions

- The user subsequently requested CPU-only encoding, so NVENC is being removed
  from the current product. The completed NVENC source-command/packet-clock
  research is historical evidence, not an active migration requirement.
  Its software-only receipts and logs remain under this phase's validation
  directory; its active trim test is archived in
  `docs/historical-tests/source-nvenc-trim.rs.txt`.
- Source AV1AN now first stream-copies the whole requested section into a
  same-container intermediate, retaining source keyframe preroll and B-frame tail
  clocks. Actual cut, final copied streams, Saved/New/cancel/hard-kill recovery and
  guarded cleanup pass8/8 CPU gates. See [copied-input qualification](source-av1an-copied-cut.md)
  for exact source lines, commands, lifecycle evidence and unqualified combinations.
- Source UI Copy+trim now accepts checked nonfirst/external videos and donors,
  preserves their mapping/order/offsets, and executes one original-input FFmpeg
  seek operation. Six actual native gates and two selected-source/stream
  browser cases pass; see [Copy trim qualification](source-copy-trim.md).
- Text subtitle conversion/burn with trim still uses the clipped-text consumer.
  Boundary-crossing animated/karaoke/effect ASS is rejected by
  `trim_subtitles.rs`; source-profile execution remains to be adapted and
  qualified. Existing legacy tests cover simple text conversion, attached-font
  burn and bitmap burn with positive/negative offsets and trim. Copied PGS is
  now passes real Direct and AV1AN copied-PGS gates against source-equivalent
  packet/timing references.
- Direct/AV1AN MOV_TEXT Copy without mapped data now selects a compatible
  MOV/MP4 private stage. Ten actual outputs cover Copy/Direct/AV1AN, PCM,
  AV1 and subtitle conversion; see [mapped-stream qualification](source-mapped-streams.md).
- This phase does not claim all trim combinations or native Windows/macOS
  execution.
