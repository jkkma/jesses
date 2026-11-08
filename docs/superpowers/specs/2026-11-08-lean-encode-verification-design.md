# Lean encode verification

Date: 2026-11-08. Status: approved.

## Goal

Encode jobs finish lean: prepare what the encode needs, encode, mux, check the
finished file's frame count, publish. The extra passes that only
re-check a result move behind one setting that is off by default. Turning the
setting on keeps today's behaviour exactly.

## Background

A 5-minute trim of an 18 GB 4K Dolby Vision MP4, encoded to 1080p AV1 with
SVT-AV1-HDR, Spline (GPU) tone mapping, two Opus tracks and 25 subtitles
converted to SubRip, spent most of its non-encoding time verifying:

| Phase | First run | After the uncommitted speedups |
| --- | --- | --- |
| Preparation (until the encoder starts) | 2:29 | 1:45 |
| Finishing (after the encoder exits) | ~1:27 | 0:16 |

The remaining preparation time is mostly a whole-file packet count and hash of
the source (~16 s), a separate decode of the trimmed range (~51 s, now partly
beside the encoder) and source/tool hashing for resume.

The lean target checks only that the required tools are present, every tool
exits cleanly, the output exists, and the finished file's frame count matches
the plan, failing the job on a mismatch. It hashes nothing, compares no packets
or decoded frames, checks no audio samples, and has no mid-job resume: a failed
job is rerun.

## Decisions

- Keep the current pipeline (FFmpeg producer with libplacebo piped into the
  standalone encoder, FFmpeg mux). Only the verification changes.
- Gate, do not delete: verification-only passes run only when the switch is on.
- Default runs keep no resume workspace. An interrupted default run is rerun from
  the start.
- Applies to every job that goes through the shared encode flow: Quick Convert,
  batch and AV1AN. The non-video convert path is unchanged.

## The switch

- The stored field stays `fullOutputVerification` (`GeneralPreferences` and
  `EncodingPreferences` in `crates/media-core/src/preferences.rs`), default
  `false`, so saved preferences, saved plans and queued jobs keep loading. Plans
  saved without the field run the default (fast) way.
- Settings label (`src/lib/features/source/SourceSettings.svelte`): "Thorough
  verification (slower)". Tooltip: off, the finished file is checked by its frame
  and stream counts only, and an interrupted job is rerun from the start; on, the source and tools are fingerprinted so an interrupted job can
  resume, every frame is decoded, and every track of the output is re-checked.
- Doc comments on both fields describe the new meaning.
- One runtime helper (today `full_output_verification(settings)` in
  `crates/media-runtime/src/jobs/encode.rs`) answers "thorough?" at every gate.
- A default run logs one line saying thorough verification is off and the output
  is checked by frame and stream count.

## Default run (switch off)

Rule: a pass that only checks a result is skipped. Where a pass also computes a
value the job needs, the job keeps computing that value and skips only the
comparison.

Before encoding:

| Pass | Default run |
| --- | --- |
| Source header probe | Header only: no `-count_packets`, no `-show_data_hash` over the whole file. Values that later steps need from it keep coming from a pass that already reads them (see "Values to keep"). |
| Tool version and capability checks, audio delay/conversion fixture encodes, SVT `--help` | Kept: about a second in total, and they fail early with a clear message. |
| Packet-timestamp frame plan and first-frame read | Kept (builds the plan; no decode). |
| Background decode of the trimmed range (`source_timing` spawn and `settle`) | Skipped. |
| Audio decode up to the trim end (`audio::scan_until`) | Kept (builds the audio cut). |
| Full-track audio scan without a trim (`audio::scan`) | Only where its start value is needed (MP4/MOV AAC priming); otherwise skipped. |
| Tone-map GPU probe and peak measurement | Kept (builds the tone map). CPU-chain `check_tools` is a gate; kept, it is milliseconds. |
| Trim interval, subtitle cue clipping, chapters, subtitle conversion | Kept. |
| Standalone recovery `prepare` (source and tool SHA-256, external source fingerprints) | Skipped; no recovery workspace. |

During encoding:

| Pass | Default run |
| --- | --- |
| Producer and encoder pipeline | Unchanged. |
| `Source::verify` identity guard before each pass | Kept (file identity only; reads nothing). |
| Encoded-video checkpoint validation (`validate_video_checkpoint`) | Skipped; the final frame-count check replaces it. |
| Recovery checkpoint copies (`checkpoint_video`, `checkpoint_pass_one`) | Skipped. |

After encoding:

| Pass | Default run |
| --- | --- |
| Mux | Unchanged. |
| Output probe | One probe of the finished file: stream list plus the video stream's packet count. The job fails with `OUTPUT_VALIDATION_FAILED`, removes its temporary and publishes nothing when the video frame count differs from the plan, or the output's streams differ in number or type from the planned output streams (selected source tracks plus external tracks). |
| Copied/converted track comparisons (`trim_source` verify, external and data stream packet comparisons) | Skipped. |
| Metadata comparisons against the expected document | Skipped. |
| Subtitle re-reads (trim and conversion) | Skipped. |
| Output audio decode and timeline check | Skipped. |
| Packet-clock check and 16-frame sample | Skipped. |
| Full output decode | Skipped (as today when the switch is off). |
| Dynamic HDR output verification | Skipped. |
| Recovery checkpoint of the final file | Skipped. |
| Source identity guard and atomic publish | Kept. |

## Thorough run (switch on)

Every pass above runs exactly as today, including resume checkpoints and the full
decode of every output frame. The uncommitted speedups (background source
fingerprint, shared donor fingerprint, batched subtitle checks, thinned progress
log) stay and keep speeding up this mode.

## AV1AN and batch

- Batch items run the same encode flow and follow the switch.
- AV1AN-only checks that only re-check results (for example
  `verify_actual_output`, `verify_source_join`) get the same gate.
- AV1AN's own chunk resume is av1an's feature and is not changed.

## Values to keep

Some verification passes also produce values. Each is confirmed against the code
while writing the plan; if a value feeds the output, the default run keeps
producing it (cheaply where possible) and skips only the comparison:

- Source probe fields (`packet_start_time` for subtitles, packet counts,
  `extradata_hash`): which feed the mux, metadata or plan.
- `trim_source` verify origin (`plan.output_origin`): whether anything besides
  later checks reads it.
- No-trim `audio::scan` start value (MP4/MOV AAC priming).
- Dynamic HDR per-frame hashes (expected to be comparison-only).

## Error handling

- Tool failures and non-zero exit codes: unchanged.
- Frame-count or stream-count mismatch: `OUTPUT_VALIDATION_FAILED`, temporary
  removed, nothing published, source untouched.
- Source swapped or modified during the job: `SOURCE_CHANGED`, as today.

## Testing

- Tests that assert verification behaviour set the switch on and keep covering
  thorough mode.
- New tests:
  - A default run invokes none of the skipped passes. Assert on the recorded
    FFmpeg/FFprobe invocations (as `source_filter_image_jobs.rs` does) or on the
    job log.
  - A default run whose output frame count differs from the plan fails without
    publishing.
  - A default run whose output is missing a planned stream fails without
    publishing.
- Frontend: label and tooltip text, and the saved field's name and default.
- Real-media acceptance with the 4K job above: the same output bytes as today's
  runs, preparation in roughly 30 s, finishing in the mux time plus a few
  seconds.

## Out of scope

- Encoding audio in parallel with video (a possible follow-up).
- Hardware video decode; libplacebo GPU scaling (measured slower here: 19.9 vs
  26.0 fps).
- Resume for default runs.
- Changing what thorough mode checks.

## Sequencing

1. Commit the tested, uncommitted speedups first, as three commits: progress-log
   thinning; source fingerprinting beside the encoder and shared donor
   fingerprint; batched converted-subtitle checks.
2. Implement this design on top.
