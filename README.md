<img src="assets/branding/jesses-icon-master.png" width="128" alt="jesses app icon">

# jesses

A desktop app for video encoding, muxing, and media analysis, created by **jkkma**.

Rust, Tauri, Svelte, and shadcn-svelte.

The first development build provides a desktop media workspace with native file
selection and drag/drop, FFprobe metadata and stream inspection, and detection of
FFmpeg, FFprobe, standalone SVT-AV1, and av1an on PATH. The interface uses a fixed
parchment-and-rust light theme.

Quick Convert currently shows a disabled configuration preview. Encoding, muxing,
folder import, thumbnails, saved jobs, and bundled media tools are not implemented
yet. This is a development foundation, not a release.

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

Run the real-tool integration tests separately with FFmpeg/FFprobe on PATH:

```sh
cargo test -p media-runtime --locked -- --include-ignored
cargo run -p media-runtime --example inspect
cargo run -p media-runtime --example inspect -- /path/to/video.mkv
```

Build a native executable with embedded frontend assets:

```sh
pnpm tauri build --debug --no-bundle
```

Native CI is configured for Windows x64, Linux x64, and macOS arm64/x64. Installer,
signing, clean-machine, and cross-platform runtime qualification remain pending.

## Development layout

- `src/`: Svelte interface and typed Tauri client.
- `src-tauri/`: desktop entry point and window command permissions.
- `crates/media-core/`: platform-independent metadata and error contracts.
- `crates/media-runtime/`: local tool discovery and bounded read-only probing.
- `tests/`: browser workflows and synthetic fixture recipes.

Rust owns media contracts. Run `pnpm contracts` after changing the DTOs and review
the generated TypeScript; CI rejects drift. Native commands launch known tools
directly with argument arrays, bounded output, and timeouts. The inspection
adapter is not an encoding process supervisor; process-tree ownership, output
transactions, durable jobs, and recovery must precede encoding support.

The activity panel retains at most 200 entries in memory. Only its open/closed
state persists; source lists and media metadata are session-only.

See [icon assets](assets/branding/README.md) for the master artwork and generated desktop formats.

See [third-party notices](THIRD_PARTY_NOTICES.md) and [license](LICENSE).
