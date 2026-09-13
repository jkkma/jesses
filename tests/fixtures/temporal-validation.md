# Frame processing validation

Quick Convert and each Batch input have explicit BWDIF deinterlacing, rational
output frame rate, and resize-kernel settings. Existing requests preserve their
progressive source rate and Lanczos resize behavior. Each workflow and source
keeps an independent draft; queued requests are immutable.

The complete source is decoded and checked before processing. BWDIF requires
uniformly interlaced frames with the selected top-first or bottom-first order.
The source header is advisory; every decoded frame must agree. Unknown field
order, mixed progressive/interlaced material, changing dimensions/SAR, variable
cadence and timestamp gaps fail before an output is published. Output frames
must be progressive, square-pixel and retain the expected color properties.

The filter order is source-frame trim, BWDIF, optional duplicate/drop frame-rate
conversion, tone mapping, then the existing bitmap/crop/resize/text/border order.
Single-rate BWDIF retains the source frame count. Bob emits one frame per field
and doubles the rate unless an explicit output rate follows it. Audio, subtitle
cues and chapters stay on the original trimmed timeline. Output video duration
is rounded to the nearest whole frame, with an explicit rational rate from 1
through 120 fps. This can differ from the unchanged audio end by at most half
an output frame. Frame counts use checked integer arithmetic.

Frame-rate conversion duplicates or drops existing images. It does not change
playback speed, interpolate motion, repair telecine/cadence, or relax source
timestamp validation. BWDIF is a separate algorithm from QTGMC; this work does
not qualify QTGMC, Detelecine or cadence-repair workflows. Deinterlacing and rate
conversion require standalone encoding until av1an's probes and chunks can be
qualified with the same temporal processing. Resize-kernel selection also works
with the existing av1an geometry path; only standalone kernels were exercised
by the new actual-tool gate.

Nearest neighbor, bilinear, bicubic and Lanczos apply when the framing width
changes. Original source dimensions remain separate from processed dimensions.
Non-square source pixels still receive the existing actionable constraint;
arbitrary SAR/DAR conversion is not included in this slice.

## Automated evidence

On Windows with FFmpeg/FFprobe 9.0.1 and standalone x264:

- `cargo test -p media-runtime --test temporal_jobs --locked -- --ignored
--nocapture`: four actual-tool gates passed in 28.22 seconds. TFF and BFF
  sources are exercised in single-rate and bob modes. Every retained luma field
  row matches its original source exactly after lossless encoding; wrong field
  order fails without a destination and source SHA-256 stays unchanged.
- The trim gate selects source frames 12–36 before bob, verifies 48 output frames,
  and compares every retained field to its source frame and parity.
- Rates 12, 30 and `30000/1001` produce the expected full output frame counts in
  MP4. Each decoded image is an unchanged source image, order is monotonic and
  displacement is bounded by one source interval. All 96,000 decoded audio
  samples match the original byte for byte.
- All four resize kernels produce distinct impulse responses. Every nearest
  neighbor output luma sample matches the independently calculated source
  coordinate. No FFmpeg reference filter is used for that expected result.
- All 26 Plan checks pass, including rational count/bounds, bob-before-FPS order,
  and integer timestamps. A real `30000/1001` regression exposed floating-point
  timestamp truncation; rational time bases and integer frame counters prevent
  the duplicated first timestamp.
- Two browser regressions pass: workflow drafts/explicit rational requests, and
  Batch review invalidation with prior queued settings unchanged. The successful
  run used a held Vite server after unrelated server lifecycle/navigation failures.
- All-target Clippy with warnings denied passes. `pnpm check` reports zero errors
  and warnings. Controls use explicit accessible labels and a fieldset legend.

The Linux CI workflow includes the actual-tool gate. These local results do not
claim a Linux execution or native desktop UI qualification.

## Original-media evidence

A read-only 1280×720 episode with 34,047 source frames passed its complete source
scan. Source frames 1200–1680 then became 601 progressive frames at 30 fps, with
bicubic resizing to 960×540. Independent full-frame inspection measured maximum
Matroska clock error of 0.334 milliseconds and retained 1:1 sample aspect ratio.
All 882,883 stereo audio samples per channel remain at 44.1 kHz; seven English
subtitle cues retain their language and title. The source SHA-256, byte count and
modification time match the earlier source baseline in this qualification run.

The output contains 7,394,092 bytes and has SHA-256
`47b2d1018f2448213d54b12d08feb14ba27a191b92d0a4b5f43a5318fe57e2a3`.
The local receipt contains the immutable request, runtime log, independent probe,
subtitle export and frame/audio/source-integrity calculation. Source media,
outputs and personal paths are not committed.

## Native frame-rate conversion

The rebuilt Windows application also encoded a three-track, 480-frame fixture
derived by stream-copy from the earlier native episode output. Controls selected
`30000/1000` fps and bicubic 960x540 alongside x264 250 kb/s/two-pass, `ref=3`,
`bframes=4`, AAC 128 kb/s and MP4. The saved job preserves that exact request;
both encoder passes completed 601 frames. An independent full decode verifies
all 601 progressive SDR 8-bit frames and exact presentation times `frameIndex/30`
using the MP4 stream's `1/30000` time base. No HDR side data remains.

Audio starts at zero and contains 882,874 stereo samples per channel, nine fewer
than the source's 882,883. Effective audio end and decoded frame end stay within
one source clock tick plus sample-rounding bounds (47 samples). The largest
adjacent timestamp residual is seven samples and the largest cumulative residual
is ten samples, both below the source's 1 ms clock tick. Every subtitle cue time,
readable character and line break matches the five selected source cues; font
face is a documented MP4 conversion loss. Source hashes, lengths and modification
times remain unchanged. `final-validation.json` and the actual native job/pass
logs are retained under `Videos/Jesses-final-native-20260913`; the output hash is
`43ebc25b049d7d55a888aea6b5d822a731edcfaa3667b3c46c93b0cdf7fe16d0`.

The nearby video summary now displays the configured rational output rate,
an invalid-rate hint, or doubled source rate for BWDIF bob as appropriate. Four
focused container/temporal browser cases pass after this display correction;
Svelte reports zero errors and warnings. Native deinterlacing and Linux/final
package qualification remain separate from this native frame-rate conversion.
