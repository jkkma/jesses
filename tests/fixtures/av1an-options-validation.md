# av1an scene controls and perceptual targeting

Windows runtime and browser qualification on 2026-09-13. The optional
`av1anOptions` object belongs to av1an single/batch requests. Omitted settings keep
existing 240-frame maximum, 24-frame minimum, standard scene detection at height
360, L-SMASH decoding, longest-first order, and constant-quality behavior.
Source and encoder drafts restore these settings; reviewed batch requests are
invalidated whenever any setting changes.

## Scene and reader contract

The source-reader choices are L-SMASH Works, FFMS2, BestSource, FFmpeg select,
and hybrid segmentation. The first three require the named VapourSynth plugin,
checked from the selected engine's actual version result. The corrected FFmpeg
source readers require an engine advertising `ffmpeg9-passthrough-v1`.

Scene detection supports standard/fast detectors and explicit scene downscale
height (even 64–4320), or original resolution. Fixed chunking skips detection.
Maximum chunk length is 0–100000 frames (0 removes the limit); minimum scene length
is 1–100000 and cannot exceed an enabled maximum. Extra splitting may distribute
frames into shorter balanced chunks. Ordering is longest-first, shortest-first,
source order, or random. Jobs still encode the first video stream only.

The upstream select generator in the previously qualified engine kept absolute
selected timestamps. With FFmpeg 9 its second test chunk produced 205 frames when
103 were expected; the existing frame guard rejected it. The source build at
upstream 805dad69143fa0a81cfe2fb89c0b9e90a828ea72 adds `setpts=PTS-STARTPTS` after
select and `-fps_mode passthrough` before Y4M output. A direct 103-frame pipe
matches an independent trim [103,206) pixel hash exactly. This upstream revision
also replaces the removed `-vsync 0` option in sampled probe pipes. The retained
compatibility patch/version marker makes both requirements inspectable.

## Target and probe contract

Quality targeting specifies an ordered range 0–100 in 0.1 increments, CRF bounds 1–63,
1–10 probes per chunk, every 1–4 frames, and even evaluation dimensions 128–8192.
The additive `metric` defaults to VMAF when omitted in an old saved target.

| Metric              | Direction               | Actual dependency check                                        |
| ------------------- | ----------------------- | -------------------------------------------------------------- |
| VMAF                | Higher is better        | Selected FFmpeg evaluates libvmaf vmaf_v0.6.1                  |
| SSIMULACRA2         | Higher is better        | VapourSynth evaluates vszip or Vship                           |
| Butteraugli INF     | Lower is better         | VapourSynth evaluates Julek or Vship at 203-nit intensity      |
| XPSNR minimum Y/U/V | Higher is better, in dB | Every frame uses FFmpeg XPSNR; sampling uses vszip R7 or newer |

All four use explicit mean aggregation. SSIMULACRA2, Butteraugli, and sampled
XPSNR require a VapourSynth source reader. Every-frame XPSNR can use select or
hybrid. A fixed application-owned two-frame script exercises the exact plugin
function and requires finite values; its process uses the same selected child
environment as av1an and its reserved file is removed afterward. Julek requires
the `julek-butteraugli-v1` marker because upstream called the case-sensitive
function `butteraugli`, while the real API exports `Butteraugli`. The marked
engine changes only the Julek invocation; the Vship branch is unchanged.

Higher metric values are not treated uniformly: the engine internally reverses
Butteraugli for searching and displays ordinary positive distances afterward.
The form explicitly states that lower is better and keeps the entered bounds
in ascending order. More than one-frame sampling requires
the corrected-engine marker when using the FFmpeg 9-compatible workflow.

Every probe uses the selected SVT implementation and the same preset, color,
grain, and explicit encoder parameters as the final encode. Probe encoder
arguments are passed explicitly, rather than the engine's `copy` shortcut,
so probes are never substituted for the final filtered encode. The engine may
choose the closest available CRF when the target range is unreachable within
its probe count or CRF bounds; these are search targets, not output guarantees.
Target jobs retain the engine debug detail log with per-chunk probe scores and
chosen CRF so the result of the search can be inspected.

This engine scores the source before final crop, resize, or borders. That
limitation appears in both the settings and job log. Subsampling scores only
sampled frames, and target probe scores do not measure the final output. Preserved
HDR/PQ targeting is rejected because these metric pipelines have not been qualified
for that signal; HDR encoding retains CRF control.

## Recovery and source identity

Saved settings, selected tools, final/probe encoder parameters, and engine version
are immutable. Strict queue parsing checks exact reader command arguments, global
frame coverage, configured chunk maximum, target fields, probe parameters, and
completed chunk counts. VapourSynth scripts must match the qualified executable
Python template after only validated source/cache/reader assignments are replaced.
Both qualified upstream revisions have the same normalized template SHA256.

Hybrid queue entries use local segment frame ranges. Recovery checks the exact
ordered owned segment names and their offsets against the global scene list,
fingerprints and locks each segment, and independently decodes the concatenated
segment sequence and original video to compare complete pixel SHA256 values.
This check runs before reuse or publication, catching substituted/reordered
segments even if a forged manifest supplies replacement file hashes. It requires
additional source decoding. Windows concat verification uses an owned relative
list filename from the guarded workspace to avoid extended-path URL resolution.

## Retained checks

The earlier av1an VMAF and every-frame XPSNR runs below qualify encoded output,
audio, source integrity and job recovery, but their numeric target qualification
is **superseded** by the FFmpeg matrix correction described below. Score ranges
in those requests are configured targets, not proof that the engine measured the
correct reference pixels. The independent final-output quality analyzer is a
separate pipeline and is not covered by this engine defect.

- Eight recovery receipt unit cases, including altered source scripts, commands,
  frame ranges, reader caches, target ranges, probe arguments, and unknown fields.
- Real FFMS2 and BestSource stop/reopen/resume completed the full 720-frame fixture
  with crop/resize/borders, Opus, subtitles, attachment bytes, timestamps, and
  unchanged source. Completed chunk hashes were preserved across resume.
- Real 5fish VMAF 92–99, CRF 20–42, three probes, sampling 1, fast scenes and framing:
  full 720 frames, Opus timeline, metadata and recovery passed in 97.88 seconds.
- Mainline SVT and HDR SVT on SDR both passed sampling 2 VMAF target stop/reopen/
  resume with the corrected source-built engine, in 129.32 seconds together.
- The corrected select and multi-segment hybrid reader gate passed in 66.46 seconds.
  It covers stop/reopen/resume, forged segment content with replacement hashes,
  exact 720 frames, Opus, subtitles/attachment bytes, border pixels, and source
  integrity. The forged segment passes a file digest check but is rejected by
  the independent complete decoded-source comparison.
- Real 720p episode excerpt source frames [2880,3360), nominal 120.12–140.14 seconds:
  hybrid, VMAF 90–97, CRF 18–42, three probes, sampling 2, 5fish preset 8, resize 960
  plus 16-pixel borders, and Opus 128 kb/s completed 480 frames/20.02 seconds. The
  copied excerpt remains byte-identical; this is not a full-episode targeting run.
  Independent output inspection confirms 10-bit 992×572 pixels, 0.500 ms maximum
  timestamp error, 961013 decoded Opus samples per channel, 0.9968427 correlation
  to the source resampled at 48 kHz, and exact 64 luma in the 10-bit black border.
  Output size 1,264,749 bytes. A 10-second PNG was inspected; the image and borders
  match the intended geometry.
- The four additional metric cases (SSIMULACRA2, Butteraugli INF, XPSNR every
  frame with select, and XPSNR sampled with L-SMASH) completed all 720 frames
  through stop/reopen/resume with framing, Opus, copied subtitles/attachments,
  unchanged source, and immutable final/probe parameters `enable-tf=0` and
  `aq-mode=2`. The complete loop passed in 286.71 seconds. Its engine detail logs
  contain actual probe scores and final CRF/score entries. The first attempt
  caught an incorrect application-side XPSNR log marker; the actual filter
  succeeded, the marker was fixed, and the complete loop then passed.
- A second real 720p episode excerpt run used Butteraugli INF 0.8–1.5, L-SMASH,
  the same framing/Opus and explicit advanced parameters. It published 480 frames,
  1,766,608 bytes, 992×572 10-bit, maximum timestamp error 0.500 ms, 961013 Opus
  samples/channel, source audio correlation 0.9968427, and exact 64 black luma.
  Its source SHA256, size and modification time remained unchanged, and a
  10-second PNG was inspected. One chunk improved from CRF 30 / score 1.60 to
  CRF 24 / score 1.39. A harder chunk exhausted its three probes at CRF 18 /
  score 2.03, illustrating the documented unreachable-target behavior.
- Three browser cases validate ranges, queued immutable metrics, lower-is-better
  guidance, source-reader dependencies, restored drafts, and invalidated batch
  review. All passed with a file-logged held Vite server. Cold startup took about
  95 seconds before the actual cases, which each finished in 1.5–2.2 seconds.
- 26 av1an unit checks passed with two owned subprocess helpers ignored; the core
  regression proves old saved targets still deserialize to VMAF. All-target
  Clippy and Svelte check passed.

The Windows runs above use selected source-built FFmpeg/FFprobe and an explicit
external VapourSynth installation. The separate portable checks below exercise
the bundled frameserver. Linux execution is deferred.

The additional metric engine is the immutable build whose SHA256 is
`83d506d737f46341ceec04d78787210fc793fbaad2b099af0bbbfd16a6dbe874`.
It advertises both `ffmpeg9-passthrough-v1` and `julek-butteraugli-v1`.
CPU vszip and Julek were exercised on this host; GPU Vship branches were not
qualified on hardware here. Missing scorer dependencies are reported before
encoding rather than silently selecting another metric.

## Source-built portable CPU scorers

The Windows recipe builds vszip 22.1.0 at `beb7a0ab` with Zig 0.16.0 and Julek r3
at `3f2780d2` with static JPEG XL, Highway, Brotli and skcms dependencies. All
twelve pinned input archives, complete dependency sources, licenses and build
recipes are retained in the scorer delivery. vszip's two Zig dependencies use
verified local source trees during compilation. Julek imports only KERNEL32;
vszip imports Windows CRT API sets, KERNEL32, ntdll and CRYPT32. No separately
installed compiler runtime or scorer DLL is required.

Direct calls through the source-built R79 runtime produced finite SSIMULACRA2
98.31991206147393, Butteraugli INF 8.506763458251953 and XPSNR Y/U/V
40.330478561391786 on the owned two-frame diagnostic pair. The complete runtime
SHA256 inventory was unchanged after execution.

The portable runtime exposed an upstream probe-reader defect: L-SMASH's
`prefer_hw=3` failed to open an encoded AV1 probe while `prefer_hw=0` decoded the
same 48 frames successfully. Earlier external installations had a working FFMS2
reader ahead of this fallback. The source-built engine now pins software decoding
for L-SMASH metric probes and advertises `lsmash-software-probes-v1`. Its SHA256 is
`db8a56645fe1ed6f2685542855acf820244b141c866cc056f468908f1c358046`.
Preflight rejects an older engine when a VapourSynth metric has only L-SMASH
available. A focused regression verifies both rejected and accepted versions.

A real 720p excerpt run used this merged seven-tool bundle with PATH restricted
to Windows System32 and deliberately invalid inherited VapourSynth/Python paths.
The request covered original source frames [2880,3360), nominal 120.12–140.14
seconds, L-SMASH, SSIMULACRA2 75–90, CRF 18–42, three probes, sampling 2, 960×540
evaluation, 5fish preset 8, 960-pixel resize, 16-pixel borders and Opus 128 kb/s.
Final and probe parameters both included `enable-tf=0` and `aq-mode=2`.

The result contains 480 frames at 992×572, 10-bit, 1,180,970 bytes, maximum
timestamp error 0.500 ms, 961013 Opus samples/channel, source audio correlation
0.9968427, and exact 64 luma in the black corner. The source SHA256, length and
modification time stayed identical; all 53 bundled runtime files were unchanged.
A 10-second PNG was inspected and has the intended picture and borders. Retained
probe results include CRF 30 / score 90.78 followed by CRF 36.25 / score 88.42.
This qualifies the portable CPU scorer on a bounded excerpt, not a full episode.

## FFmpeg metric reference correction

Retaining and reviewing actual per-chunk scores exposed a second upstream defect
in FFmpeg 9 targeting. Y4M carries the reference pixels without color-matrix tags,
while the encoded probe declares BT.709. FFmpeg automatically inserted a
YUV-to-RGB-to-YUV conversion into the reference path to reconcile those tags.
The encoded output remained valid, but the resulting target score was wrong.
Both every-frame XPSNR and av1an's ordinary VMAF path were affected; sampled
VapourSynth XPSNR, SSIMULACRA2 and Butteraugli use separate comparison paths.

The source patch prepends `setparams=colorspace=unknown` to both metric inputs
before their existing scale filters. It changes only matrix negotiation tags,
preserving the actual samples and range. On the owned 48-frame diagnostic,
every-frame XPSNR changed from Y/U/V 9.4399/15.2874/18.9996 to
43.9798/39.3546/39.8942. The corrected pipe's complete statistics file, including
every frame and aggregate value, equals an independent comparison against the
original tagged file byte for byte. VMAF changed from 48.544283 to 98.975676;
all per-frame and pooled JSON metrics equal the tagged-file comparison exactly.

Jesses requires the `ffmpeg-metric-matrix-v1` engine marker for VMAF and
every-frame XPSNR. The actual recovery fixtures now require finite scores and
explicit lower bounds that reject the observed corrupt-reference pattern. The
older portable stage completed all nine lifecycle/recovery tests with unchanged
runtime files, but that result does not qualify these two numeric modes.

The corrected engine has SHA256
`2f28430da17604523f539ba87a733459a19f9d16784e2e13bfefc18dda91b7a6`.
Two additional real 480-frame excerpt requests used the final seven-tool bundle
with restricted PATH and conflicting inherited frameserver settings. Both source
identity checks and all 53 runtime-file hashes remained unchanged. Independent
decoding verified 992×572 10-bit video, maximum timestamp error 0.500 ms, 961013
Opus samples/channel and source audio correlation 0.9968427 for each result.
Both 10-second PNGs were inspected.

- VMAF 90–97, sampling 2 and L-SMASH produced 1,045,263 bytes. Retained probes show
  CRF 30 / VMAF 97.03 changing to CRF 36.25 / VMAF 96.42. Encoded black-corner
  luma is 65, within the qualified lossy tolerance around 64.
- XPSNR 25–50, every frame and the select reader produced 1,307,677 bytes.
  Retained CRF 30 probe scores include 44.76, 45.75 and 47.04 dB; encoded
  black-corner luma is exactly 64.

The final tool-manifest SHA256 is
`533dac29023d8f469426dd3a9455e3c13f94237c475fec900094485e42241541`.
These results supersede the earlier numeric target qualifications for these two
modes; the source interval remains [2880,3360), not a full episode.

The final corrected bundle then passed all nine retained recovery tests in
569.71 seconds, with no failures. Coverage includes all four additional scorer
configurations with finite-score and numeric checks, VMAF across all three SVT
builds, select/hybrid readers, framing with Opus/AAC, stop/reopen/resume, altered
receipts and hybrid segments, and live pause/continue/cancel. All 53 runtime files
remained byte-identical with installed tools hidden and deliberately conflicting
global VapourSynth/Python settings. Only the separate FFMS2/BestSource reader
case was filtered because those optional source plugins are not in this bundle.
The retained receipt is `target/scorers-qualification-20260913/packaged-recovery-corrected.json`;
its test-runner SHA256 is
`530f078211f430ee7054b6566fc6d86e114050fe2bc34fc267ee69b4b47d2a7c`.
