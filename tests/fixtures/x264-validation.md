# Standalone x264 validation

Validated on Windows on 2026-09-11 with FFmpeg/FFprobe 9.0.1 and x264
0.165.3222M (8/10-bit, built-in Matroska support). These are development
qualification results, not installer or clean-machine qualification.

## Execution and timing

Quick Convert launches the standalone x264 executable. FFmpeg supplies decoded
Y4M frames; x264 writes a timed Matroska stream into an already-owned file handle.
The common mux step copies selected source tracks around that encoded video.
The application decodes every source and output frame, validates count and
timing, and checks tracks and metadata before publishing without overwriting.

The driver supplies the validated rational cadence, source depth/range, SDR color,
square-pixel aspect ratio, and explicit chroma placement. It preserves B-frame
reordering without disabling B-frames. Direct intermediate experiments covered
24000/1001, 2997/125, and 30000/1001 fps, including a seekable inherited stdout
handle. Matroska muxing retained presentation times within 0.5 ms.

## Actual media and native interface

- A 1920 x 1080 SDR anime excerpt completed through the public job manager and
  the rebuilt native Quick Convert form at CRF 23 / medium. Both outputs contain
  236 decoded H.264 frames, with the original AAC track, ASS subtitles, and 24
  font attachments selected.
- A complete 1920 x 1080 episode completed at CRF 23 / fast: 34,047 frames,
  including 22,076 B-frames, at the validated decimal cadence 2997/125 fps.
  Encoding alone ran at approximately 125 fps on this machine; source and output
  validation are additional phases. The job passed complete decoded-output
  validation and published successfully.
- Independent checks found a maximum video timestamp error of 0.499834 ms. All
  61,159 AAC packet payloads and PTS/DTS matched exactly, and both tracks decoded
  to identical PCM: 62,626,816 samples per channel at 44.1 kHz. FFprobe reported
  7,630 packet durations as 24 ms in the source and 23 ms after remuxing; this
  one-tick representation difference did not change audio samples or timestamps.
- Full-episode qualification exposed a subtitle header ambiguity: FFprobe
  reported source stream start 0 and output stream start 0.740 seconds, although
  all 353 ASS packet timestamps, durations, and payload hashes matched exactly.
  Full job probes now inspect the first packet of each selected nonempty subtitle stream
  with a bounded, supervised query. Start validation compares that actual cue
  timestamp. Tests still reject shifted cues.
- The native interface offered x264's CRF 23 / medium defaults and 8-bit source
  label, saved the completed job, and retained older SVT-AV1 history. The separate
  av1an tab retained its own SVT settings and chunk controls.
- Importing the 3840 x 2160 HDR movie excerpt showed an explicit SDR-only x264
  message and disabled submission. The anime and HDR excerpt source hashes
  remained unchanged.

## Automated gates

The opt-in `x264_jobs` integration target uses actual encoders and generated
media, including:

- 8-bit and 10-bit SDR, each with limited and full range; full-range H.264 input
  can be encoded again without an unintended range conversion.
- Fractional cadence and actual B-frames, complete decoded-frame counts, and
  alternate video selection with reordered copied streams.
- Unicode paths, audio/subtitle packet hashes and timing, attachment hashes,
  chapters, and source preservation.
- Batch previews and immutable queued settings, persistent history, missing
  executable reporting, HDR rejection, and active cancellation without output
  publication or locked temporary files.

The 96 frontend tests cover the existing workflows plus encoder defaults,
per-source/per-encoder drafts, asynchronous picker/submission races, tool gates,
HDR restrictions, batch preview invalidation, queue snapshots, and history labels.
Existing real SVT-AV1, av1an, probing, and remux integration gates also pass.

Parallel cancellation testing exposed a Windows inheritance race: a concurrently
launched inspection process could inherit a protected encoder output handle.
Inspection now uses the same explicit handle-list process launcher as encoding.
A deterministic Windows regression verifies that an unrelated live inspection
cannot keep that output locked. Test tool launches use the same owned launcher.

Run the native x264 gate with FFmpeg, FFprobe, and x264 on PATH:

```sh
cargo test -p media-runtime --test x264_jobs --locked -- --include-ignored
```

Linux CI installs x264 and runs this gate alongside the existing native checks.

## Remaining scope

x264 currently supports progressive, constant-rate, square-pixel 4:2:0 SDR with
explicit supported color and left, center, or top-left chroma placement. HDR,
tone mapping, film grain synthesis, interlacing, scaling, and audio conversion
are unavailable in this driver. CRF 0 does not promise lossless 10-bit output;
an explicit lossless mode remains pending. x264 through av1an, standalone x265,
AOM, and VPX remain separate milestones. Executables are discovered on PATH and
are not bundled.
