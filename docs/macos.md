# macOS app archives

Jesses builds native Apple Silicon (`aarch64-apple-darwin`) and Intel
(`x86_64-apple-darwin`) application archives for macOS 12 or newer. Each archive
contains `jesses.app`, its runtime/license resources, a complete size/hash/mode
manifest and these instructions. It uses macOS' built-in WKWebView. No .NET or
separate browser runtime is required.

These are **ad-hoc signed development artifacts**, without Developer ID identity
or notarization. They match the fork's unsigned-development distribution scope;
Apple distribution signing is a separate option. Tauri documents the distinction
in its [macOS signing guide](https://v2.tauri.app/distribute/sign/macos/).

## Media tools

The macOS archive intentionally keeps media tools external, like the fork's
lean macOS releases. Install only the tools needed for your workflows. For a
Homebrew installation, the ordinary starting point is:

```sh
brew install ffmpeg-full mkvtoolnix x264 x265 aom libvpx svt-av1
export PATH="$(brew --prefix ffmpeg-full)/bin:$PATH"
```

FFmpeg and FFprobe must be a matching pair. Audio conversion requires 8.1 or
newer and passes the real codec-delay admission check before video processing.
`svt-av1` supplies mainline SVT, not the distinct HDR or 5fish forks. Select
mainline SVT explicitly, or install the exact fork and configure
`JESSES_SVT_AV1_HDR` / `JESSES_SVT_AV1_5FISH` to its absolute executable path.
Jesses never silently changes the chosen encoder build.

The full FFmpeg formula supplies AOM, Vorbis, libass and zimg support that the
reduced `ffmpeg` formula omits. It is keg-only, so command-line sessions select
its binary directory explicitly. The Finder-launched app also checks the known
`/opt/homebrew/opt/ffmpeg-full/bin` and `/usr/local/opt/ffmpeg-full/bin` locations,
followed by `/opt/homebrew/bin` and `/usr/local/bin`
after the inherited absolute PATH entries. This does not modify your shell or
system PATH. **Tools & settings** reports each resolved executable and its
capability/error details.

The workflow uses GitHub's native [`macos-15` arm64 and `macos-15-intel` x64
runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
The [full FFmpeg formula](https://formulae.brew.sh/formula/ffmpeg-full) is a
different, keg-only installation from the reduced formula. Homebrew does not
currently provide Intel macOS bottles for every dependency in this lane; those
tools may build from source on the Intel runner. Hosted job results, rather than
the existence of formula names, establish native dependency qualification.

AV1AN, VapourSynth source readers, QTGMC plugins, SSIMULACRA2/Butteraugli
plugins, grav1synth, Subtitle Edit's console OCR tool and Tesseract remain
optional external dependencies. The feature's admission checks explain what is
missing. Installing FFmpeg alone does not imply these optional workflows work.

## Build, package and verify

Use the repository's pinned Node/pnpm/Rust versions and Xcode command line tools:

```sh
pnpm install --frozen-lockfile
pnpm tauri build --target aarch64-apple-darwin --bundles app --config src-tauri/tauri.macos.conf.json
python3 scripts/package-macos.py --app target/aarch64-apple-darwin/release/bundle/macos/jesses.app --destination target/macos-arm64 --target aarch64-apple-darwin --native
```

Use `x86_64-apple-darwin` on an Intel Mac. The packager checks the thin Mach-O
architecture, bundle identity, runtime and license resources, ad-hoc signature,
and system-only launcher library dependencies. It refuses an existing destination,
redirected payload entries, changed source bytes and archive inventory/hash/mode
mismatches. Application preferences and media are never copied into the archive.

The **macOS native checks and app archives** workflow runs each architecture on
its matching hosted runner, executes Rust and actual-media gates, builds the app,
then verifies and briefly starts the exact staged native app. It uploads artifacts
and receipts without publishing a release. Runner architectures follow the
[current GitHub runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
A passing startup check establishes that the app remained alive during startup;
it does not replace interactive visual workflow or GPU qualification.
