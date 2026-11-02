# Source utility packet and measured-size qualification

Registered image sequences now reach Read Bitrates and Bitrate Chart. Analysis
builds a private owned concat manifest from the receipt's original image order,
reads each original compressed-image packet, and retains the registration lease
and complete original/cache guards until the scan joins. It does not report the
unrelated packet sizes of the FFV1 import cache. Selected stream indices remain
explicit; a nonexistent stream fails instead of selecting another stream.
Cancellation joins the probe and removes the owned manifest before releasing
registration. Original mutation is rejected.

This route intentionally corrects a fork bug. `FfmpegUtils.CreateConcatFile`
(`463–495`) emits one file line per image with no duration, rate or sentinel.
`UtilReadBitrates.cs:31–42` passes that manifest to the remux-based size helper;
`UtilPlotBitrate.cs:28–30` passes it directly to FFprobe. With the qualified
provider, the literal file-only PNG manifest emits invalid approximately
−9.22e12-second timestamps; the direct chart call also omits the concat demuxer
arguments needed by the headerless manifest. The source remux succeeds and
retains every original compressed-image packet, but repairs their clocks into
repeated timestamps. Those broken clocks are not preserved.

The corrected analysis manifest uses the registered import FPS, one packet per
original and no repeated final image. It supplies that image demuxer rate and
explicit durations. Each duration is the difference between rounded absolute
microsecond boundaries, so fractional periods do not accumulate rounding drift.
A non-25 rate and 24000/1001 are exercised. Regular encoded-file analysis and
all encoding/import settings remain unchanged. No new control is exposed.
The literal source failure and successful remux packet receipt are retained in
`target/test-artifacts/native-backlog/sequence-literal-source.json` and
`sequence-source-comparison.json`.

`GrainTableResult.sizeBytes` is an optional decimal string for old-receipt
compatibility; every new measured/extracted table result supplies actual
published table bytes. A separately retained denoised reference is excluded,
matching `UtilFilmGrain.cs:250,273,320`. `CrfLadderResult.samplesSizeBytes` is
present only after successfully retaining the samples directory. It counts all
files recursively, regardless of extension, including lossless reference cuts,
encoded rungs and retained scorer files (`UtilCrfLadder.cs:428–431`,
`RunTask.cs:121`, `IoUtils.cs:798–823`). Unretained work has no measured output
receipt. The source completion log/status and batch totals consume these values;
projections are not substituted. OCR has no source ReportOutput and receives no
invented size field.

Focused evidence is under `target/test-artifacts/native-backlog/`:

- `sequence-receipts-real.log`: actual ordered PNG packet totals/bins, requested
  rate/fractional chart clock, stream selection, cancellation/lease cleanup,
  original mutation, grain table-only bytes, retained/unretained ladder bytes,
  and successful table batch receipts.
- `retained-directory-size.log`: every extension and nested-file total, absent
  retention and cancellation.
- `utility-receipts-browser.log`: measured table/retained-directory output
  summaries, absence of a private-ladder size, and successful table batch totals.
- `utility-receipts-contracts.log` and `utility-receipts-svelte.log`: generated
  DTOs and focused frontend diagnostics.

These are local Linux execution and mocked UI receipts. Native Windows/macOS
execution is separate.
