<img src="assets/branding/jesses-icon-master.png" width="128" alt="jesses app icon">

# jesses

A desktop app for video encoding, muxing, and media analysis, created by **jkkma**.

Rust, Tauri, Svelte, and shadcn-svelte.

The development build provides a desktop media workspace with native file
selection and drag/drop, FFprobe metadata and stream inspection, and detection of
FFmpeg, FFprobe, standalone SVT-AV1, and av1an on PATH. The interface uses a fixed
parchment-and-rust light theme.

Quick Convert encodes one file at a time using standalone SVT-AV1 for video and
FFmpeg for Opus audio and MKV muxing. It preserves audio tracks, subtitles, font
attachments, chapters, languages, and default/forced track flags. Quality, preset,
audio bitrate/channels, and output destination are editable. Jobs have progress,
cancellation, a saved history of the latest 100 jobs, and interruption recovery.

The initial workflow supports progressive constant-frame-rate SDR video, 8-bit or
10-bit YUV 4:2:0, square pixels, and BT.709 or unspecified color metadata. HDR,
variable frame rate, rotated/interlaced/anamorphic video, and multiple video
streams are rejected with an explanation. Folder import, thumbnails, batch
queues, additional containers, and bundled tools remain future work. This is a
development build, not a release.

## Run locally

Install Node.js 24, pnpm 11.19.0, and the platform's
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/). Rust 1.98.1 is pinned
by `rust-toolchain.toml`. Install FFmpeg with FFprobe for media inspection. Quick
Convert also requires `SvtAv1EncApp` and an FFmpeg build with `libopus`. Make the
executables available on PATH before starting jesses. The Tools panel reports
installed tool versions; the encoding backend checks the source before processing.

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

Run the real-tool integration tests separately with FFmpeg, FFprobe, and standalone
SVT-AV1 on PATH (CI runs these on Linux):

```sh
cargo test -p media-runtime --locked -- --include-ignored
cargo run -p media-runtime --example inspect
cargo run -p media-runtime --example inspect -- /path/to/video.mkv
```

The same job engine can be exercised without the UI. Supply absolute paths; the
optional preset defaults to 4. The example preserves audio channel counts.

```sh
cargo run -p media-runtime --example encode -- /path/to/input.mkv /path/to/output.mkv 4
```

Set `JESSES_JOB_DIRECTORY` to isolate the example's job store (otherwise it uses a
`jesses-example-jobs` directory in the system temporary directory). The example's
`--list` option reopens the store, recovers interrupted jobs, and lists saved jobs. A fourth
positional argument cancels after that many milliseconds, for cancellation tests.

Build a native executable with embedded frontend assets:

```sh
pnpm tauri build --debug --no-bundle
```

Native CI is configured for Windows x64, Linux x64, and macOS arm64/x64. Installer,
signing, clean-machine, and cross-platform runtime qualification remain pending.

## Development layout

- `src/`: Svelte interface and typed Tauri client.
- `src-tauri/`: desktop entry point and window command permissions.
- `crates/media-core/`: platform-independent metadata, encoding, and error contracts.
- `crates/media-runtime/`: tool discovery, probing, process supervision, and durable jobs.
- `tests/`: browser workflows and synthetic fixture recipes.

Rust owns media contracts. Run `pnpm contracts` after changing the DTOs and review
the generated TypeScript; CI rejects drift. Native commands launch known tools
directly with argument arrays. Inspection has bounded output and timeouts;
encoding streams data between FFmpeg and SVT-AV1 without a raw-video intermediate
file. Windows processes belong to a kill-on-close Job Object before they begin
running; Unix processes use owned process groups. Closing the app cancels and
drains an active job before exiting.

Outputs are staged in an owned sibling directory, checked for frame timing,
stream counts, codecs, dimensions, metadata, and chapter preservation, then
published without replacing any existing path. The destination filesystem must
support hard links (for example, NTFS). Cancellation and failure remove only
owned temporary files. After an unexpected stop, reopening the app marks the job
interrupted and cleans its staging files; any published destination is preserved.
Interrupted jobs do not automatically resume.

Desktop jobs are saved under the app's local data directory in `jobs/jobs.json`.
A process lock prevents two app instances from using the same job store. Media
tools and encoding settings are not bundled or installed by the app.

The activity panel retains at most 200 entries in memory. Only its open/closed
state persists; source lists and inspection metadata are session-only. Encoding
job history persists separately.

See [icon assets](assets/branding/README.md) for the master artwork and generated desktop formats.

See [third-party notices](THIRD_PARTY_NOTICES.md) and [license](LICENSE).
