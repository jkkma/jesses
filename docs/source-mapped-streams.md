# Source mapped video and MOV data

Source authority is `QuickConvert.cs:49–110` and `TrackList.cs:425–556`. The
fork rejects multiple mapped video tracks only for direct binary encoders.
The fork also permits NVENC multi-video mapping, but hardware encoders are
excluded by the later explicit CPU-only user scope. Copy retains every checked
video. Data outside MOV and attachments outside MKV are left
out with a source log entry before assigning output metadata ordinals.

Existing `streamIndices`, `ExternalTrack` and `trackOrder` carry these selections;
no additional source UI control is needed. Native copy now accepts additional
video from other guarded files, retains each input's selected indices, offsets,
metadata and order, and verifies every copied packet against its original input.
Direct/AV1AN single-video restrictions remain. Native explicit unsupported
container mappings fail preflight; the source UI performs the documented
container drop/log step before constructing its immutable request.

Mapped data uses a private MOV stage, while unaffected routes keep their private
MKV stage. The MOV stage and final container preserve selected data order,
codec tags, packet bytes and timestamps. Complete streaming data timelines avoid
mistaking MOV's duration span for an absolute end after a source offset. Original
source identities remain held and checked through no-clobber publication. Typed
copy, x264 encode and edited held-command routes use the same owned paths.
Converted MOV text can share the MOV stage with data, with every cue exported
and checked rather than treating container gap packets as subtitle cues.

Explicit tmcd mappings remain real copied packets. MOV muxing disables automatic
timecode generation, so a copied video's derived timecode tag cannot synthesize
an unchecked extra track or duplicate a selected one. Without a mapped tmcd,
that derived video tag is normalized as absent; video packets remain unchanged.
Legacy explicit timecode copying remains separate and retains its existing clock
checks. A data-only gpmd source with real timed packets does not invent video;
its full packet identity is validated without pretending binary data has an AV
decoder.

The pinned FFmpeg9 source-equivalent command that places tmcd before its
associated video writes a MOV which its own FFprobe cannot read (an EEXIST/
`File exists` stream-group error). Native validation rejects this independently
reproduced corruption and never publishes it. It does not silently reorder the
user's mapping. Valid video/gpmd/tmcd order is preserved exactly. This is a
validation boundary for a source-generated corrupt output, not a claim that the
native publisher accepts every mapping FFmpeg can initially write.

`tests/source_mapped_stream_jobs.rs` has five actual integrations: multiple
primary/external videos with interleaved audio; every packet hash/timestamp and
source offset; held source mutation/collision; primary tmcd with a two-packet
external timed gpmd source; ordinary copy, edited held copy, x264 and data-only
outputs; MOV_TEXT text/timing; unreadable source ordering; unsupported data in
MKV; and suppression of unchecked timecode. The generated 755-byte gpmd fixture
and its C generator/provenance live in `tests/fixtures/`; it contains synthetic
opaque timed samples and no user/device recording.

```sh
. target/toolchains/qtgmc-linux-20261101/env.sh
cargo test -p media-runtime --test source_mapped_stream_jobs -- \
  --ignored --nocapture
```

Log: `target/test-artifacts/source-command-edit/mapped-streams.log`.
Hardware encoding investigation is historical and excluded by the latest user
scope. Source Copy+trim with multiple/external video and donors is qualified in
[source Copy trim](source-copy-trim.md). MOV_TEXT without data is covered below;
these are local Linux results, separate from native Windows qualification.

## MOV_TEXT without a data track

A copied tx3g track or explicit Mov_Text conversion now selects an ISO staging
container even when no data track is checked. MOV preserves copied PCM; AV1/VP9
pictures use MP4. Explicit ASS/SubRip/WebVTT conversion out of tx3g retains MKV.
No extra UI setting is introduced. Unknown picture color declarations stay
unknown: a forced colr box is omitted when there is no known source declaration.

The focused default-stack integration in `mov_text_without_data_survives_copy_direct_and_av1an_mp4_mov_and_pcm`
passed ten real outputs: Copy/Direct x264/AV1AN x264 × MP4/MOV, copied PCM+text
MOV, held unchanged AV1+text MP4, tx3g→SubRip MKV, and SubRip→tx3g MP4. Original
subtitle packet hashes/PTS/durations are compared in the six copy/encode paths;
converted captions use the normal complete cue verifier. Original files remain
byte-identical. Log: `target/test-artifacts/source-command-edit/mov-text-no-data.log`.

Copy validation handles Matroska's bounded leading reordered-video DTS omission
and the inverse ISO reconstruction. Payload, order, every PTS/duration and all
later DTS remain checked; explicit source offsets still apply. This policy is
video-only, limited to16 leading packets and the relevant container, and never
permits an interior timestamp hole. A separate actual x264 B-frame fixture pins
the omitted prefix and rejects a wrong100ms offset. Log: `copied-b-frame-dts.log`
in the same directory.

## VP9 in source-profile ISO outputs

A source-trimmed VP9 picture with MOV_TEXT can now be written directly into
MP4 without falsely requiring Matroska's chroma-siting header. ISO VP9 omits
that declaration even when its complete encoded video access units are copied
unchanged. Before admitting this one missing value, the source-final-ISO path
checks the validated checkpoint and every final video packet's SHA-256, size,
order, duration and PTS/DTS under the proven mux clock. Explicit conflicting
chroma, codec, pixel format, color and SAR declarations still fail; the rule
does not apply to missing Matroska fields or another codec. Every final frame
is still decoded and its timing/geometry checked.

`source_vp9_iso_chroma` passed the actual source trim + VP9 + MP4 + MOV_TEXT
workflow (4.07 s). A separate header regression checks missing-only admission
and five explicit conflicts, and the real packet-verifier regression rejects
changed payload, missing tail and shifted timestamps. Receipts are
`target/test-artifacts/native-backlog/vp9-iso-after.log`,
`vp9-iso-headers.log` and `vp9-packet-negatives.log`. This is local Linux
qualification, separate from native Windows/macOS execution.

## Exact tx3g rendering headers across ISO muxing

Qualified FFmpeg9 MP4 muxing copies the tx3g codec header and may append a20-byte
`btrt` bitrate box (`movenc.c`703–759,2246–2285). Independent source-equivalent
copy/remux proves two cases: an existing identical hint can repeat, or a48-byte
MOV header containing the30-byte rendering record and18-byte Arial font table
receives its first hint. The latter has buffer size0 and max/average328, matching
the unchanged input MOV's sample-table-derived bitrate before any `btrt` exists.

The comparator validates all atom boundaries. It preserves every rendering,
font/style and unknown-atom byte/order; it only folds identical20-byte bitrate
repeats, or accepts the first exact trailing hint with buffer0 and a positive
32-bit max/average equal to that unchanged input's probed bitrate. Changed,
conflicting, missing/malformed or reordered declarations cannot authorize a
style difference. Complete subtitle payload/order/PTS/duration and decoded cues
remain independently checked; no header-hash bypass or re-encode is used.

The final bounded Linux pass is green: tx3g3/3 (positive, wrong-rate, malformed,
style/font and unknown-order negatives) and the complete mapped-stream6/6,
including all ten outputs in the no-data text matrix. Local receipts are
`target/test-artifacts/final-small-checks/`; exact commands, resource limits and
Windows qualifications are recorded in [final Linux validation](final-linux-validation.md).
