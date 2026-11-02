# Native Linux QTGMC qualification provider

`scripts/build-qtgmc-linux.py` reconstructs a task-local VapourSynth R79 provider
with the QTGMC dependencies pinned by the Windows source bundle. This is a
Linux x64 qualification toolchain, not a claim of a redistributable Linux
portable application or Windows/macOS qualification. The restored provider at
`target/toolchains/qtgmc-linux-20261101` passes its eight-preset, real-reader gate;
all six native caption tests, including the four QTGMC text/bitmap and frame/bob
export paths, pass in 9.24 seconds. Its actual receipts are `qualification.json`
and `build-provenance.json`; durable application logs are under
`target/migration-validation/image-native-20261101/`.

It needs Python 3.14+ for the source archives, an explicit Python 3.12+ with its
development headers for the frameserver, GCC/G++, CMake 3.28+, Ninja, make,
nasm, pkg-config, Autoconf, Automake and Libtool. It installs only inside its
new destination and private VapourSynth configuration tree. No system package,
Python site-packages, shell startup file or user configuration is modified.

First restore the verified production FFmpeg build, including development
libraries needed by L-SMASH's private decoder:

```sh
~/.local/bin/python3.14 scripts/build-package-ffmpeg-linux.py \
  --build-directory target/toolchains/ffmpeg-full-20261101 \
  --cache target/tool-source-cache --jobs 2
python3 scripts/test-qtgmc-linux.py
~/.local/bin/python3.14 scripts/build-qtgmc-linux.py \
  --destination target/toolchains/qtgmc-linux-20261101 \
  --media-build target/toolchains/ffmpeg-full-20261101 \
  --python /usr/bin/python3.12 --cache target/tool-source-cache --jobs 2
```

Use an unused destination for a fresh build. An interrupted or failed owned
build can be continued by explicitly adding `--resume` to the same command.
Resume verifies the exact input paths, media receipt, source locks and payload
hashes from completed stages before reuse. Failed command logs and intermediates
are retained; it does not silently replace an existing directory or remove
someone else's files. A changed input requires a new destination.

The recipe shares the R79, Cython, L-SMASH and codec pins in
`package-av1an-windows-lock.json`, QTGMC module/plugin pins in
`qtgmc-windows-lock.json`, and Meson 1.12.0 in the Linux FFmpeg lock. The extra
`qtgmc-linux-lock.json` records znedi3's previously unbundled graphengine and
vsxx gitlinks at their exact source commits. The znedi3 source commit is checked
against its checksum-bound GitHub ZIP comment. No moving Git branch or build-time
dependency fetch is allowed. Unbuilt graphengine test dependencies are not used.

The provider builds R79's native SDK/runtime, static single-precision FFTW and
nine plugins: L-SMASH, znedi3, fmtconv, MVTools, Misc, RemoveGrain, EEDI3,
FFT3DFilter and TemporalSoften2. Its L-SMASH compilation reuses the existing
pinned Linux reader recipe and statically links the exact dav1d/libvpx/zlib
closure from the verified FFmpeg build. Source archives and source-derived
notices are retained, including those shared codec sources. The final receipt
records hashes of the runtime, Python modules, plugins and build provenance.

Before reporting readiness, it decodes eight real FFV1 frames through L-SMASH
and renders every frame of QTGMC's eight source presets at `30000/1001`→
`60000/1001` fps. Native app integration remains a separate gate:

```sh
. target/toolchains/qtgmc-linux-20261101/env.sh
cargo test -p media-runtime --test image_subtitle_jobs --locked -- \
  --ignored --nocapture
cargo test -p media-runtime --test image_sequence_source --locked -- \
  --ignored --nocapture --test-threads=1
```

The generated environment uses the actual VSPipe binary and task-local Python
module/plugin/configuration paths. It selects the same verified FFmpeg pair
for application discovery and direct fixture invocations. No stub provider or
test-only substitute establishes QTGMC acceptance.

See `source-image-caption-validation.md` for the prior system-FFmpeg evidence,
the concrete image-caption and sequence gates, and the remaining metadata
failure that must be investigated if it persists on the qualified toolchain.
