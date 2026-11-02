# Source-pinned AV1 film-grain provider

The Nmkoder fork builds `rust-av/grav1synth` commit
`1044228cd411672b565e5762a9b3597f4dd163b0`. Its complete
`.github/scripts/bundle-tools.sh:1416–1618` fetches that commit, builds its locked
Cargo dependencies and tests `presets`; it applies no local patches. The film
stock text files are embedded by `src/presets.rs` at compile time. Crates.io
0.2.0 has the same version number but lacks these presets, unified `apply --iso`,
`--replace` and the IVF stream-clock fixes. Version text alone cannot identify a
usable provider.

Jesses builds the exact source with private static FFmpeg 8.1 and dav1d 1.5.4.
The bindings locked by that source support FFmpeg 6–8.1; they cannot use the
separate packaged FFmpeg 9 pair. The private build enables native decoders plus
dav1d, so grain measurement retains actual FFV1/AV1 decoding. Optional library
auto-detection, encoders, network access and standalone FFmpeg programs are
disabled. The application's main FFmpeg/FFprobe selection remains independent.
No upstream grav1synth source or embedded stock algorithm is patched.

## Build and package

Use Python 3.14, native Rust 1.98.1, a C compiler, make, meson, ninja, nasm,
pkg-config and libclang. Build into a new destination:

```sh
python3 scripts/build-package-grav1synth.py \
  --destination target/grav1synth-provider --jobs 4
```

On Windows, first assemble the repository's isolated media compiler, install
Rust's `x86_64-pc-windows-gnu` target and pass `--msys-root` to that compiler.
`LIBCLANG_PATH` can select the native LLVM directory containing `libclang.dll`.
The independent provider executable uses GNU Rust/C tooling; the Jesses desktop
continues to use its existing MSVC target. The recipe checks actual imports and
refuses non-system runtime DLL dependencies.

`scripts/grav1synth-lock.json` pins complete upstream archives, the embedded dav1d
archive and the source's 14 stock names. Cargo vendoring preserves every locked
crate and its original licenses. The delivery retains the original complete
sources, vendor archive, readable MIT/LGPL/BSD/patent notices, frozen build
inputs, commands, executable hash, preset listing and dependency report. Windows
also retains the matching MinGW compiler/runtime sources. Existing destinations
and cached archives with incorrect hashes are refused.

Pass its `delivery` directory to production staging:

```sh
python3 scripts/stage-bundled-tools.py ... \
  --grav1synth-build target/grav1synth-provider/delivery
```

The Windows/Linux package workflow builds and stages this provider, requires
native manifest discovery, runs actual preset/container media tests and checks
it again after archive extraction. Runtime discovery selects the absolute
`JESSES_GRAV1SYNTH` override, then the verified package manifest, then absolute
PATH entries. A corrupt bundled executable is reported rather than bypassed.
Lean macOS builds keep external media tools; the native source recipe above can
produce an external provider, selected by its absolute override or PATH.

## Qualified source behavior

All source names are retained exactly:

| Family        | Names                                                    |
| ------------- | -------------------------------------------------------- |
| Standalone    | `Super8`, `MaxMid`                                       |
| 16mm          | `16mm`, `16mm-1`, `16mm-2`, `16mm-3`                     |
| Super 35      | `Classic35`, `Classic35-1`, `Classic35-2`, `Classic35-3` |
| Full frame 35 | `Modern35`, `Modern35-1`, `Modern35-2`, `Modern35-3`     |

The format stock suffixes mean default Fujifilm Eterna 250D, `-1` Eterna 500T,
`-2` Kodak Vision3 250D and `-3` Kodak Vision3 200T. Suffixes on standalone names
and unknown suffixes are rejected before writing an output.

For encode-time tables the app follows the source helper's algorithm: create a
15-second 64×64/24fps black AV1 stub, apply the selected provider preset, inspect
its actual headers and extend only the final segment to at least 24 hours.
The table is immutable request data. Tests independently hash the pinned stock
files and compare every emitted synthesis parameter state plus first/final
states with the matching embedded stock. All 14 outputs have distinct parameter
sequences, complete coverage and valid table syntax. Seeds are excluded from
the curve comparison. The fixture is
`crates/media-runtime/tests/fixtures/grav1synth-presets.json`.

The application-level real tests also apply each of the 14 stocks to IVF and
read its actual table back. Photon-noise apply, deliberate replacement, table
extraction and removal run at 24, 24000/1001 and 30000/1001 fps. Every IVF header
clock numerator/denominator, packet timestamp and frame count is identical to
the source. Every base decoded pixel remains identical with synthesis disabled;
source bytes and occupied destinations remain unchanged. Replacement changes
actual synthesis parameters. Compatible MKV, MP4 and WebM tests retain chapters,
stream metadata, attachments and non-video packet evidence as before.

Current Linux evidence, 2026-11-01, is under `/tmp/jesses-toolchain`:

- `packaged-grain-provider-stocks.log`: actual manifest discovery and all 14
  encode-time tables passed, without a grav1synth executable override. The same
  run also passed actual 320×240 padded-stride grain measurement at 8/10 bits
  (2 tests, 8.55s).
- `packaged-grain-provider-containers.log`: three real integration tests passed,
  covering all 14 IVF stock writes, three exact frame clocks, six cut container
  families and four grain container families (3 tests, 6.17s).
- `packaged-grain-provider-measure.log`: actual source denoise/measurement,
  retained-reference and ladder workflow passed with the static provider.
- `grav-provider-recipe-unit.log`: four source/inventory/environment/no-clobber
  recipe tests passed.

Windows and macOS native builds and media execution still require their native
runners. Workflow wiring is not a passed native qualification result. NVIDIA
execution remains explicitly deferred by the user and is unrelated to this
CPU grain-provider qualification.
