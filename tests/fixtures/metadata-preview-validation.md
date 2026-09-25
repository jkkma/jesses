# Metadata and display preview validation

The inspector keeps common information visible and puts detailed file and stream
metadata in collapsed disclosures. Stream indices remain the original FFprobe
indices. Full codec names, profiles, default-track flags, field order, audio
channel layout, subtitle kind, and attachment filename/MIME type are reported
when available. Unknown subtitle codecs remain unknown.

File sizes and reported bitrates retain exact decimal text across IPC. Detailed
durations retain fractional seconds. Average and nominal frame rates retain
their original rational strings; a difference between them alone is not labeled
as variable frame rate. Missing metadata remains unknown. No exact rate is
invented from a rounded media-tool diagnostic.

Per-stream numeric duration takes precedence over a Matroska `DURATION` tag;
file duration is the preview fallback only when stream duration is unavailable.
Seek controls use the selected video's duration and frame interval. Requests at
or beyond the reported end are rejected by the backend.

Display thumbnails apply pixel aspect ratio and the full orthogonal display
matrix, including reflections and quarter-turn rotations. Unsupported matrices
produce an explicit error, while the coded source preview remains available for
crop work. Crop coordinates always refer to the coded source. HDR thumbnails
use the existing display-only SDR conversion.

Seeking or refreshing clears the previous image. Numeric editing cancels an old
request immediately; source changes, stream changes, closure and disposal discard
late completions. A failed refresh clears an earlier crop proposal. Applying a
proposal requires a current image with the same source fingerprint and remains
an explicit action.

## Repeatable checks

```sh
cargo test -p media-runtime --lib probe::tests --locked
cargo test -p media-runtime --lib analysis::tests --locked
cargo test -p media-runtime --test analysis_jobs --locked -- --ignored --test-threads=1
pnpm exec playwright test tests/frontend/metadata.spec.ts tests/frontend/analysis.spec.ts
```

Parser tests cover mixed stream types, sparse indices, absent and malformed
metadata, exact large decimal values, both rational rates, tagged duration, and
older serialized records. Browser tests cover detailed disclosures, audio-only
files, short selected videos, immediate numeric cancellation, late completions,
and failed-refresh crop invalidation. Actual-tool tests compare decoded preview
pixels with FFmpeg's automatic display orientation, reject seeks beyond a short
Matroska video, and verify cancellation, source preservation, and coded crop
coordinates.

Packaged Windows evidence is recorded separately from source tests and release
qualification. Linux and macOS native qualification remain deferred.

On 2026-09-24, a local Windows x64 portable debug candidate passed 320 ordinary
Rust tests, all seven actual-tool analysis cases, and 276 browser tests. Native
WebView2 checks compared 22 fixture probes and 15 decoded previews, exercised
four expected rejections and active cancellation, and checked failed-refresh
recovery through the UI. Source hashes, sizes and modification times stayed
unchanged; all package payload hashes matched after use and no owned process
remained after graceful close. This is current-host development evidence, not a
release or fresh-machine qualification.
