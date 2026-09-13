# Container conversion validation

Quick Convert, Batch encode, Remux and multi-source mux accept `.mkv`, `.mp4`,
`.mov` and `.webm` destinations. Batch has an optional container setting; missing
historical requests retain Matroska. The destination selector describes subtitle
conversion and MP4/MOV's first-default-track behavior before submission.

| Container | Allowed video                        | Allowed audio                                                   | Subtitles and attachments                                                                                  |
| --------- | ------------------------------------ | --------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| Matroska  | Existing validated copy/encode paths | Existing validated copy/encode paths                            | Compatible text/bitmap tracks and attachments; MP4 text requires explicit text conversion in Quick Convert |
| MP4       | H.264, HEVC, AV1, VP9, MPEG-4        | AAC, MP3, AC-3, E-AC-3, ALAC, Opus, FLAC                        | Text becomes MP4 text; bitmap tracks and attachments must be deselected or burned where supported          |
| MOV       | H.264, HEVC, MPEG-4, ProRes, MJPEG   | AAC, MP3, AC-3, E-AC-3, ALAC, signed little-endian PCM 16/24/32 | Text becomes MP4 text; no bitmap tracks or attachments                                                     |
| WebM      | VP8, VP9, AV1                        | Opus, Vorbis                                                    | Text becomes WebVTT; no attachments or bitmap tracks                                                       |

These are explicit preflight allowances. The tests below qualify representative
codec combinations; they do not establish every allowed combination on every OS.
Unknown track types, unsupported codec pairs, empty converted subtitle tracks,
MP4/MOV track metadata beyond language/title/provenance, unsupported dispositions,
and non-title MP4/MOV chapter metadata receive actionable constraints. Text
conversion can change font/style/position; every readable cue, line break, order
and timing is checked. Invisible trailing spaces before line breaks may be removed.
Unrepresentable drawings, changed cues and timing fail publication. MP4 text input
can be explicitly converted to SubRip, ASS or WebVTT in standalone Quick Convert.

Every output starts with the existing fully validated Matroska stage. A different
container uses a second owned sibling; source fingerprints remain guarded through
final publication. Output validation checks exact stream order, codec/media
properties, track language/title, dispositions, chapters and duration, plus every
copied media packet's SHA-256, size, order and timing. MP4/MOV titles are validated
as `handler_name`; the one expected chapter auxiliary stream is structurally
validated before being excluded from the media-track comparison. Extra streams
cannot bypass that check.

MP4 may reconstruct a bounded leading prefix of missing Matroska video DTS.
Known DTS, PTS and durations remain within 2 ms; coded payloads remain identical.
Copied audio is fully decoded on both sides. Start, end and sample count must stay
within one source time-base tick (at most 2 ms); AAC can discard only the final
padding already established by the last source packet's shorter duration.
QuickTime's color-range limitation is handled for H.264/HEVC through codec VUI
metadata, with coded packet hashes still checked. Color changes fail validation.

For encoded video, the previously validated rational frame clock is restored
after the Matroska intermediate's millisecond quantization. PTS presentation rank
is retained through B-frame reordering. The final output is fully decoded again
and checked against its exact frame count/clock. Plain stream-copy jobs retain
source timestamps. All final video/audio must decode without errors. Existing
destinations are never overwritten; cancellation before the shared publication
lock cannot publish either intermediate.

## Automated evidence

On Windows with FFmpeg/FFprobe 9.0.1 and standalone x264:

- `cargo test -p media-runtime --lib jobs::container --locked`: 2 checks passed,
  covering codec/metadata/disposition/empty-track rejection and strict chapter
  auxiliary classification/title/default normalization.
- `cargo test -p media-runtime --test container_jobs --locked -- --ignored
--nocapture`: 4 actual-tool gates passed. H.264/AAC/subtitles to MP4/MOV and
  VP9/Opus/subtitles to WebM preserve reordered streams, titles/languages, chapters,
  full decoded frame count, source SHA-256/mtime and cleanup. Multi-source edits
  survive MP4; incompatible H.264/AAC WebM fails without output. Quick Convert
  trims to 24 frames in MP4 and imports its MP4 text back into Matroska. A separate
  48-frame `24000/1001` x264 case proves B-frame presence and exact final cadence.
- All-target Clippy with warnings denied passed.
- `pnpm exec playwright test tests/frontend/containers.spec.ts --workers=1`:
  2 browser checks passed for labeled container choice/destination updates and
  Batch review invalidation with previously queued requests unchanged.
- `pnpm check`: zero errors or warnings. The shared selector carries an explicit
  accessible name; no native UI result is inferred from the browser checks.

## Original-media evidence

A read-only 1280×720, 34,047-frame episode was qualified through its complete
source scan, then frames 1200–1680 were encoded as H.264/AAC with seven MP4-text
cues. The final MP4 has 480 frames at `2997/125` fps (20.020020 seconds), with a
maximum independently measured timestamp error below 0.5 microseconds. Its
English subtitle title/language and Japanese audio language survive. Source
SHA-256, size and modification time are unchanged. Output size is 7,230,668 bytes;
SHA-256 is `fc80909000e6d766affa083a10989882f2605d9549f301f8e5fcf75f0feee4aa`.
The local receipt retains the request, runtime log, full probe and independent
frame-clock/source-integrity calculation. Media and personal paths are not stored
in this repository. Native UI output qualification and Linux workflow execution
remain separate evidence.
