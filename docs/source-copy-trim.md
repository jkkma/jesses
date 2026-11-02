# Source Quick Copy with trim

Quick Copy now uses one FFmpeg operation over the original mapped inputs. It
seeks every input before `-i`, retains source keyframe preroll, and writes the
requested duration. Source frame/time-exact sections become millisecond
keyframe-copy sections (`QuickConvert.cs:162–184`,
`QuickConvertUi.cs:1381–1440`, `TrimSettings.cs:60–80,218–230`). No first-video or
single-file restriction is added: checked primary/external video, audio,
subtitles, MOV data, metadata/chapter donors, offsets and output order retain
their source-scoped identities. Hardware encoding remains excluded by the
user's CPU-only instruction; packet copy remains supported.

The internal `sourceTrim` snapshot selects this source behavior. Older typed
requests keep their original compatibility path. No additional UI control is
exposed. The original request/history is never rewritten to point at a private
cut. Unchanged Shift+Run uses the same typed verification; edited programs use
the established explicit edited-command contract. Final bytes are decoded and
published directly through the existing atomic no-clobber/source guards.

An independent original-input reference pins both ends of each selected packet
sequence and the shared mux clock, including copied-GOP preroll and encoder
priming. Every copied access unit is also checked against its original source
for payload/order/PTS/DTS/duration. Known ISO/Matroska leading video-DTS
representation changes remain limited to their bounded prefix and are backed
by the complete same-container reference. Converted subtitles use the original
maps and actual requested codec; full reference packet/header/style/clock proof
precedes adoption of their output metadata. ASS animation is not rewritten into
a clipped text asset. Donors and originals remain guarded through publication.

Ordinary converted audio retains strict decoded source-window sample/clock
checks. Measured normalization can produce different DSP timing: FFmpeg's
zero-LRA dynamic fallback can emit a timestamp hole even for stationary input,
as the source's `LoudnessConfig` documentation acknowledges. That path must
match the exact measured-filter reference in complete encoded packets, codec
headers, every decoded frame/sample timestamp and count, and a full PCM hash.
It does not receive a global timestamp-gap tolerance. Input-side accurate seek
applies authored offsets before selecting converted samples, matching FFmpeg's
source command semantics.

The trim dialog consistently follows the first checked video source and its
checked stream. This intentionally corrects a fork inconsistency:
`MainWindow.QuickConvert.cs:284` gives `CutWindow` the loaded main file, and
`CutWindow.Load:130–140` uses that file's first video/FPS, while run validation
resolves the mapped source. Jesses keeps the existing controls but uses one
source identity for duration, frame nudges, preview, keyframe lookup and the
submitted request. Source changes no longer overwrite that checked video index
with the new file's first stream after the track-selection effect runs.

Evidence:

- `tests/source_copy_trim_jobs.rs`: six actual default-stack gates cover all
  three trim modes; exact independent packet parity; nonfirst/external videos,
  offsets/order and metadata/chapter donors; original/nonzero source clocks;
  converted and normalized primary/external audio; SubRip/MOV_TEXT/WebVTT
  conversion; B-frame input; held unchanged/cancel; later-owner collision; and
  mutated donor rejection without replacing its later bytes.
- `source_copy_packets` has a separate real negative gate for a 100 ms whole
  output shift and an interior PCM packet deletion.
- `tests/frontend/source-copy-trim.spec.ts`: two browser cases cover nonfirst
  video in the main file and in a different file while the main remains loaded.
  They assert the selected 60 fps nudge, source duration, preview/snap input and
  stream, and frozen final request.

Logs: `target/test-artifacts/source-command-edit/source-copy-trim.log` (6/6),
`source-copy-trim-browser.log` (2/2), `source-copy-helper-negative.log` (1/1),
and `source-copy-held-final.log` (the final held lifecycle recheck).
These establish local Linux execution; native Windows/macOS qualification is
separate.
