# Streaming frame validation

Jesses validates FFprobe's decoded frame metadata as it arrives. Source and output
scans use the same constant-memory validator. The declared cadence and the one
supported decimal alternative are checked together; every accepted frame still
has to meet the existing timestamp, geometry, chroma, color, and HDR rules.

The scanner accepts complete JSON only. Malformed/truncated documents, duplicate
frame arrays, trailing data, and oversized frame records fail explicitly. A parse
failure immediately stops the process tree. A completed parse still requires a
successful decoder exit and empty error output before any result is accepted.

Buffering is bounded independently of the number of frames: stdout crosses a
two-chunk channel with 64 KiB chunks, one frame record is limited to 1 MiB, and
stderr retains a bounded tail. Decoders also allocate their own codec working
memory. The 24-hour scan timeout is an execution safeguard, not an estimate.

The selected video stream uses up to eight decoder threads, capped by available
logical processors. FFprobe supports indexed codec options such as
`-threads:0 8`; see the [FFprobe documentation](https://ffmpeg.org/ffprobe.html).
Source scan progress resets before encoding. During finalization, muxing leaves
scan progress indeterminate until decoded-output validation starts at zero.
The interface estimates the current phase's speed and remaining time from a
rolling 30-second observation window after a five-second warmup. A job/phase
change, missing progress, or progress regression resets the estimate; 15 seconds
without an update replaces it with an unavailable message. It is not a whole-job ETA.

## Repeat a complete source scan without encoding

The manual qualification test uses the same probe, plan, and streaming frame scan
as encoding, but creates no encoded output. Run it separately from ordinary CI:

```powershell
$env:JESSES_VALIDATION_INPUT = 'C:\absolute\path\source.mkv'
$env:JESSES_HDR10_FALLBACK = 'false'
cargo test -p media-runtime --lib validates_external_source_without_encoding -- --ignored --nocapture
```

Set `JESSES_HDR10_FALLBACK` to `true` only when explicitly qualifying an HDR10 base
layer from supported Dolby Vision/HDR10+ input. If the input environment variable
is absent, the manual test prints that qualification was not run; its empty run
is not evidence about real media. The synthetic and integration gates remain
separate and require their stated tools.

## Local Windows results

Qualification on 2026-09-11 uses FFmpeg/FFprobe 9.0.1 and the installed Rust 1.98.0
toolchain. User-owned videos and generated outputs remain outside the repository.
The repository remains pinned to Rust 1.98.1 for CI.

The complete 1080p SDR anime episode passed: 34,047 frames, with its declared
24000/1001 cadence reconciled to the verified 2997/125 timestamp cadence. Decoded
frame validation took 56.1 seconds; the complete test including packet inspection
took 58.7 seconds.

The complete 720p version also passed all 34,047 frames at the verified 2997/125
cadence. Frame validation took 33.1 seconds (34.3 seconds including packet
inspection).

The complete 75.8 GB, 2160p movie passed all 159,123 frames at its declared
24000/1001 cadence with explicit HDR10 fallback. Decoded-frame validation took
2,563.0 seconds (42.7 minutes); the complete qualification including initial
inspection took 2,613.8 seconds. This is a complete source scan, not a full-film
encode. Repeated process samples showed the validator around 22 MiB with an
observed peak of 22.3 MiB; FFprobe's decoder had an observed peak of 635.3 MiB.
The source sizes and modification timestamps remained unchanged.

Threaded and default-decoder frame JSON was byte-identical on the 236-frame 1080p
anime excerpt and 114-frame 2160p HDR excerpt. The HDR excerpt scan took 8.28 seconds
with default decoder settings and 2.02 seconds with eight threads. These are local
measurements, not performance guarantees.

Native folder import loaded both full anime resolutions and the 75.8 GB movie,
including stream inspection. A native standalone encode at CRF 30/preset 13/grain
0 preserved all 236 excerpt frames, AAC audio, ASS subtitles, and 24 font
attachments. The rebuilt app displayed advancing source-validation progress on
the complete episode; canceling that scan stopped its FFprobe process and left
no destination file.
The final native build also displayed an estimated scan speed and current-phase
remaining time after warmup on the full episode. Cancel reached its terminal
state, the matching decoder process was gone, and the destination did not exist.

An av1an encode of the 114-frame 2160p excerpt at CRF 30/preset 10/grain 8 with
explicit HDR10 fallback passed the production source/output validators. Separate
FFprobe inspection confirmed 10-bit AV1, limited-range BT.2020/PQ, mastering data,
and exact MaxCLL 200/MaxFALL 142. Exporting decoder side data with
`-export_side_data +film_grain` confirmed grain parameters on all five sampled
frames. A normal decoded-frame probe does not export grain parameters by default.
A matched frame from the source and output was also inspected using the same SDR
tone mapping: framing and color appearance aligned, with no obvious corruption.
This spot check does not qualify HDR monitor playback or whole-video visual quality.

The dedicated av1an tab also completed a native encode of that HDR excerpt with
two workers and the same quality/grain/fallback settings. Its saved job used the
av1an backend, and independent inspection confirmed all 114 frames, HDR10, film
grain on all five sampled frames, and the four selected copied tracks. Quick
Convert retained its own standalone settings after switching tabs. Browser
regressions cover fixed backends, per-source draft restoration, separate reset
and destination behavior, shared queue/history, and delayed picker/submission
replies after a source change or reset.

Automated gates passed: 95 workspace unit tests; 80 Chromium frontend tests; 24
encode tests with real-tool cases included; and both av1an integration tests.
The latter include active-worker cancellation. The streaming tests cover an
80 MiB lossless process stream, a greater-than-78 MiB frame document, backpressure,
parser/decoder failures, late metadata changes, cancellation, timeout, and dropped
future cleanup. Build, typecheck, formatting, Clippy with warnings denied, and
IPC contract checks passed. Linux native runtime qualification remains pending.
