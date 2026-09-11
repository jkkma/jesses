<img src="assets/branding/jesses-icon-master.png" width="128" alt="jesses app icon">

# jesses

A desktop app for video encoding, muxing, and media analysis, created by **jkkma**.

Rust, Tauri, Svelte, and shadcn-svelte.

The development build provides a desktop media workspace with native file
selection and drag/drop, FFprobe metadata and stream inspection, and detection of
FFmpeg, FFprobe, standalone SVT-AV1, and av1an on PATH. The interface uses a fixed
parchment-and-rust light theme. The Remux tab copies selected streams from one
source into a new Matroska file, with progress, cancellation, and output validation.

Quick Convert encodes one selected video with standalone SVT-AV1 or av1an's parallel
SVT-AV1 chunks and copies the selected audio, subtitles, and attachments. Both
backends support validated HDR10 output and optional film grain synthesis.
Folder import and Batch encode prepare
multiple files with individual track selections and common quality settings.
Jobs run sequentially, and their settings and history survive restart. Multi-source
muxing, thumbnails, audio conversion, pause/resume, and bundled media tools remain pending.
This is a development build, not a release.

## Encode a file

Add a local file in Files, open Quick Convert, choose the video and copied tracks,
and select a new `.mkv` destination. CRF 30 and preset 4 are the initial settings;
CRF 1–63 and presets 0–13 are accepted. The output video is 10-bit AV1.
FFmpeg, FFprobe, and the standalone `SvtAv1EncApp`
must be on PATH. The selected tool and its version appear in the job log.

The encoder supports progressive, constant-frame-rate SDR video with
explicit color metadata, square pixels, and 4:2:0 8-bit or 10-bit input. HDR10 uses
limited-range 10-bit BT.2020/PQ with validated mastering metadata. It rejects
unsupported HDR formats, variable frame rates, rotation, unsupported chroma placement, and nonzero
video/container start times. Dimensions must be even, from 64 through 8192 pixels,
and frame rates must be between 1 and 120 fps. Source and encoded frames are decoded for timing,
frame count, geometry, and color validation before the output is published. Each
frame scan has a 10-minute and 64 MiB metadata limit; sources beyond those limits
fail explicitly. Long HDR movies can exceed these bounds; short excerpts are
qualified, while streaming frame validation for full movies remains pending.
Selected non-video tracks keep their original codecs.

**Encode backend** defaults to standalone SVT-AV1. Choose **av1an** to use scene
detection and parallel encoding with 1–32 workers (default 2), capped at 240 frames
per chunk. This integration requires av1an, VapourSynth, and L-SMASH Works in
addition to FFmpeg, FFprobe, and SVT-AV1. Jesses checks av1an's reported plugin
availability. av1an currently encodes the first video track only; standalone SVT
can encode another selected video track. Jobs remain sequential; workers run
chunks within the active job. More workers require more CPU and memory.

av1an runs in a uniquely reserved workspace inside the output folder, with caches
kept there. Completed-chunk frame counts drive progress. The output passes the
same decoded-frame, track, and metadata checks as standalone encoding. Jesses
validates the concatenated IVF frame records and corrects its rate/count header
before muxing, covering av1an versions that write a fixed 30 fps header. Existing
destinations are never replaced. Cancel stops the supervised av1an worker tree;
remaining chunk work files are retained with their location in the job log.
There is no automatic resume. Source files and their folders receive no caches.

**Film grain synthesis** accepts 0–50, default 0 (off). Nonzero values add AV1
grain synthesis; encoder denoising remains disabled. Synthesis does not reproduce
the source grain exactly. The same setting is available for both backends and
batch encoding; no content preset silently enables grain.

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
tracks for each file. Choose common backend, workers, CRF/preset, grain, and HDR10
fallback settings and an existing writable
output folder, then select **Preview batch**. The app proposes names such as
`episode_av1.mkv` and `episode_av1_2.mkv`, avoiding existing files and destinations
already reserved in the queue. Unicode names and spaces are retained where valid.
The preview creates no output files or folders.

The preview reports per-file selection and header compatibility errors. Each
FFprobe header inspection is limited to 30 seconds and 2 MiB of metadata. A ready
row still requires the full decoded-frame and output validation when its job runs.
Changing files, tracks, backend, workers, quality, grain, HDR fallback, or the output folder requires a new
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
standalone SvtAv1EncApp on PATH:

```sh
cargo test -p media-runtime --lib --locked -- --include-ignored
cargo test -p media-runtime --test real_tools --locked -- --include-ignored
cargo run -p media-runtime --example inspect
cargo run -p media-runtime --example inspect -- /path/to/video.mkv
cargo run -p media-runtime --example remux -- /path/to/video.mkv /path/to/new-output.mkv
cargo run -p media-runtime --example encode -- /path/to/video.mkv /path/to/encoded.mkv 30 4
```

With av1an, VapourSynth and L-SMASH Works installed, run its integration gate:

```sh
cargo test -p media-runtime --test av1an_jobs --locked -- --ignored --test-threads=1
cargo run -p media-runtime --example encode -- /path/to/video.mkv /path/to/encoded.mkv 30 4 0 false av1an 2
```

The encode example's optional arguments are CRF, preset, grain strength, explicit
HDR10 fallback (`true`/`false`), backend (`svtAv1`/`av1an`), and worker count.
See the upstream [av1an CLI reference](https://rust-av.github.io/Av1an/) and
[SVT-AV1 parameters](https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/v4.0.0/Docs/Parameters.md)
for the underlying tools.
See the [local av1an/HDR validation record](tests/fixtures/av1an-validation.md)
for tested media characteristics and remaining qualification limits.

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
