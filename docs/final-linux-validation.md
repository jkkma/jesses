# Final Linux validation — 2026-11-02

Jesses uses CPU video encoders. Hardware encoding is unavailable; legacy records
remain readable and active hardware requests fail explicitly. Vulkan/libplacebo
Spline processing and GPU-assisted metrics remain supported. The source reference
is Nmkoder `a7e9731`; the final pushed Jesses revision is reported in the handoff.

## Latest bounded finishing pass

All checks below passed using the final tx3g header handling. Commands used
`--locked --offline`, the normal test-thread stack, and one isolated target:

```sh
export CARGO_TARGET_DIR="$PWD/target/final-small-checks-20261102"
export CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
unset RUST_MIN_STACK
cargo test -p media-runtime --lib --locked --offline container::tx3g::tests -- --nocapture --test-threads=1
cargo test -p media-runtime --lib --locked --offline -- --test-threads=1
cargo test -p media-runtime --test source_mapped_stream_jobs --locked --offline -- --ignored --nocapture --test-threads=1
cargo test -p media-runtime --test source_utility_receipts --locked --offline -- --ignored --nocapture --test-threads=1
cargo test -p media-runtime --test source_vp9_iso_chroma --locked --offline -- --ignored --nocapture --test-threads=1
cargo test -p media-runtime --test source_av1an_rotated_cut_jobs --locked --offline -- --ignored --nocapture --test-threads=1
cargo test -p media-runtime --lib --locked --offline source_queue_concat::tests -- --ignored --nocapture --test-threads=1
cargo clippy -p media-runtime --lib --tests --locked --offline -- -D warnings
```

| Check                                                                         | Result                                         |
| ----------------------------------------------------------------------------- | ---------------------------------------------- |
| tx3g header positives and malformed/style/font/rate/order negatives           | 3 passed                                       |
| Complete runtime library                                                      | 407 passed, 32 opt-in tests ignored; 439 total |
| Primary/external mapping, MOV data and MP4/MOV subtitle routes                | 6 passed                                       |
| Original-image sequence bitrate/chart and measured utility receipts           | 3 passed                                       |
| VP9/MP4 missing-only chroma declaration after complete packet proof           | 1 passed                                       |
| Rotated AV1AN copied cut, literal-engine pixels and untouched source packets  | 1 passed                                       |
| Sealed-chunk concat: seven selected-method routes, negatives and pause/cancel | 3 passed                                       |
| Strict runtime library/test Clippy                                            | Passed, no warnings                            |

The 32 ignored library tests are explicit opt-in media/provider/process gates.
The table enables the relevant concat gates separately; the broader historical
phase receipts remain scoped evidence, not a claim that every ignored test ran.
The tx3g rules preserve the complete base/style/font/unknown-atom bytes and full
subtitle packet/cue/timestamp proof. Only identical valid bitrate-box repeats,
or the first exact trailing bitrate hint bound to an unchanged MOV input's
sample-table bitrate, can differ. See [mapped streams](source-mapped-streams.md).

A watchdog checked target allocation and partition free space every 0.25 seconds,
stopping early at 1.90 GiB toward a 2 GiB budget or below 45 GiB free. Peak target
allocation was **839,733,248 bytes (0.782 GiB)**. The entire isolated target was
removed after verification; **55,930,159,104 bytes (52.089 GiB)** remained free.
No old Linux or Windows debug target was recreated. Local logs, result JSON and
cleanup receipts remain under `target/test-artifacts/final-small-checks/`; these
ignored artifacts do not travel in a fresh clone.

## Earlier aggregate and focused evidence

These results preceded the final narrow tx3g extension; they are retained with
that timing rather than presented as a fresh full-workspace rerun:

- `cargo test --workspace --locked`: **453 passed, 331 intentionally ignored**
  across 73 result groups, using the normal thread stack. The stale unit that
  rejected source-valid CRF 0 was corrected to reject out-of-range 64.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed before
  the last subtitle-header helper and its tests. The latest runtime Clippy is
  independently listed above.
- Command editing: 10 passed; copied trim: 6 passed; output metadata: 3 passed.
- Package/CPU/IPC script checks: 23 scripts and 130 cases passed. The later
  CPU-policy check passed 5/5, including the added CPU-only developer example.
- Contract export comparison passed; the finishing changes add no IPC fields.
- Final Chromium suite: **279/279 passed**, Svelte check **0 errors/0 warnings**,
  and production build passed with Vite's bundle-size advisory. The permanent
  layout regression covers **800×750**, 800×768 and 1360×850. Conditional HDR
  controls and the completed footer fit at the exact 800×750 size. This is
  mocked-browser evidence, not native Windows rendering or media execution.

The source AV1AN New/Saved and typed-FPS work is closed locally:
`native-source-new-final.log` passes 2/2 in 225.07 s; typed 24→30 fps produces all
120 frames with matching literal-engine pixels and timestamps; frozen count-policy,
raw-FPS mismatch failure, partial-copy cancellation/same-process retry and an
actual kernel write-failure cleanup all pass. Saved Finalizing passes without
rerunning chunk encoders. The detailed contracts and deliberate x265+MP4 source
bug correction are in [source chunks](source-av1an-chunks.md) and
[recovery actions](source-av1an-recovery-actions.md).

## Qualified local tool environment

The retained Linux tool layout used for the commands above is:

```sh
. target/toolchains/qtgmc-linux-20261101/env.sh
export JESSES_FFMPEG="$PWD/target/toolchains/ffmpeg-vulkan-20261101/delivery/ffmpeg"
export JESSES_FFPROBE="$PWD/target/toolchains/ffmpeg-vulkan-20261101/delivery/ffprobe"
export JESSES_AV1AN="$PWD/target/toolchains/av1an-linux-20261101/target/release/av1an"
export JESSES_SVT_AV1_HDR="$PWD/target/toolchains/source-svt-qualified-20261101/resources/tools/svt-av1-hdr/SvtAv1EncApp"
export JESSES_SVT_AV1_5FISH="$PWD/target/toolchains/source-svt-qualified-20261101/resources/tools/svt-av1-5fish/SvtAv1EncApp"
export JESSES_TEST_TOOL_RESOURCES="$PWD/target/toolchains/source-svt-qualified-20261101/resources"
export PATH="$PWD/target/toolchains/ffmpeg-vulkan-20261101/delivery:$PATH"
```

The environment script supplies the qualified R79 runtime, plugin/config paths
and Python. Source-built HDR/5Fish SVT providers are selected explicitly;
`JESSES_SVT_AV1` is reserved for an intentional legacy mainline override. The
host also has x264, x265, AOM, VPX, mkvmerge and Grav1Synth. A fresh checkout must
build/install providers from the linked [packaging recipes](packaging.md); the
local tool paths and media are not committed.

## Native platform limits and disk safety

MSVC `cargo check -p media-runtime --tests --target x86_64-pc-windows-msvc` and
the `console-fixture` example check passed earlier. They establish compilation
only and predate the final tx3g helper extension. **No native Windows execution
or Windows package qualification is claimed.** Native macOS qualification is
also separate. Continue with [the Windows handoff](windows-local-validation.md),
including the twelve-case real-console fixture, desktop, CPU providers,
HDR/Vulkan, pause/recovery and portable-package checks.

The initial full debug-symbol workspace build exhausted this laptop's disk.
A later watchdog stopped safely before another exhaustion. Generated Linux and
MSVC debug directories were then deliberately removed, restoring 45.624 GiB;
source, providers, source caches, downloaded media and evidence were preserved.
Do not repeat an unrestricted debug-symbol build by default. Check free space,
use an isolated bounded qualification target and suitable debug-info settings,
monitor its allocation, and remove generated outputs after preserving receipts.
