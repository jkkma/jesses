<img src="assets/branding/jesses-icon-master.png" width="128" alt="jesses app icon">

# jesses

A desktop app for video encoding, muxing, and media analysis, created by **jkkma**.

Rust, Tauri, Svelte, and shadcn-svelte.

The development build provides a desktop media workspace with native file
selection and drag/drop, FFprobe metadata and stream inspection, and detection of
FFmpeg, FFprobe, standalone SVT-AV1, x264, and av1an on PATH. The interface uses a fixed
parchment-and-rust light theme. The Remux tab copies selected streams from one
source into a new Matroska file, with progress, cancellation, and output validation.

Quick Convert drives standalone SVT-AV1 and x264 executables. The separate av1an
tab handles scene detection and parallel SVT-AV1 chunks. Both workflows copy
selected audio, subtitles, and attachments. SVT-AV1 supports validated HDR10 output
and optional film grain synthesis; x264 currently supports SDR H.264 output.
Folder import and Batch encode prepare
multiple files with individual track selections and common quality settings.
Jobs run sequentially, and their settings and history survive restart. Multi-source
muxing, thumbnails, audio conversion, additional standalone encoders, av1an quality
targets and configurable chunk methods, pause/resume, and bundled media tools remain pending.
This is a development build, not a release.

## Encode a file

Add a local file in Files, open Quick Convert, choose the video and copied tracks,
and select a new `.mkv` destination. SVT-AV1 starts at CRF 30 and preset 4;
CRF 1–63 and presets 0–13 are accepted. The output video is 10-bit AV1.
FFmpeg, FFprobe, and the standalone `SvtAv1EncApp`
must be on PATH. The selected tool and its version appear in the job log.
Standalone `aomenc`, `vpxenc`, and `x265` drivers remain pending; Quick
Convert will expose them as encoder choices as each driver is implemented.
See the [standalone driver implementation plan](docs/standalone-encoders.md) for
the remaining architecture and qualification gates.

Choose **x264 · H.264** in Quick Convert for direct x264 encoding, or choose x264
with the standalone workflow in Batch encode. It defaults to CRF 23 and the
**medium** preset; CRF 0–51 and the ten named x264 presets are supported. The
installed executable must advertise Y4M input, Matroska output, and the source's
8-bit or 10-bit depth. Source depth, range, dimensions, cadence, and SDR color are
retained; x264 currently requires explicit left, center, or top-left chroma
placement. HDR sources, film grain synthesis, and HDR10 fallback are rejected for
x264. CRF 0 does not promise lossless 10-bit output; a separate lossless mode is
not implemented. Existing SVT-AV1 settings and saved jobs remain readable.

x264 writes a timed Matroska intermediate through the supervised pipeline before
selected tracks are copied into the final Matroska output. This preserves B-frame
presentation order without disabling B-frames or reconstructing timestamps from a
raw H.264 stream. The completed output must pass the same complete decoded-frame,
track, and metadata checks before publication. See the [x264 validation record](tests/fixtures/x264-validation.md).

Encoding supports progressive, constant-frame-rate SDR video with
explicit color metadata, square pixels, and 4:2:0 8-bit or 10-bit input. SVT-AV1 HDR10 uses
limited-range 10-bit BT.2020/PQ with validated mastering metadata. It rejects
unsupported HDR formats, variable frame rates, rotation, unsupported chroma placement, and nonzero
video/container start times. Dimensions must be even, from 64 through 8192 pixels,
and frame rates must be between 1 and 120 fps. Source and encoded frames are decoded for timing,
frame count, geometry, and color validation before the output is published. Frame
metadata is parsed and checked incrementally, so memory does not grow with video
length. Each frame's metadata is limited to 1 MiB, and each complete scan has a
24-hour execution limit. Scans use up to eight decoder threads, capped by the
available logical processors. Preparation and finalization show separate scan
progress. After observing progress for five seconds, the app estimates the current
phase's speed and remaining time; estimates reset between phases and disappear
when progress stops arriving. Cancellation stops and awaits the scanner's process tree.
Selected non-video tracks keep their original codecs.

Open the separate **av1an** tab to use scene detection and parallel encoding with
1–32 workers (default 2), capped at 240 frames
per chunk. This integration requires av1an, VapourSynth, and L-SMASH Works in
addition to FFmpeg, FFprobe, and SVT-AV1. Jesses checks av1an's reported plugin
availability. av1an currently encodes the first video track only; standalone encoders
can encode another selected video track. Jobs remain sequential; workers run
chunks within the active job. More workers require more CPU and memory.

Quick Convert and av1an keep independent settings, copied-track selections, and
destinations for each source and encoder while the app remains open. Returning to a source
restores that workflow's draft; **Reset settings** resets only its current draft.
Default destinations end in `_av1.mkv`, `_x264.mkv`, or `_av1an.mkv`. Quick Convert
always starts the standalone encoder directly; the av1an tab always starts av1an.

av1an runs in a uniquely reserved workspace inside the output folder, with caches
kept there. Completed-chunk frame counts drive progress. The output passes the
same decoded-frame, track, and metadata checks as standalone encoding. Jesses
validates the concatenated IVF frame records and corrects its rate/count header
before muxing, covering av1an versions that write a fixed 30 fps header. Existing
destinations are never replaced. Cancel stops the supervised av1an worker tree;
remaining chunk work files are retained with their location in the job log.
There is no automatic resume. Source files are never modified; caches stay in the
reserved output workspace, even when the output folder also contains the source.

**Film grain synthesis** accepts 0–50, default 0 (off). Nonzero values add AV1
grain synthesis; encoder denoising remains disabled. Synthesis does not reproduce
the source grain exactly. The same setting is available for SVT-AV1 in Quick
Convert, av1an, and batch encoding; no content preset silently enables grain.

Static HDR mastering and content light metadata are checked in the source and
decoded output, allowing only AV1's fixed-point precision difference. **Allow
HDR10 fallback** is off by default. Turning it on explicitly permits discarding
Dolby Vision enhancement data and HDR10+ dynamic metadata in favor of an HDR10
base layer. Supported Dolby Vision input is HEVC profile 7/compatibility 6 or
profile 8/compatibility 1; profile 5 and unrecognized profiles fail explicitly.
HLG and tone mapping remain pending. Files shows reported pixel format, bit depth,
color tags, and HDR indicators; metadata absent from stream headers is labeled
as unreported because it may still exist on decoded frames.

A source declaring `24000/1001` fps may use timestamps authored at decimal
`23.976` (`2997/125`) fps. The encoder accepts that one alternative only when every
decoded frame fits the existing timestamp tolerance and all frame metadata checks
pass. The accepted cadence is used for decoding, encoding, and output validation;
the tolerance is not widened to accept gaps or variable frame rates.

Use **Start encode** for an idle workspace or **Add to queue** to submit an
immutable settings snapshot. Change the source or destination to add another job.
One job runs at a time, in submission order. **Cancel job** stops one job;
**Stop queue** cancels the active job and every waiting job. A failed job does not
prevent later queued jobs from running. Every input and destination is rechecked
when its job starts.

## Import a folder and prepare a batch

In Files, choose **Add folder** and optionally include subfolders. Discovery reads
regular files with supported media extensions, sorts the discovered paths, and
skips symbolic links and Windows reparse points. Errors and skipped entries remain
visible. A scan returns at most 500 media files after examining at most 10,000
entries; discovery times out after 30 seconds. A truncated scan should be retried
with a smaller folder. **Stop import** keeps completed imports and discards late
results; the current read-only scan or probe may finish in the background.

Open **Batch encode**, select up to 100 files, and review the video and copied
tracks for each file. Choose the workflow and encoder, then common workers,
CRF/preset and the encoder's supported grain/HDR10
fallback settings and an existing writable
output folder, then select **Preview batch**. The app proposes names such as
`episode_av1.mkv` and `episode_av1_2.mkv`, avoiding existing files and destinations
already reserved in the queue. x264 proposals use `_x264` instead of `_av1`. Unicode
names and spaces are retained where valid.
The preview creates no output files or folders.

The preview reports per-file selection and header compatibility errors. Each
FFprobe header inspection is limited to 30 seconds and 2 MiB of metadata. A ready
row still requires the full decoded-frame and output validation when its job runs.
Changing files, tracks, workflow, encoder, workers, quality, grain, HDR fallback, or the output folder requires a new
preview.

**Queue ready files** submits only the ready rows, with immutable per-file
settings, in the reviewed order. The entire submitted batch must pass admission
and fit the 100-record queue/history capacity before any new job is added. A
collision, invalid input, or history-write failure rejects the submission without
partially adding it. **Stop queue** also invalidates a batch submission whose
source checks are still in progress. Preview again before retrying a rejected
batch.

## Remux a file

Add a local file in Files, open Remux, choose the streams and their order, and
select a new `.mkv` destination. Keep attachments after video/audio/subtitle tracks.
At least one video or audio track is required. FFmpeg and FFprobe must be on PATH.
Streams are copied without encoding; unsupported Matroska streams fail explicitly.

Existing destinations are never replaced. The app writes a temporary sibling,
checks packet counts, selected stream properties, dispositions, metadata, chapters,
attachment hashes, and duration, then publishes the output without overwriting.
Publication requires a filesystem with hard-link support (for example NTFS);
unsupported filesystems fail and leave existing files unchanged. Verification scans
the source and output, so preparing/finalizing can take time for large files.

Cancel stops the owned process trees and removes temporary output. Closing the
app cancels and awaits active and queued jobs. The desktop app saves up to 100 job
records under its platform data directory. Jobs left unfinished after a crash are
shown as **Interrupted** on restart; they never resume automatically, signal old
process IDs, or delete old media paths. Review their output and logs before
submitting a new job. Encoder frames/chunks cannot be resumed yet. Job settings
are retained for history; they do not become defaults for new jobs.

History uses an exclusive instance lock and atomic file replacement. If history
cannot be read or saved, new jobs are blocked and the error remains visible;
corrupted history is preserved. Correct the problem and restart the app.
Job logs are stored under the platform app log directory; each supervised tool log retains
the newest two 4 MiB segments. Saved history contains only bounded log summaries.
av1an also writes its own detail log there. Cleanup failures are reported with
their paths; total disk-log retention remains
future work. Command-line examples use in-memory history.

## Run locally

Install Node.js 24, pnpm 11.19.0, and the platform's
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/). Rust 1.98.1 is pinned
by `rust-toolchain.toml`. For media inspection, install FFmpeg with FFprobe and make
the executables available on PATH before starting jesses.

```sh
pnpm install --frozen-lockfile
pnpm desktop
```

`pnpm dev` starts a browser preview at `http://127.0.0.1:1420`. The browser has no
local media access; its optional sample is explicitly labeled synthetic data.

```sh
pnpm check
pnpm build
pnpm exec playwright install chromium
pnpm test
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
pnpm contracts:check
```

Run the real-tool integration tests separately with FFmpeg, FFprobe, and
standalone SvtAv1EncApp and x264 on PATH:

```sh
cargo test -p media-runtime --lib --locked -- --include-ignored
cargo test -p media-runtime --test real_tools --locked -- --include-ignored
cargo run -p media-runtime --example inspect
cargo run -p media-runtime --example inspect -- /path/to/video.mkv
cargo run -p media-runtime --example remux -- /path/to/video.mkv /path/to/new-output.mkv
cargo run -p media-runtime --example encode -- /path/to/video.mkv /path/to/encoded.mkv 30 4
cargo test -p media-runtime --test x264_jobs --locked -- --ignored --test-threads=1
cargo run -p media-runtime --example encode -- /path/to/video.mkv /path/to/encoded.mkv 23 5 0 false standalone 2 x264
```

With av1an, VapourSynth and L-SMASH Works installed, run its integration gate:

```sh
cargo test -p media-runtime --test av1an_jobs --locked -- --ignored --test-threads=1
cargo run -p media-runtime --example encode -- /path/to/video.mkv /path/to/encoded.mkv 30 4 0 false av1an 2
```

The encode example's optional arguments are CRF, preset, grain strength, explicit
HDR10 fallback (`true`/`false`), workflow (`standalone`/`av1an`), worker count, and
encoder (`svtAv1`/`x264`). The old `svtAv1` workflow name remains accepted by the
example and when loading older history. x264 integration tests require x264 on PATH.
See the upstream [av1an CLI reference](https://rust-av.github.io/Av1an/) and
[SVT-AV1 parameters](https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/v4.0.0/Docs/Parameters.md)
for the underlying tools.
See the [local av1an/HDR validation record](tests/fixtures/av1an-validation.md)
for tested media characteristics and remaining qualification limits.
See the [streaming validation record](tests/fixtures/streaming-validation.md)
for full-source scans, bounded-memory checks, and native UI qualification.

Build a native executable with embedded frontend assets:

```sh
pnpm tauri build --debug --no-bundle
```

Native CI targets Windows x64 and Linux x64. macOS support is deferred. Installer,
signing, clean-machine, and cross-platform runtime qualification remain pending.

## Development layout

- `src/`: Svelte interface and typed Tauri client.
- `src-tauri/`: desktop entry point and window command permissions.
- `crates/media-core/`: platform-independent metadata and error contracts.
- `crates/media-runtime/`: tool discovery, probing, process pipelines, validated
  output transactions, folder/batch preparation, and job history/queue.
- `tests/`: browser workflows and synthetic fixture recipes.

Rust owns media contracts. Run `pnpm contracts` after changing the DTOs and review
the generated TypeScript; CI rejects drift. Native commands launch known tools
directly with argument arrays, bounded output, and timeouts. Job supervision uses
atomic Job Object assignment on Windows 10+ and process groups on Unix. Unix tools
must not deliberately detach from their process group. Binary pipelines use
bounded buffers and owned file handles, require success from both stages, and
stop both trees on failure. Full encode resume and cross-platform native UI
qualification remain future gates.

The activity panel retains at most 200 entries in memory. Only its open/closed
state persists; source lists and media metadata are session-only.

See [icon assets](assets/branding/README.md) for the master artwork and generated desktop formats.

See [third-party notices](THIRD_PARTY_NOTICES.md) and [license](LICENSE).
