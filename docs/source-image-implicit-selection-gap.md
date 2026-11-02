# Fixed-image export with no checked video

The additive native `ImageRequest::ImplicitExport` route implements the source
workflow when at least one nonvideo track is checked and no video is checked.
It preserves all loaded processing inputs in order, including guarded imported
image sequences. It does not inject a selected-file video map: FFmpeg chooses
video automatically from every loaded input. The existing explicit-video
`Export` request remains strict and unchanged.

One source bug is deliberately corrected: fixed image outputs suppress audio,
subtitle and data output. No extra setting is exposed. Copying a subtitle into
image2 produces a file bearing a PNG/JPEG extension that contains subtitle text;
GIF instead fails because its muxer cannot store that stream. Retaining the
source controls and automatic video selection does not require creating those
invalid images or carrying that GIF failure into Jesses.

## Source evidence and correction

Inspected against the current fork on 2026-11-01:

- `Nmkoder/UI/Tasks/QuickConvert.cs:49–64` requires a checked track but separately
  records whether any video/audio is checked.
- `QuickConvert.cs:325–344` includes all loaded inputs outside Batch and omits
  video codec arguments and picture filters when no video is checked. Retained
  PNG depth, GIF palette, resize/crop, tone mapping, QTGMC and other picture
  values therefore do not affect this route.
- `QuickConvert.cs:363–393` still emits applicable audio, subtitle, metadata and
  trim options. Fixed-format `TrackList.GetMapArgs(videoOnly:true)` emits no maps
  when no checked video survives. `TrackList.cs:385–408,484–517` preserves input
  order and does not add a fallback video map.
- `QuickConvertUi.cs:1397–1440` repeats input-side seek before each input and
  independently emits output-side trim. Its frame-mode trim clock is the first
  video of the current source, not necessarily FFmpeg's automatically selected
  video. `TrimSettings.cs:104–150` supplies the half-frame seek/window and
  optional count. Metadata retains the source's output ordinals, even when
  automatic selection differs from the checked row order.

The dedicated real-media matrix creates a one-second, four-frame MKV containing
video, PCM audio and one SubRip cue. Against pinned FFmpeg 9.0.1, the source-shaped
commands have these observed results:

| Nonvideo options                 | PNG/JPEG numbered files                                           | GIF                           |
| -------------------------------- | ----------------------------------------------------------------- | ----------------------------- |
| `-c:s copy`                      | 5 files: 4 valid images plus the literal 7-byte `Caption` payload | muxer rejects subtitle stream |
| `-sn`                            | 4 valid images                                                    | succeeds                      |
| audio Copy/Strip/AAC, with `-sn` | 4 valid images                                                    | succeeds                      |

The source's successful process exit is therefore insufficient image validation.
The correction emits `-an -sn -dn`, omits irrelevant audio/subtitle encoders and
loudness work, and retains source metadata/trim and automatic video selection.
All three fixed formats now produce the four valid frames. Audio-only inputs
with no loaded video fail explicitly rather than becoming falsely named audio
files. The matrix does **not** claim audio corruption in the observed source
Copy/AAC cases: those audio streams were already omitted by automatic selection.

## Native contract and integrity

The request contains ordered `inputs`, raw `checked` input/stream identities,
`currentInput`, output family/path, frozen typed nonvideo settings, metadata,
trim, and the source `keepFrameCount` verdict. There are no normal-path raw
arguments or picture settings. Every loaded candidate is opened and guarded,
including candidates FFmpeg does not choose. Empty selection, checked video,
missing checked identities and missing current input are explicit errors.

Global/chapter donors, explicit empty title/language values, output-ordinal
track metadata and dispositions retain their source syntax. Audio/subtitle
settings are frozen but have no output stream to affect in a fixed image export.
Frame trim uses the current source's first-video average rate, with nominal-rate
fallback, the source half-frame window and optional `-frames:v`. Input-side time
seek is repeated for every input. An unknown current frame rate cannot silently
fall back to a different loaded video's rate.

The generated no-map program is held by the same native prepare/confirm/cancel
broker as other Quick Convert paths. No requested conversion runs while the
review is open. Unchanged confirmation executes the typed automatic-selection
program. An explicitly edited program may change valid image geometry/trim;
its actual frames determine the result. Closing, clearing, renderer navigation,
cancellation and manager shutdown join the image task and release its assets.

Both routes write one identity-guarded GIF or image2pipe spool, verify the chosen
image family and completely decode every actual frame. Sequence publication
checks every compressed packet and every byte, reserves each destination, then
stream-copies packets without re-encoding and verifies the packet hashes again.
Source identities and cancellation are checked before atomic no-replace
publication. Failed/canceled path writers rebind their reserved file identities
before cleanup, preventing zero-byte spool/log leaks. Existing ownership and
symlink/replacement checks remain in force; no directory adoption for malformed
source packets was added. Spooling temporarily retains approximately another
encoded output's disk space, as documented in `source-image-command-edit.md`.

## Qualification

`crates/media-runtime/tests/image_implicit_jobs.rs` supplies eight actual Linux
native gates:

1. Fifteen format/nonvideo-mode combinations reproduce the source outcomes above
   and compare corrected native compressed PNG/JPEG packets, or decoded GIF
   pixels, against independent valid FFmpeg output.
2. Automatic selection includes two loaded inputs and multiple videos, choosing
   the actual largest video independently of the checked row/current source.
3. Equal-resolution ties follow loaded input order, confirmed by image colors.
4. Current-source frame clock, half-frame windows, count-preserving/count-free
   trims, exact time trim and repeated input seek.
5. Typed empty/invalid/checked-video selections and audio-only/no-video failure.
6. Source metadata/donor/output ordinal arguments, ignored nonvideo loudness
   settings, unchanged review and valid edited geometry.
7. Held review/close, mutation of an automatic candidate, late destination
   collision, cancellation and owned cleanup.
8. Cancellation during actual encoded-image spooling; native navigation
   cancellation joins the task before returning and publishes nothing.

The evidence is
`target/migration-validation/image-native-20261101/implicit-image-matrix.log`.
The separate `image-command-edit.log` covers the eight explicit image command
regressions, including a confirmed ordinary native job surviving cancellation
of a held image review during renderer navigation. These are real Linux proofs,
not mocked browser or native Windows/macOS execution claims.

```sh
. target/toolchains/qtgmc-linux-20261101/env.sh
cargo test -p media-runtime --test image_implicit_jobs --locked -- \
  --ignored --nocapture
```

The source's `%8d` pattern in `QuickConvert.cs:1222–1223` was verified against
actual FFmpeg output: it starts at `00000001.png`/`.jpg`, with no prefix. Source
full-sequence exports now use that spelling for normal, reviewed/edited and
implicit routes. Legacy ranged `Export` requests (`fullSource:false`) keep their
existing `frame-%06d` names; this is an API compatibility path, not a UI toggle.
The dedicated image gates check exact names/counts, cancellation and late output
collision alongside compressed packet identity. This closes the earlier literal
source filename gap without claiming completion of the overall migration.
