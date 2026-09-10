<img src="assets/branding/jesses-icon-master.png" width="128" alt="jesses app icon">

# jesses

A desktop app for video encoding, muxing, and media analysis, created by **jkkma**.

Rust, Tauri, Svelte, and shadcn-svelte.

The development build provides a desktop media workspace with native file
selection and drag/drop, FFprobe metadata and stream inspection, and detection of
FFmpeg, FFprobe, standalone SVT-AV1, and av1an on PATH. The interface uses a fixed
parchment-and-rust light theme. The Remux tab copies selected streams from one
source into a new Matroska file, with progress, cancellation, and output validation.

Quick Convert encodes one selected video with standalone SVT-AV1 and copies the
selected audio, subtitles, and attachments. Jobs can be queued sequentially and
their settings and history survive restart. Multi-source muxing, folder import,
thumbnails, audio conversion, pause/resume, and bundled media tools remain pending.
This is a development build, not a release.

## Encode a file

Add a local file in Files, open Quick Convert, choose the video and copied tracks,
and select a new `.mkv` destination. CRF 30 and preset 4 are the initial settings;
the output video is 10-bit AV1. FFmpeg, FFprobe, and the standalone `SvtAv1EncApp`
must be on PATH. The selected tool and its version appear in the job log.

The first encoder supports progressive, constant-frame-rate SDR video with
explicit color metadata, square pixels, and 4:2:0 8-bit or 10-bit input. It rejects
HDR, variable frame rates, rotation, unsupported chroma placement, and nonzero
video/container start times. Source and encoded frames are decoded for timing,
frame count, geometry, and color validation before the output is published. Each
frame scan has a 10-minute and 64 MiB metadata limit; sources beyond those limits
fail explicitly. Selected non-video tracks keep their original codecs.

Use **Start encode** for an idle workspace or **Add to queue** to submit an
immutable settings snapshot. Change the source or destination to add another job.
One job runs at a time, in submission order. **Cancel job** stops one job;
**Stop queue** cancels the active job and every waiting job. A failed job does not
prevent later queued jobs from running. Every input and destination is rechecked
when its job starts.

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
Job logs are stored under the platform app log directory; each tool log retains
the newest two 4 MiB segments. Saved history contains only bounded log summaries.
Cleanup failures are reported with their paths; total disk-log retention remains
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
cargo test -p media-runtime --locked -- --include-ignored
cargo run -p media-runtime --example inspect
cargo run -p media-runtime --example inspect -- /path/to/video.mkv
cargo run -p media-runtime --example remux -- /path/to/video.mkv /path/to/new-output.mkv
cargo run -p media-runtime --example encode -- /path/to/video.mkv /path/to/encoded.mkv 30 4
```

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
  output transactions, and job history/queue.
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
