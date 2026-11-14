<img src="assets/branding/jesses-icon-master.png" width="128" alt="jesses app icon">

# jesses

Jesses is a desktop workspace for video encoding, muxing, and media utilities,
created by **jkkma**. It uses Rust, Tauri, Svelte, and shadcn-svelte.

The interface brings together file and track selection, Quick Convert, AV1AN,
utilities, and settings in a compact workspace. Jesses runs **CPU video encoders
only**: x264, x265, VPX, SVT-AV1, and AOM. Vulkan processing, libplacebo Spline
tone mapping, and GPU-assisted metrics remain available when their dependencies
and hardware support them. Copying media does not depend on how it was encoded.
See [CPU encoder scope](docs/cpu-only-encoding.md).

## Getting started

Jesses releases currently target Windows x64. Linux and macOS support are
deferred. See [platform status](#platform-status) for development and
qualification details.
To run the desktop app from a checkout, install:

- Node.js 24 or newer and pnpm 11.19.0, as specified in `package.json`.
- Rust 1.98.1, pinned by `rust-toolchain.toml`, and the platform's
  [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).
- FFmpeg and FFprobe for media inspection, plus the CPU encoder and other tools
  needed for the chosen workflow. AV1AN additionally needs a compatible
  VapourSynth frameserver and source reader. Some utilities need extra tools.

From the repository root:

```sh
pnpm install --frozen-lockfile
pnpm desktop
```

Tool discovery supports explicit `JESSES_<TOOL_ID>` executable overrides, such
as `JESSES_FFMPEG` and `JESSES_FFPROBE`, verified bundled tools where available,
and permitted PATH fallbacks. Provider-specific rules also apply. See
[packaging and tool discovery](docs/packaging.md),
[SVT provider setup](docs/svt-forks.md), and
[utility dependencies](docs/media-utilities.md#dependencies). The source-style
SVT-AV1 choice resolves Patman's MSVC HDR-PMod in the Windows bundle. The
bundled encoder requires the x64 Microsoft Visual C++ v14 Redistributable
14.51 or later. [Download it from Microsoft](https://aka.ms/vc14/vc_redist.x64.exe).
Older jobs retain their saved encoder identifiers.

`pnpm dev` starts a browser preview at `http://127.0.0.1:1420`. It does not
provide the desktop's local media access or native tools.

## Using the workspace

1. In **File List**, add files or a folder, or drag media into the window. Choose
   **Muxing Mode** to work with tracks from loaded files, or **Batch Processing
   Mode** to run a task on each file separately. In Muxing Mode, select files and
   use **Load Tracks** (or double-click a file) to add their tracks.
2. In **Track List**, choose and order the tracks to include. Audio, subtitle,
   attachment, metadata, and container compatibility depend on the selected
   task and destination. Manual track selection is unavailable in Batch mode.
3. Choose **Quick Convert** for a single encoder pipeline, stream copy, video
   removal, or GIF/PNG/JPEG output. Choose **AV1AN** for scene-based, parallel
   CPU encoding. Set the output path, container, video and track options, and
   any processing needed for the source.
4. Press **Run** in the shared footer. Progress, Pause/Resume where supported,
   Stop, and the activity log remain accessible across tabs. Batch mode runs
   eligible tasks in file order, with a naming template and per-file status.

Quick Convert and AV1AN provide trim, crop, resize, borders, frame processing,
color and tone-mapping controls, audio and subtitle options, and advanced encoder
settings. Availability depends on the selected workflow, source, and installed
providers. The Advanced filter grid accepts raw FFmpeg filter strings in order;
use filters appropriate for the media and output. For details and limitations,
see [custom filters](docs/source-custom-filters.md),
[source selection and previews](docs/source-selection-and-previews.md), and
[images and sequences](docs/images.md).

Hold **Shift** as an encode reaches command preparation to review and edit its
arguments before execution. In a batch, Shift is checked separately for each
file. Closing or clearing the review cancels that item. Edited commands are
intended for users who understand the underlying tools; review them carefully.
See [command review](docs/source-command-review-ui.md).

## Utilities and recovery

The **Utilities** tab includes cuts, concatenation, color metadata transfer,
subtitle OCR, grain tools, quality metrics, bitrate charts, and sample encodes.
Select a utility and press Run; some operations require Muxing Mode or additional
tools. See [media utilities](docs/media-utilities.md) for dependencies and
operation-specific behavior. In particular, color metadata transfer can edit a
selected target in place; review its dialog and the documentation before running it.

The app validates media and output compatibility and normally publishes to a new,
noncolliding destination. Some operations scan or decode the complete source or
output, so preparation and finalization can take time. The activity log provides
progress and diagnostics.

Eligible interrupted jobs can be resumed from verified saved work. AV1AN's
**Resume…** and **Clear Temp…** controls offer saved settings, new settings, and
cleanup actions where applicable. Recovery checks source, tool, and saved-work
identity; it may reject changed or unrecognized inputs rather than reuse them.
See [AV1AN recovery](docs/source-av1an-recovery-actions.md). Completion and
shutdown actions are session-only; an armed shutdown has an abortable countdown.
See [completion actions](docs/completion-actions.md).

## Platform status

Windows x64 runs the automated frontend and desktop checks and receives the
portable ZIP and Scoop manifest. See the [Windows handoff](docs/windows-local-validation.md)
and [packaging guide](docs/packaging.md) for native qualification scope.

Linux support is deferred, including automated checks and hosted packages.
The [Linux validation record](docs/final-linux-validation.md) documents the
previous checks. macOS support is also deferred, with manual development
builds described in the [macOS notes](docs/macos.md).

## Development

The frontend is in `src/`, the Tauri desktop entry in `src-tauri/`, shared media
contracts in `crates/media-core/`, and process supervision and media workflows in
`crates/media-runtime/`. Browser tests and fixture recipes are in `tests/`.

Common checks are `pnpm check`, `pnpm build`, `pnpm test`, and
`pnpm contracts:check`. Rust checks use Cargo; provider and real-media tests have
additional dependencies. Rust owns the IPC contracts: after changing DTOs, run
`pnpm contracts` and review the generated TypeScript. Full native builds and
tests can consume substantial disk space; consult the
[Linux validation record](docs/final-linux-validation.md) for bounded examples
and qualification history.

See [migration and parity notes](docs/migration-parity.md) for detailed scope,
[icon assets](assets/branding/README.md), [third-party notices](THIRD_PARTY_NOTICES.md),
and the [GPL-3.0 license](LICENSE).
