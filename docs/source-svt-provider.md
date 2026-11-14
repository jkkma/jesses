# Source SVT provider and AV1AN target defaults

The source UI has one SVT choice. New canonical `svtAv1` requests use the qualified
HDR provider for the host. Windows uses the MSVC 19.51 build of [SVT-AV1-HDR-PMod 2026-07-20](https://github.com/Patman86/SVT-AV1-Mods-by-Patman/releases/tag/2026-07-20)
from commit `e4b6c4ff5a8c52a03a7cfee49ca89513c4d75cbe`. Linux support is
deferred. Its retained development recipe uses the
[SVT-AV1-HDR 2026-09-01 build](https://github.com/juliobbv-p/svt-av1-hdr/releases/tag/pgo-build-2026-09-01-0033340)
from commit `00333404f455471aaa6ee2c927cac3c93efb76e3`, with HDR10+ and Dolby
Vision integration. Version markers and binary
hashes are in `scripts/svt-forks.json`, and source hashes are in
`scripts/bundled-tool-sources.json`.

Standalone SVT progress includes frames, speed, bitrate, encoded size and
elapsed time as the encoder reports them. For a deterministic frame plan,
the typed pipeline supplies the exact frame count so the encoder also reports
remaining time and estimated size. Unbounded source filters and manually
edited command programs retain end-of-stream behavior.

Discovery checks the canonical and HDR overrides first. Windows checks the verified
package HDR record before the managed installation. The retained Linux route
checks the managed installation before the package. Distinct and canonical PATH
names are the last fallback. A discovered
unqualified provider fails clearly. Explicit `JESSES_SVT_AV1` remains the existing
legacy/custom-provider override and checks the provider's actual CLI capabilities.
The internal `svtAv1Hdr` and `svtAv1FiveFish` identities remain compatible without
adding source UI choices. Package manifests bind `sourceSvtProvider` to the HDR
record; the isolated package probe checks the exact canonical resolved path.

Manual static HDR, Dolby Vision RPU, HDR10+ JSON and managed grain-table resources
use the same guarded typed resource consumers as the compatible internal fork
identities. Scalar arguments are checked against the resolved encoder's help;
accepting the canonical identity does not authorize an unsupported mainline flag.
Historical Linux Quick/AV1AN gates verified every output frame, dynamic metadata resources,
source-only `tx-bias`, `noise-adaptive-filtering`, `hbd-mds`, and the authored
14-point grain table in AV1 bitstream headers.

## Quality zero

Source Direct and AV1AN SVT/x264/x265 emit literal `--crf 0`; AOM/VPX emit literal
zero CQ. These source requests carry `lossless:false`. The separate native
`lossless:true` contract retains its stronger verification. Hardware encoder choices are excluded by the user's later CPU-only policy; their historical quality mapping is not exposed.

On the pinned HDR provider, source SVT CRF0 is valid and lossy: the four-frame
192×128 10-bit fixture decoded to 294,912 bytes with 66,439 bytes different from
the source. The standalone proof and hashes are in
`target/migration-validation/source-svt-20261101/crf-zero-proof.json`.
Both Quick and AV1AN now accept this source value without inventing a lossless
claim. Generated-command tests cover literal zero for all five software codecs
in both backends; the source frontend preserves literal zero for the CPU codec/backend combinations.

## Source target defaults

The source builder (`Nmkoder/UI/Tasks/Av1an.cs:566–605`) emits a scalar target and
metric. It emits the VMAF model and two VMAF threads when applicable, and leaves
search range, probe count/rate/resolution/statistic/encoder arguments to AV1AN.
The additive persisted `sourceTargetDefaults:true` applies this behavior to source
AV1AN requests while preserving the historical explicit native target API.
Stale hidden target draft values do not affect validation, availability, resource
planning, score evaluation, emitted arguments or recovery receipts.

The pinned engine defaults are:

| Item                      | Effective source behavior                                                  |
| ------------------------- | -------------------------------------------------------------------------- |
| Encoder search            | SVT15–50; AOM/VPX15–55; x264/x26515–35                                     |
| Probe count/rate          | Four probes; every frame                                                   |
| Probe encoding resolution | Input size (`probe_res:null`)                                              |
| Metric evaluation bounds  | VMAF/FFmpeg XPSNR1920×1080, aspect ratio preserved; VS metrics input size  |
| Probe video parameters    | Engine defaults (`video_params:null`, `params_copied:false`)               |
| Statistic                 | Automatic                                                                  |
| Scalar tolerance          | Exact engine ±1%; e.g.35→34.65–35.35, not an explicit integer-tenths range |

The successful actual weighted-XPSNR gate deliberately supplied invalid/stale
hidden bounds/probe values. Its captured native queue is
`target/migration-validation/source-svt-20261101/actual-target-queue.json` and
engine log is `actual-target-engine.log` beside it. The fixture source and probe
encoding are192×128; FFmpeg metric evaluation uses1920×1080 bounds and actually scales this
3:2 fixture to1620×1080. The retained probe FFprobe result and showinfo trace
are `actual-probe-dimensions.json` and `actual-evaluation-dimensions.log` beside
the queue proof. Completion
means the source search workflow finished, not a guarantee that a bounded search
hits its requested score.

The source omits `--vmaf-filter`. The stamped internal
`source_unfiltered_probes` switch now prevents the compatibility engine from
inheriting final FFmpeg transforms into source probe encodes. An actual target
run keeps the original192×128 probes/reference while final chunks produce a
160×128 crop. Captured queue/probe/evaluation receipts establish the distinction;
source target probing deliberately does not measure the later transformed final
picture. The source transformed-target gap is closed by that behavior. Legacy
explicit target requests retain transformed probes. The subsequent arbitrary
raw-filter/per-chunk consumer and recovery proofs are in
[source custom filters](source-custom-filters.md).

## Intentional engine correction and recovery

The exact upstream engine pin is commit
`805dad69143fa0a81cfe2fb89c0b9e90a828ea72` (source SHA-256
`7f570da8fe0ba5970cbf04d882b48e442e04d1df5ec180e12d9ff974450dfca2`).
Its unsigned XPSNR stats regex rejected valid negative average scores and silently
skipped negative frame scores. `scripts/patch-package-av1an.py` now accepts signed
values and adds a deterministic negative-frame/negative-average regression and
`xpsnr-signed-stats-v1` version marker. The marker is locked in the package recipe
and retained in build provenance. This is an intentional engine bug correction,
not a claim of byte-identical unpatched upstream behavior. Runtime availability
rejects every-frame XPSNR engines without that fix.

Saved jobs retain frozen settings, provider roles, arguments and executable
content stamps. New canonical provider selection does not authorize a different
provider on saved-settings reuse. Byte-identical tool relocation remains allowed;
changed encoder/engine bytes fail. Explicit new-settings restart can deliberately
create a new recipe after its existing recovery checks. Metric/probe cache ownership now recognizes only the pinned engine's exact
numbered names for the frozen metric, validated complete scene map and effective
quantizer bounds. Quiescent caches and their separate prequeue scene authority
are sealed only after the entire engine tree exits. Existing sealed-file changes
fail before cleanup; a missing disposable cache can be regenerated. Saved/New
resume removes recognized disposable probes before launching the engine, retains
completed encoded chunk authority, and never trusts cached probe bytes as an
executable receipt. Unknown files, out-of-range names and symlinks remain errors.

The real32-frame Saved/New gate cancels during an active metric before the queue
is saved, reopens history, rejects changed cache bytes and foreign files without
deletion, then completes both resumes and cleans the owned workspace. Deterministic
tests additionally cover wrong metric, bad index/quantizer/extension, canonical
number spelling and outside symlinks. The saved-job provider proof at
`legacy-provider-recovery/proof.json` retains five completed chunks and refuses
changed engine bytes with `RECOVERY_INVALID`; no output is published.

## Evidence and platform limits

Linux real media: `source_svt_jobs`5/5 (CRF0, manual HDR/scalars, managed FGS table,
source target defaults, canceled target Saved/New recovery), nine verified output
encodes. `source-native-final.log` records the final gate. AV1AN native library
regression62/62 (two ignored); source unit filter48/48; deterministic patched-engine
parser1/1; package recipe8/8.
Logs are under `target/migration-validation/source-svt-20261101`.

The durable tools are `target/toolchains/source-svt-qualified-20261101`, the
qualified FFmpeg9 delivery and the R79 provider environment. The AV1AN binary,
updated patch/build-input hashes and prior executable receipt remain under
`target/toolchains/av1an-linux-20261101`. No global install is changed.
The source bundle supplies Linux/Windows assets; the checked source recipe has
no prebuilt macOS SVT-HDR asset. This Linux proof does not claim actual macOS or
Windows execution, nor an NVIDIA hardware pass.
