# av1an and HDR10 validation

Local Windows qualification on 2026-09-11 used FFmpeg/FFprobe 9.0.1, av1an
0.5.2-unstable (7df934d), VapourSynth with L-SMASH Works, and standalone
SVT-AV1-HDR 4.1.0-19. Rust checks used the installed 1.98.0 toolchain; the
repository's rustup configuration remains pinned to 1.98.1. This is local
qualification, not clean-machine or cross-platform certification.

User-owned media and generated excerpts stay outside the repository. No source
media was overwritten. The following runs used the public `encode` example and
the same JobManager path as desktop requests:

| Source excerpt                                      | Request                                                               | Verified output                                                                                                                                |
| --------------------------------------------------- | --------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| 1080p SDR anime, 236 frames                         | av1an, 2 workers, CRF 30, preset 10, grain off                        | AV1 10-bit, 24000/1001 fps, all frames, original audio/subtitles/font attachments                                                              |
| 2160p HEVC with DV profile 7 and HDR10+, 114 frames | av1an, 2 workers, CRF 30, preset 10, grain 8, explicit HDR10 fallback | AV1 10-bit BT.2020/PQ limited range, all frames, mastering metadata within AV1 precision, exact MaxCLL 200/MaxFALL 142, copied audio/subtitles |

Grain side data was independently confirmed on five decoded HDR output frames.
The HDR excerpt's source hash was unchanged. The initial stream-copy excerpt had
missing tail-frame timestamps; it was replaced with a cut at a complete GOP.
The application correctly rejected the damaged excerpt. Timing tolerances were
not widened to accommodate it.

Two upstream compatibility findings shaped this integration:

- The installed av1an's FFmpeg hybrid chunker uses removed `-vsync` syntax with
  FFmpeg 9. The qualified backend explicitly uses L-SMASH Works and checks its
  reported availability.
- av1an IVF concatenation wrote a 30 fps header and the first chunk's frame count
  despite correct sequential frame records. Jesses checks the complete owned IVF
  structure, geometry, frame count, and sequential timestamps before correcting
  rate/count fields. The normal full decoded-output check still follows.

Automated gates cover old settings defaults, metadata validation and rejection,
queue/preflight immutability, malformed IVF refusal, literal path arguments,
progress parsing, existing destination refusal, and active av1an cancellation.
Run `cargo test -p media-runtime --test av1an_jobs -- --ignored --test-threads=1`
with all av1an dependencies installed. Ordinary CI runs the portable tests and
the separately installed FFmpeg/SVT gates; it does not silently skip failed
av1an dependency checks.

The full movie has not been encoded. Source/output frame scans now validate
incrementally with bounded memory; see the [streaming validation record](streaming-validation.md)
for complete-source qualification and current limits. Explicit stop and durable
resume are covered in the [recovery validation record](av1an-recovery-validation.md).
Target-quality modes, additional source plugins, bundled tools, and Linux av1an
runtime qualification remain future work.
