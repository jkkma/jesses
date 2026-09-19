# Encoder parameters, saved presets and command-plan validation

The qualified scalar catalog covers standalone x264 reference frames, B-frames,
B-frame adaptation, adaptive quantization, trellis and lookahead; the three SVT
families expose adaptive quantization, temporal filtering, overlay frames and
hierarchical prediction. FFmpeg libx265 has a separate nested-parameter catalog
(reference frames, B-frames, adaptation, AQ, SAO and CU-tree), and libvpx-vp9 exposes
AQ, lookahead and alternate references. CLI and library spellings stay separate.

Each identifier must exist in its encoder catalog, appear once, and contain an
integer in the catalog's qualified range. No user text becomes an executable,
option name, path, filter or separator. Application-owned input, output, color,
clock, process, rate-control and pipeline settings remain protected. The actual
installed encoder must advertise each native option. The opaque libx265 parser
also receives a bounded, real three-frame probe; unknown-parameter diagnostics
fail preflight. Speed presets apply first and explicit scalar overrides afterward.
Av1an's common encoder vector feeds final encoding, quality probes and recovery
settings together.

Saved parameter presets replace only the override list. They are scoped by
encoder and backend and do not reset quality, speed, paths or source processing.
Up to 30 validated presets are stored by Rust in the versioned preferences file
under the resolved application configuration directory. Installed and portable
instances therefore use their own configuration roots. Dedicated save/remove
operations atomically edit only presets, preserving concurrent general settings
and recent-media edits. Invalid, unknown-version, oversized or corrupt preference
files remain untouched and saving is blocked with an error. No WebView local
storage is used for these presets. Existing version-1 preferences without a preset
field load with an empty list; arbitrary legacy encoder strings are not imported.

Command preview is an explicit, cancellable action. It runs the existing source
fingerprint guard, complete video timeline/metadata scan, selected-audio checks,
encoder capability checks, trim/subtitle preparation and rate-control resolution.
The result contains native executable paths, argument arrays, working directories,
source fingerprint and the verified output frame count/rate. Values are shown as
JSON argv arrays, never shell commands. Two-pass plans show a fresh decoder for
each pass. Editing a draft or leaving the component cancels and discards stale
results. Preview creates no queue/history entry or final media, never enters an
av1an recovery workspace, and removes its temporary preparation files before
returning. Actual execution allocates fresh temporary names and repeats validation.

The preview clearly identifies two deferred details: av1an execution stages its
managed launcher/dependency environment and performs engine/plugin/scorer checks;
and non-Matroska final container commands depend on the completed encoded stream's
headers and timestamps. These commands are not fabricated in the preview. The
validated source, encoder and Matroska staging commands are still reviewable.

## Recorded checks

- `cargo test -p media-runtime --test parameter_jobs --locked -- --ignored --nocapture`:
  two real-tool tests passed on Windows. All six encoder families produced 48
  decoded frames with representative overrides. x264 and x265 B-frame overrides
  took effect despite the fastest speed preset. The selected mainline SVT binary
  was source-built 4.2.0; a differently identified fork on PATH was correctly
  rejected before selecting the explicit qualified mainline binary.
- The second real-tool case scanned a source path containing spaces, ampersand,
  dollar sign, percent sign, apostrophe and Unicode. A trimmed 24-frame two-pass
  plan preserved that path as one argument, showed both 250 kb/s passes and the
  requested B-frame override, left no output or temporary files, and preserved
  the source hash. Pre-cancelled preview also left the source directory unchanged.
- The scalar-validator unit test rejects protected flags, duplicate identifiers,
  invalid ranges and shell/nested-parser separators while checking distinct
  native CLI and library argument spelling.
- Six preference-store unit tests passed, including concurrent preset/general/
  recent changes, config-root isolation, old-record compatibility, and corrupt
  preset read/save/remove preservation.
- Svelte type and accessibility diagnostics: zero errors and zero warnings.
- `pnpm exec playwright test tests/frontend/parameters.spec.ts`: four browser
  tests passed in 6.3 seconds after the final Rust-persistence integration. They
  cover range rejection, saved preset/encoder isolation, immutable submitted
  requests, cancellation and stale preview replies, Batch review invalidation,
  and unreadable preferences disabling Save with the preserved-file error.
  Earlier attempts timed out before application assertions while the development
  server was unresponsive; a warmed server logging to a file resolved that setup
  issue.

## Original 720p media

A read-only, complete 34,047-frame episode was scanned, then source frames
`[1200,1680)` were encoded to MP4 with x264 CRF 18/fastest speed preset and explicit
`ref=3`, `bframes=4`, `b-adapt=0`, `aq-mode=2`. Independent final-output inspection
found 480 progressive frames at 2997/125 fps, including 382 B-frames, with maximum
presentation-clock error below 0.500 microseconds. H.264 video, Japanese AAC audio
and seven English text-subtitle cues were present. Audio frame inspection decoded
882,874 samples per channel after MP4 edit-list handling (the source clip contains
882,883 samples; this is within the existing container sample-timeline bound).

The output was 5,644,355 bytes with SHA-256
`449c1c54dbec1fa923872fa05266a66829e8f604e4ff9f94b835dba7d8507d78`.
The source's complete SHA-256, byte count and modification timestamp were unchanged.
The private local evidence directory retains the exact request, runtime log,
independent frame probe, subtitle export and JSON verification receipt. This is a
runtime media check; native control interaction is qualified separately.

## Native command plan and two-pass output

The rebuilt Windows portable application was exercised on a read-only-derived,
three-track 20-second episode fixture. Native controls selected standalone x264,
250 kb/s two-pass rate control, medium speed, `ref=3`, `bframes=4`, 30 fps, bicubic
960x540, AAC 128 kb/s and MP4 output. Command preview reported 601 output frames
and showed the second-pass argv before any final output existed. The successful
job's persisted settings match these controls; retained first- and second-pass
logs each report all 601 encoded frames.

Independent output verification found 601 progressive 8-bit BT.709 frames, with
every integer timestamp exactly equal to its frame index divided by 30. The
output contains eight I-, 137 P- and 456 B-frames. The measured video bitrate was
approximately 262 kb/s on this short clip; the requested average bitrate is not
an exact size cap. All five subtitle cue times and readable text match; MP4
retains font size but drops the source font-face markup, as disclosed by the
container control. Audio contains 882,874 samples per channel, nine fewer than
the source and within the 47-sample boundary bound derived from its 1 ms clock.

Output SHA-256 is
`43ebc25b049d7d55a888aea6b5d822a731edcfaa3667b3c46c93b0cdf7fe16d0`
(999,863 bytes). The original episode, derived source and selected-track fixture
retain their full hashes, sizes and modification times. Local receipts are
`final-validation.json`, `native-job-validation.json` and the retained native
job/pass logs under `Videos/Jesses-final-native-20260913`.

Native review also exposed a hardcoded MKV summary and a source-rate label after
frame-rate conversion was selected. Both display paths now reflect the draft.
Four focused container/temporal browser cases pass, including av1an's shared
container summary, rational/invalid/bob rate labels, workflow draft restoration
and Batch request immutability. Svelte diagnostics remain clean. This subsequent
display correction awaits the final rebuilt package's native check; these
results do not claim clean-machine package qualification. Linux qualification is
deferred.

The same native portable build was then closed normally and restarted. Opening
its persisted recent source, choosing x264 and applying the persisted parameter
preset restored `ref=3` and `bframes=4` in the controls. The preset came from the
portable Rust preferences record; this does not claim persistence of every
transient encode draft. Both application instances exited normally. Native
evidence is `native-preset-restart-applied.png` in the same local evidence folder.
