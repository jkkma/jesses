# Explicit frame-paired quality analysis

The inspector compares selected reference and candidate video streams with SSIM,
PSNR or VMAF v0.6.1. Each interval uses an explicit zero-based start frame and
frame count. Full decoded inspection establishes that both intervals exist and
have matching dimensions, 8/10-bit 4:2:0 pixel format, SDR color, chroma placement,
square pixels and relative frame timing (2.1 ms container-rounding tolerance).
Ordinal timestamps then pair exactly one corresponding frame from each input.
No frame repetition, automatic scaling or implicit tone mapping is used. Users
must still verify that the selected intervals show corresponding content.

PSNR uses FFmpeg's pooled-error summary, not an average of decibel values or the
rounded per-frame MSE log. Infinite PSNR is represented explicitly as identical
decoded pixels. SSIM and VMAF aggregate the per-frame scores. VMAF selects the
versioned built-in `vmaf_v0.6.1` model explicitly. The [FFmpeg filter documentation](https://ffmpeg.org/ffmpeg-filters.html)
defines the scorers and frame synchronization options.

Windows qualification on 2026-09-13 passed two parser cases, two browser cases,
and `quality_jobs` with actual FFmpeg/FFprobe. The real-tool test compares PSNR
with independently decoded byte-level mean-square error to within 0.00001 dB,
checks exact SSIM and infinite PSNR on identical pixels, runs VMAF, rejects an
interval beyond EOF, and checks cancellation and source hashes/timestamps.
The browser checks cover original stream identities, explicit offsets/count,
keyboard frame inspection, invalid counts and late-result cancellation.

A real 120-frame section of the full av1an episode output was compared with an
explicitly prepared lossless reference from original frames [2880,3000), with the
same 16-pixel top border and 10-bit format. Scores were SSIM **0.9985560333**,
PSNR **55.50073 dB**, and VMAF **97.0998128**. These scores apply only to that
matched interval. Requests, lossless reference and per-frame results are retained
under `Videos/Jesses-migration-native-20260913/real-quality-*`.

Work is cancellable, shares the two-analysis concurrency limit, and owns every
tool process. Source scans cap at one million frames, requested intervals at
60,000 frames, and tool records/reports have explicit size/time limits. The frozen
Windows desktop build `49c84b2425f90efa7842acc5d297fd2dd4c23458550d575a5843e1e744fb6777`
used its source-built FFmpeg/FFprobe pair to compare this real interval through
the native candidate picker and explicit frame controls. Its displayed VMAF
97.100 and final-frame119 score97.354 match the retained numeric result after
display rounding. Keyboard End reached that final point. Evidence is retained
under `Videos/Jesses-complete-native-20260913/native-vmaf-*`.
Linux runtime and final release qualification remain separate gates. HDR scoring
and additional metric plugins are not enabled.
