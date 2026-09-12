# Audio conversion validation

Development qualification on Windows on 2026-09-12 covers per-track audio
conversion in the standalone encoding workflow. The automated gates and short
real-media checks below have passed. Full-episode, native-interface, and CI
qualification remain pending in the final section.

## Supported behavior

Quick Convert and batch encoding offer Copy source, Opus, and AAC independently
for each selected audio stream. Copy source is the initial setting. Conversion
supports preserving the source channel count or choosing mono or stereo; track
titles, languages, and dispositions are retained. The displayed source codec,
channel count, and sample rate continue to describe the input.

Audio settings apply to the original source stream index, even when selected
tracks are reordered for muxing. Standalone x264, SVT-AV1, SVT-AV1 5fish, and
SVT-AV1-HDR share this audio stage. av1an remains copy-only, including batch jobs.
Video compatibility requirements for each encoder still apply.

Drafts remain separate for each source, video encoder, and workflow. Deselecting
an audio track removes it from the request without erasing its draft. Batch jobs
keep per-file audio settings, require a new preview after edits, and retain the
reviewed settings in immutable queue snapshots and saved history. Older history
without audio settings retains the selected-track copy behavior.

## Codec and tool requirements

Audio conversion requires a matching FFmpeg and FFprobe 8.1 or newer pair with
the requested audio encoders. A behavioral preflight exercises AAC priming in
Matroska with both tools before encoding the source. An incompatible pair is
rejected with an actionable error; copying audio remains available. Checking
only the executable version or whether an AAC encoder is listed is insufficient
to establish correct decoded timing.

Opus uses FFmpeg's `libopus` encoder and produces 48,000 Hz audio. AAC uses the
native AAC-LC encoder and retains a supported source sample rate. Conversion
requires a known sample rate and a source channel count from one to eight.
Multichannel conversion requires a recognized speaker layout matching that
count. Opus cannot preserve `4.0`, `5.0(side)`, or `5.1(side)` layouts; the error
directs the user to mono, stereo, AAC, or copy.

Bitrates are whole numbers in kb/s, starting at 32. The maximum depends on the
selected codec and output channels:

| Mode                                         | Maximum bitrate                                                         |
| -------------------------------------------- | ----------------------------------------------------------------------- |
| Opus mono, including a preserved mono source | 256 kb/s                                                                |
| Opus stereo or supported multichannel output | 512 kb/s                                                                |
| AAC                                          | `min(512, floor(source sample rate × output channels × 6 / 1000))` kb/s |

For example, AAC mono at 8,000 Hz is limited to 48 kb/s; at 44,100 Hz it is
limited to 264 kb/s. Choosing a codec or channel count reduces an existing
bitrate when necessary to fit the displayed maximum. A typed value outside the
range blocks submission. Copy does not expose bitrate or channel conversion
controls.

## Decoded timing and publication checks

Converted audio is validated using decoded samples after codec delay and
pre-skip, rather than assuming that encoded packet timestamps equal the audible
start. Complete source and output audio scans check sample rate, channel count,
speaker layout, sample count, continuity, and decoded starting time.

The source scan permits bounded container timestamp quantization: the larger of
2 ms or three source timebase ticks plus one sample, with each tick capped at
1 ms. Larger gaps, overlaps, and changing sample rates are rejected. Converted
audio uses `asetpts=N/SR/TB+STARTPTS` to follow its decoded sample clock while
preserving its first decoded timestamp. This does not insert silence or drop
samples to hide discontinuities. Opus sample-rate conversion remains explicit.

Output continuity retains the stricter 2 ms bound, and the decoded start must
remain within 2 ms of the source. Sample-count validation accounts for rate
conversion, rounding, and AAC's final partial frame. The application also
validates every decoded video frame and its timing, selected tracks, and
metadata before publishing the output without overwriting an existing file.

## Real-media excerpt

A 1920 × 1080 source excerpt contained 236 video frames and 27 tracks: video,
44,100 Hz stereo AAC, ASS subtitles, and 24 font attachments. Its source audio
decoded to 433,152 samples per channel.

| Completed output                | Audio result                                          |
| ------------------------------- | ----------------------------------------------------- |
| Standalone x264 + AAC           | 44,100 Hz stereo; 433,152 decoded samples per channel |
| Standalone x264 + Opus          | 48,000 Hz stereo; 471,458 decoded samples per channel |
| Standalone SVT-AV1 5fish + Opus | 48,000 Hz stereo; 471,458 decoded samples per channel |

The Opus count matches the source count scaled to 48,000 Hz and rounded to the
nearest sample; reported residual padding rounds to zero samples.

Independent checks of all three outputs verified:

- Every decoded video frame and its presentation timestamp.
- All subtitle packet payload hashes and timestamps.
- The hashes of all 24 font attachments and retained metadata.
- Decoded audio sample counts, rate, and timing.
- Waveform alignment by correlation at the beginning, middle, and end.

These are short-excerpt results. They do not establish full-episode or packaged
application qualification.

## Automated gates

The following local checks passed:

- All 130 frontend tests, including per-track codec defaults, bitrate limits,
  source and workflow isolation, track selection, stale batch previews,
  immutable queued settings, saved history, and existing remux behavior.
- Svelte checking with zero errors and zero warnings.
- Workspace tests: 6 core tests, 118 runtime tests, and 1 ordinary integration
  test.
- Clippy across the whole workspace.
- All 8 opt-in native `audio_jobs` integration tests.

The native audio gate covers mixed copied/AAC/Opus tracks and reordered source
indices; channel conversion and resampling; per-file batch settings and rejected
tampered requests; cancellation during finalization; low-rate AAC final-frame
padding; rejection of negative container timelines; bounded authored timestamp
jitter without sample loss; and both-tool AAC priming preflight.

Run the native audio gate with the required encoders and a compatible FFmpeg and
FFprobe pair on PATH:

```sh
cargo test -p media-runtime --test audio_jobs --locked -- --include-ignored
```

The audio implementation and native fixtures are in
[`audio.rs`](../../crates/media-runtime/src/jobs/audio.rs) and
[`audio_jobs.rs`](../../crates/media-runtime/tests/audio_jobs.rs).

## Qualification still in progress

- **Full episode — pending:** complete the approximately 23-minute real source,
  then record final decoded-video and audio results and independent timing,
  subtitle, attachment, and metadata checks.
- **Native interface — pending:** record the rebuilt desktop application's
  import, per-track audio controls, submission, completed history, cancellation,
  and output verification results.
- **CI — pending:** record the final remote run after the completed change is
  published.

Installer, bundled-tool, and clean-machine qualification are not established by
the checks above.
