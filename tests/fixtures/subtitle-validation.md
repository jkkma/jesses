# Subtitle conversion and burn-in validation

The 2026-09-13 development slice adds per-source-track Copy, SubRip, ASS,
WebVTT and Burn into video actions to standalone Quick Convert and Batch.
Submitted requests retain their selected stream identities and subtitle actions;
changing an action invalidates the batch review. Copy remains the default.

Text conversion compares readable text, cue count/order and timestamps before
encoding. It then exports the muxed result and compares exact target cues and
headers/styles. Formatting and positions can change between formats, and the UI
states that tradeoff. Conversions which cannot preserve readable text are
rejected, including ASS drawings and a characterized FFmpeg entity-to-ASS case.
SubRip/WebVTT precision is 1 ms; conversions involving ASS permit 10 ms.
Source title, language and dispositions are retained; stale statistics are cleared.

Text burn-in uses original or clipped/rebased text assets after crop/resize and
before borders. Bitmap graphics are composited in source coordinates before
crop/resize. The burned track is removed from the output stream list. One burn
track is supported. HDR graphics require explicit HDR-to-SDR tone mapping; the
separate tone-map validation records qualification of that combination.

Font attachments are extracted even when not selected for copying. A known font
MIME type or font filename extension identifies candidate attachments, including
the common octet-stream fallback. Media filenames never determine extracted
paths: a newly reserved private directory contains generated names protected by
the normal owned-file guards. Fonts use visible generated names because libass
skips dot-prefixed files; a native regression detected and prevents silent system
font fallback. Arbitrary source/output paths remain OS arguments, and the decoder
uses its private directory as cwd instead of interpolating paths into filters.
Each capture is bounded to 32 MiB/60 seconds, with at most 128 fonts and 128 MiB
total font payload. Cancellation covers extraction, conversion and rendering;
owned assets are removed on success/failure/cancellation. Source file guards span
the entire existing encode process and source identity is rechecked before publish.

## Executed development gates

On Windows with FFmpeg/FFprobe 9.0.1 and libass:

- Three runtime unit cases verify conversion corruption/precision rejection,
  source-track/HDR/multiple-burn rules, and untrusted font filename handling.
- Four opt-in actual-media cases cover all nine ASS/SubRip/WebVTT combinations,
  clipped timings, retained title/language, exact output frame counts, font burn,
  geometry order, text conversion rejection, cancellation and source preservation.
- The generated original font in `subtitle-font.ttf` consists only of geometric
  block glyphs. It proves that the embedded font is selected even with an
  octet-stream MIME type and a traversal-shaped attachment filename.
- An original synthesized PGS bitmap has a known white rectangle, source position
  and display interval. The output checks its clipped/scaled bounds, black borders,
  subtitle-free track list, all 48 video frames and blank frames outside the cue.
- Two browser regressions cover source indices, one-burn enforcement, independent
  workflow drafts, disabled av1an actions, visible conversion guidance, batch review
  invalidation and unchanged previously queued requests. Labels and help text are
  explicitly associated with keyboard-accessible controls.

```text
cargo test -p media-runtime --lib jobs::subtitles --locked
cargo test -p media-runtime --test subtitle_jobs --locked -- --include-ignored
pnpm test tests/frontend/subtitles.spec.ts
```

The actual 720p episode was also read directly, with a separate 480-frame output
for source interval [1200,1680). Full source validation checked all 34,047 frames.
The successful Matroska contains H.264 1280x720 and FLAC, with 882,883 verified
44.1 kHz audio samples and no remaining subtitle/font tracks. An independent
original-cue/font render of the first selected frame differs from the lossy output
by mean absolute luma error 0.661/255. The source SHA-256, length and modification
time match the pre-run receipt; no partial subtitle assets remain. Original media,
output, visual captures and receipts stay local under `Videos/Jesses-subtitles-20260913`.

## Qualification limits

These are development runtime and browser checks, not clean-machine installation
or Linux desktop evidence. The active container is Matroska; MP4/MOV/WebM output
parity and OCR are separate work. av1an subtitle transformations and bitmap trim
are rejected explicitly. Complex timed text at trim boundaries remains subject to
the documented trim restrictions. Font fallback for absent glyphs follows libass;
no claim is made that conversion retains arbitrary original styling or layout.
