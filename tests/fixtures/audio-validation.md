# Audio conversion validation

Development qualification on Windows on 2026-09-12 covers per-track audio
conversion in the standalone encoding workflow. The local automated gates,
short-excerpt checks, full-episode runtime qualification, native-interface
completion and restart checks, and independent verification of the native
interface's full-episode output below have passed. Windows and Linux CI also
passed on the implementation commit recorded below.

## Supported behavior

Quick Convert and batch encoding offer Copy source, Opus, and AAC independently
for each selected audio stream. Copy source is the initial setting. Conversion
supports preserving the source channel count or choosing mono or stereo; track
titles, languages, and dispositions are retained. The displayed source codec,
channel count, and sample rate continue to describe the input.

Audio settings apply to the original source stream index, even when selected
tracks are reordered for muxing. Standalone x264, SVT-AV1, SVT-AV1 5fish, and
SVT-AV1-HDR share this audio stage. av1an was copy-only at this checkpoint;
the subsequent [av1an framing and audio qualification](av1an-framing-audio-validation.md)
extends that shared stage to av1an and its batch jobs.
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

These excerpt results supplement the full-episode runtime checks below.

## Full-episode runtime qualification

A complete 1280 × 720 episode, approximately 23 minutes 40 seconds long,
completed with standalone SVT-AV1 5fish at CRF 30, preset 8, lineart bias 5,
texture bias 4, and grain synthesis off. Audio used Opus at 128 kb/s with the
source channel count preserved. The complete job took 414.279 seconds, including
validation.

Independent output checks established:

- All 34,047 decoded video frames were present, and every presentation timestamp
  matched the source exactly.
- Source audio contained 62,626,816 decoded samples per channel at 44,100 Hz;
  output audio contained 68,165,242 at 48,000 Hz. Both decoded timelines started
  at zero. Their decoded-duration difference was 0.001984 ms.
- All 353 ASS subtitle packet hashes and timestamps matched, as did the hashes
  of all 24 font attachments.
- The source file hash remained unchanged.

PCM waveform correlation at three positions independently checked audio
alignment:

| Position  | Correlation | Measured lag |
| --------- | ----------- | ------------ |
| Beginning | 0.99814     | −0.104 ms    |
| Middle    | 0.99268     | +0.146 ms    |
| End       | 0.99890     | +0.292 ms    |

## Native interface qualification

The rebuilt desktop application imported the full episode and initially offered
Copy source for its audio. Selecting 5fish showed CRF 18, preset 2, lineart bias
5, and texture bias 4. The native form was then set to CRF 30, preset 8, and Opus
128 kb/s with preserved channels and a new output destination. That full-episode
job reached Succeeded. Its saved request retained all 27 selected tracks,
5fish CRF 30, preset 8, grain synthesis off, lineart bias 5, texture bias 4,
and Opus 128 kb/s with preserved channels.

Independent verification of that native output passed. The 265,308,053-byte
file contained all 34,047 video frames with a maximum presentation-time error
of 0 ms. Audio again converted 62,626,816 samples per channel at 44,100 Hz into
68,165,242 at 48,000 Hz, starting at zero with a duration difference of
0.001984 ms. All 353 subtitle packet hashes and timestamps and all 24 font
hashes were unchanged. The beginning, middle, and end waveform correlations
and measured lags exactly matched the full-episode runtime results above.

A separate draft using AAC at 128 kb/s with preserved channels and another new
destination was added to the queue. The interface displayed its saved AAC
request. Canceling only that queued job succeeded while the full-episode Opus
job continued running.

A fresh AAC job was also started and canceled during source-video validation,
after approximately ten minutes of the 23-minute source had been scanned. It
reached Canceled without an error, published output, owned temporary files, or
remaining media child processes. This native-interface check covers active
cancellation during preflight; cancellation during audio finalization is
covered separately by the native integration gate.

The application was closed and restarted into a new process. The interface
showed zero pending jobs, the full Opus job still marked Succeeded, and both the
queued and active AAC jobs still marked Canceled. Expanded history retained the
27-track selection and all saved video and Opus settings. All 17 job records
were present: the 14 existing records plus the successful Opus job and two
canceled AAC jobs. Existing records retained their meaning, with missing audio
settings normalized to the empty-array default. Final source hash checks
matched their recorded originals. The application was closed after
verification; neither canceled job had a published output, and no owned
temporary files or media child processes remained.

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

## Continuous integration

Implementation commit
[`ebdf944`](https://github.com/jkkma/jesses/commit/ebdf94477eeacd51a2e202699b9bb14542ca7b23)
passed all three jobs in
[run 34686964988](https://github.com/jkkma/jesses/actions/runs/34686964988):
frontend, Windows x64 desktop, and Linux x64 desktop. Both native executables
built with the embedded frontend, and contract, formatting, lint, and workspace
checks passed.

Linux also passed the existing real-tool and x264 gates, rejected the older
FFmpeg/FFprobe 6.1.1 pair before video encoding, built the SHA-256-pinned official
FFmpeg 9.0.1 source, passed all eight audio integration tests with that pair, and
passed the standalone 5fish/HDR gate. The source build includes x264, x265, Opus,
and dav1d; its exact configuration and dependency versions form the cache key.

Installer, bundled-tool, and clean-machine qualification are not established by
the checks above.
