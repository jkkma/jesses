# Source AV1AN copied-input trim

The source New Settings queue-preservation audit now has a separate phase in
[source-av1an-chunks.md](source-av1an-chunks.md). The earlier New/cancel/restart
fixtures below use the historical explicit API rebuild policy
(`sourceWorkflow=false`); they do not qualify the fork's saved-queue New behavior.

The source adapter's existing `sourceTrim` flag selects this behavior only for
an AV1AN request with a configured trim. It adds no control. The original public
request and settings remain frozen; the effective internal request remaps copied
stream indices and removes the trim so it cannot be applied a second time.

Nmkoder `UI/Tasks/Av1an.cs:664` calls `CutTrimmedInput` before wiring the resulting
`inPath` into `-i` at719 and before the Shift+Run editor at734–736. Replayed saved
arguments bypass preparation and retain their saved input. `UI/Tasks/UtilCut.cs:115–143`
uses this media command, preserving the original container extension:

```
ffmpeg -ss START -i ORIGINAL -t DURATION -map 0 -c copy \
  -avoid_negative_ts make_zero -ignore_unknown CUT
```

The copied start is the requested start, not a pre-snapped keyframe. Duration is
the requested end minus start; the end clamps to the source format duration.
Frame positions use the source rate and millisecond ties-to-even rounding.
FFmpeg retains demuxer/keyframe preroll. A request for frames24..72 of the fixture
therefore feeds72 copied frames to AV1AN, rather than an exact48-frame cut.
Only guarded stream-copy preparation runs before command review; custom filters
remain in the later encoder pipeline.

The receipt binds the immutable original's identity and full SHA256, exact copy
arguments/tool bytes/section, owned files, complete cut identity/hash, stream
mapping and packet clocks. Independent bounded scans compare every emitted
packet's payload, size, order, duration and shared shifted PTS/DTS to a contiguous
original sequence, including ASS/PGS subtitle clamp behavior. Known streams cannot
be omitted or reordered. Metadata, attachments and requested-clock chapter cuts
are verified, followed by complete audio/video decode. The final mux separately
verifies every copied primary audio/subtitle/data packet, plus decoded copied
audio sample clocks; transport AAC uses explicit access-unit and decoded-PCM proof.
Original naming, user-visible identity and explicit metadata donor paths remain
authoritative.

A B-frame cut can contain a tail timestamp gap: the qualified copy contains74
frames starting at0.083s, with final decoded PTS3.166s rather than ordinal73's
nominal3.125s. This is the source command's copy boundary. Only a verified cut
receipt enables the internal ordinal-reader policy. It still requires a complete
decode, finite strictly increasing timestamps and stable properties on every
frame. AV1AN must emit all74 ordinal frames at the configured rate and the final
encoded clock/count remain strictly validated. No client flag bypasses the legacy
or Direct CFR checks.

Each trim has a separate `.jesses-ID.source-cut-DIGEST` variant. Saved Settings
rechecks original/cut hashes and reuses the same cut bytes/mtime and verified
chunks. New Settings authorizes replacement only for the full exact settings
digest; the previous variant remains immutable until the new recipe is committed.
An unconfirmed New transaction journals only recipe/source/tool authority, not
chunk receipts. Canceling or restarting after a hard kill restores that committed
recipe and preserves only chunk files actually remaining after the authorized
reset. It never resurrects deleted chunks. Fresh or changed-trim preparation
cancellation kills the owned process, removes its new stage and preserves the old
variant. Success and guarded Delete/Clear remove owned variants. Foreign files
prevent destructive cleanup before either workspace is modified.

Actual CPU qualification is in
`crates/media-runtime/tests/source_av1an_trim_jobs.rs`:

- `target/migration-validation/source-av1an-trim-20261101/native-matrix.log`:
  **8/8 passed**,63.94s, covering exact independent source-copy commands,
  chapter/title/attachment subset, final copied AAC/ASS, B frames with a+5s origin,
  copied PGS, Saved reuse, committed New, canceled New, hard-killed pending review,
  changed original/cut/receipt refusal and foreign-file guarded deletion.
- `preparation-cancel-final.log`: **1/1 passed**,5.57s, expanded to both fresh and
  changed-trim New preparations, with real paced FFmpeg PID termination and old
  variant preservation. JSON receipts retain cancellation latency/PID/error.
- `ordinal-unit.log`: **1/1 passed**, rejecting duplicate/backward/nonfinite source
  clocks, changed frame geometry and a non-CFR final output.
- `direct-pgs-final.log`: **1/1 passed**,3.79s, additionally proving Direct copied
  PGS packets/timing against the independent source final-mux command.

These are actual Linux CPU executions using the retained FFmpeg9 provider, pinned
source SVT-HDR and AV1AN/R79. Windows/macOS execution,
all transport/container combinations, and source subtitle conversion/burn remain
separate qualification work. No hardware encoding claim is made.

## Rotated copied cuts

The verified cut retains its original coded video and display matrix. The internal
cut-only planner validates the complete orthogonal matrix, plans display geometry
(and inverted SAR for swapped axes), and separately retains coded dimensions/SAR
for every original decoded frame. Changed or malformed transforms fail validation;
ordinary unverified Plan admission remains unchanged. This adds no preprocessing
encode and no public bypass field.

The pinned AV1AN Select reader runs FFmpeg with default autorotation before its
second filtering pipe. Both preview and persisted execution therefore omit the
cut's explicit orientation from that second pipe. Jesses' independent
`-noautorotate` decoder still applies the exact explicit transform. This avoids
rotating twice while keeping the source's original bytes and copied non-video
streams authoritative. Internal packet/reference probes remain hidden.

`tests/source_av1an_rotated_cut_jobs.rs` reproduces the literal original copy
command and pinned engine before running the native held/unchanged-confirm route.
The asymmetric lossless fixture is96×64 coded with90° matrix; both actual engines
produce64×96 and exactly matching first-frame YUV pixels. The cut's complete
ordered packets/PTS/DTS/durations match the independent source command, the
original remains byte-identical, and no FFV1 preparation is introduced.

Evidence in `target/test-artifacts/native-backlog/rotation/`: `before.log`
records the original Display Matrix rejection; `after.log` passes1/1 in17.01s;
`unit.log` passes1/1, including distinct coded/display dimensions and SAR,
no-double-rotation filter construction, changed matrix, malformed matrix and
encoded-side-data rejection. `native-pixel-receipt.json` retains the completed
native job and actual pixel/dimension proof. This qualifies Linux CPU Select;
it does not claim native Windows or every rotation/frameserver combination.
