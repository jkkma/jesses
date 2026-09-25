# Source selection and previews

Quick Convert and av1an normally follow the file selected in **Files**. Use
**Encoding source** to keep one loaded file selected while inspecting another.
Video stream selection, source preview, geometry, color, primary tracks and the
queued request all use that encoding source. Each workflow retains its own choice
and source drafts. Removing a chosen source blocks submission until it is
reselected or **Follow the Files selection** is restored.

Expand **Tracks from other files** in Quick Convert or av1an to include audio,
subtitles and attachments from other imported files into the same encode. Each
selection keeps its source identity and original stream index. Switching the
encoding source or encoder restores that draft's choices; removing or refreshing
a selected file blocks submission until its affected tracks are reviewed again.

By default the output contains primary media tracks, selected external media
tracks, primary attachments, then external attachments. The selected video source
owns geometry and color. External tracks keep their timestamps unless a signed
offset is specified; an external track can extend beyond the video. Up to 100
tracks from 32 other files are supported. Attachments are copied. External audio
defaults to copy; choose AAC, Opus, FLAC, MP3, Vorbis
or E-AC-3 to set each track's bitrate and channel layout. The same source-rate,
speaker-layout and installed-codec checks used for primary audio apply to each
external source. Opus explicitly converts to 48 kHz and FLAC uses 24-bit samples.
Converted audio supports manual gain and loudness measured from the owning source.
Positive offsets delay a track; negative offsets advance it, up to 24 hours in
either direction. Trimming clips the shifted source timeline and rebases it to
zero. Selected audio must be converted when trimming. Container compatibility uses the
chosen output codec and is checked before video encoding. AAC conversion with
audio starting after zero requires Matroska: the MP4/MOV workflow cannot preserve
its priming delay and rejects that combination before encoding.

Quick Convert can convert external text subtitles or burn one selected text or
bitmap subtitle track into the picture. Font attachments are read from that
subtitle's own file. Trimming and offsets apply before rendering; QTGMC renders
the captions into its verified lossless intermediate. Text is drawn after resizing
and before borders; bitmap graphics are overlaid before geometry changes. av1an
retains copy-only subtitle selection. Animated ASS cues crossing a trim boundary
and timed inline WebVTT markup have explicit timing restrictions.

The output layout controls can reorder media tracks across files and change each
selected track's title, three-letter language code, default and forced flags.
Attachments remain after media tracks. Container tags and chapters can each come
from a different imported file, including a file contributing no tracks. Omitted
overrides preserve source values; empty titles or language values clear those tags.
Changing a selected donor requires reviewing the draft again before submission.
MP4 and MOV require a default track for each media type. If disabling the first
track's default flag would be undone by the muxer, preparation asks for another
default track or a Matroska destination. Clearing a title also clears its inherited
handler label.

MOV output can include one existing QuickTime `tmcd` timecode track from the video
source or another imported file. Its frame rate and duration must match the full
video interval. Trimming and cadence changes are blocked because copying a timecode
packet does not recalculate its clock. The track is added after the verified media
stage, and its packet bytes, timestamps and timecode tag are checked in the final
MOV. Other data formats are not silently treated as timecode.

Command previews and target-size measurements include the external selection.
Execution holds read-only source guards and compares copied packet contents and
timestamps before publishing. Converted audio is decoded to verify sample
coverage, original start time and codec delay; output codec, rate, layout and
metadata are checked separately. Recovery also fingerprints external source bytes;
changed or missing files cannot silently enter a resumed output. These selections
are saved in job history and saved requests; they are not applied to folder batches.

## Inspector thumbnails

The inspector reports the original rational frame rate, pixel aspect ratio,
display aspect ratio and rotation reported by FFprobe. Expand **Video thumbnail
& scrubbing** to inspect a bounded frame. Choose the video stream and position;
changing either cancels the previous request and discards late results.

Inspector images honor positive pixel aspect ratios and rotations in 90-degree
steps. HDR images are shown as SDR without changing the source or encode settings.
Arbitrary display-matrix angles are reported but receive an explicit thumbnail
compatibility message. Crop previews use coded pixel coordinates before rotation
or display aspect correction, so the crop outline still targets source pixels.

The preview PNG has its source display matrix cleared after the explicit
transform, preventing image viewers from applying the rotation twice. Source
metadata, sizes and bytes stay unchanged.

## Per-file batch eligibility

Batch encode snapshots only encode requests and revalidates each source. Concat,
reference comparisons, bitrate charts and CRF ladders have separate workflows;
they are not offered as per-file batch operations. Concat has one ordered source
list, comparisons require an explicit reference, and a CRF ladder manages its own
sample/rung set. Their results do not silently become queued encodes.

## Validation

Browser tests cover independent source selection, removal, request identity,
late thumbnail results and stream selection. Real-tool source analysis tests
compare the oriented/anamorphic PNG's decoded RGB against FFmpeg's automatic
display-matrix reference, retain coded crop coordinates, verify source bytes,
and exercise HDR previews and cancellation. This is Windows development evidence;
it does not qualify packaged builds or other operating systems.

The opt-in `external_track_jobs` suite exercises real x264 and av1an/SVT encoding,
copied external packet timing and payloads, primary subtitle conversion alongside
external copies, attachment order, metadata ownership, target-size accounting,
history, offset audio and rejected selections. Additional cases cover independent
external audio conversions with colliding source indices, decoded source identity,
converted target-size accounting and compatible MP4 output. Delayed AAC destined
for MP4/MOV is rejected during preparation. A native stop/reopen/resume check
rejects changed external bytes and saved audio settings before reusing work.
Recovery unit tests also cover replaced files, legacy records and
source-independent workspace cleanup.

`external_track_controls` adds decoded gain/offset/trim checks, exact shifted
packet checks, cross-file ordering, independent metadata/chapter donors and track
overrides. `external_subtitle_jobs` covers source-local fonts, conversion with
overrides, positive and negative cue timing, and generated PGS bitmap captions.
Every bitmap output frame is checked against the expected visible interval, with
and without trimming. The QTGMC case exercises text and bitmap rendering, bob,
trim and lossless verification with an external compatible frameserver runtime;
the bundled runtime currently rejects it because `havsfunc` is absent.

See [FFmpeg's display-rotation option](https://ffmpeg.org/ffmpeg.html#Video-Options)
for the counterclockwise metadata convention.
