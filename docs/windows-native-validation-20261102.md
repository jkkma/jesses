# Windows native validation, 2026-11-02

**Current status:** The user resumed work after the pause. See the
[continuation ledger](windows-continuation-20261102.md) for later checks and
remaining acceptance gates. This document retains the earlier chronology; its
historical process and completion statements are not live status.

This is the Windows continuation of [the local handoff](windows-local-validation.md),
on an existing Windows 11 x64 development host. Base checkout:
`2c5dd9d6f636ff05e6fdad030c2f2a3bb007e079`, with uncommitted fixes described below.
This is not FreshImage qualification. No source media or global tool defaults were
changed, and no commit, push, or release publication was performed.

Evidence roots below are relative to the checkout. Keep their JSON, logs, media,
and screenshots together; generated compiler output is not the sole evidence.

- Native: `target/test-artifacts/windows-native/20261102T185541Z-a192fabc/`.
- Desktop: `target/test-artifacts/windows-desktop/20261102T191841Z-0e0fd24a/`.
- Source providers: `target/test-artifacts/windows-providers/`.
- Frontend: `target/test-artifacts/windows-frontend/`.

## Completed checks

| Check | Evidence and limits |
| --- | --- |
| Native console lifecycle | 12/12 actual Win32 cases, `console-qualification/20261102T190644Z-6cbed9570afb48bc8967b8e71dce567e/console-qualification.json`. Includes visible/hidden jobs, literal Unicode argv, pause/resume/cancel, window closure, fixed AV1AN tail, readiness shutdown, and concurrent helper joins. |
| Frontend | Frozen pnpm 11.19.0 install, check/build, and 279/279 Windows Playwright tests; no retries, skips, or flaky cases. Compact metrics layout fixed. |
| Full Rust checks after fixes | `workspace-tests-post-metadata-fix.result.json`: 468 passed, 0 failed, 328 ignored across 73 groups. All-target Clippy with `-D warnings` passed in `workspace-clippy-post-metadata-fix.result.json`. The earlier post-AV1AN attempt was interrupted without an exit result; its receipt is retained and is not counted as passing. Ignored tests require separate explicit execution. |
| Sequence release | `completed-preflight-release-green`: 4/4 native sequence tests. IPC state registration: 4/4. Fresh desktop import of five PNGs, encode to Done, then immediate removal released the exact managed cache; originals, output, and successful history remained. |
| CPU Quick Convert | Actual desktop HEVC encode of a short local AV1 excerpt; 48 frames with requested 2–4 s source-seek semantics. Full video/audio decode, all 11 copied audio/subtitle packet sequences and 13 font attachment hashes verified. See `gui-quick-convert-verification/`. Container duration includes copied subtitle/preroll timing. |
| Image sequence | Desktop import at 30 fps and CPU HEVC encode. Independent full decode: five frames at 0, .033, .067, .100, .133 s; all five original PNGs retained. See `gui-image-sequence-verification/`. |
| Spline and no-Vulkan behavior | `host-spline-physical-vulkan`: CPU x264 output with physical Radeon 880M libplacebo processing. `host-spline-no-vulkan-held`: unchanged Spline rejects unavailable Vulkan, while a reviewed replacement command remains executable. These are explicitly host-development receipts. |
| Source timing/filter tests | `host-source-trim-matrix`: 5/5 native cases for source packet clocks, AAC, ASS, PGS, mixed conversion, and external offsets. `source-provider-filter-trim-images`: 1/1, three trim modes, stateful raw filters, 30 images, and legacy isolation; source FFmpeg plus host x264. The Unix held-command wrapper case was not represented as Windows coverage. |
| Recovery ownership regression | `concat-owned-output-green`: 28/28 focused tests, including prefix parity, traversal rejection, and native NTFS junction rejection. Host and pinned AV1AN use identical relevant path adapter/options emitter; see `av1an-contract-comparison/comparison.json`. |
| Source-cut attachment regression | `source-cut-attachments-green`: 3/3. Valid codec-less font attachments survive; omitted/added attachments, added media, codec/order changes, and font hash tampering are rejected. |
| Cut publication regression | `source-cut-publication-green`: 4/4 source-cut tests, including the Windows writable-handle to immutable-source transition. The red receipt reproduces error 32. |
| AV1AN development retry | The video-only request that previously failed at concat succeeded with source FFmpeg/x264 and host AV1AN 7df934d. Independent decode: 192 frames, 854×480, 24000/1001, BT.709. See `av1an-native-retries/video-only-independent-verification.json`. This does not qualify the pinned packaged engine. |
| AV1AN source-cut retry | The previously failing 25-stream 0–2 s request also succeeded. Independent full decode: 48 video frames and audio; 10 subtitles and all 13 font attachments remain, with exact original attachment hashes/names. Audio validation measured 95,232 source/output samples at 48 kHz. See `av1an-native-retries/anime-trim-independent-verification.json`. Same development provenance as above. |
| Pinned-base desktop AV1AN | The actual GUI 25-stream 0–2 s request completed with source AV1AN 805dad6/private runtime and source media tools. Independent decode: 48 frames at 1920×1080, all 25 streams, exact 13 font attachment hashes/names, and 95,232 matched audio samples. See `av1an-native-retries/pinned-gui-independent-verification.json`. This is a development GUI copy, not the final portable ZIP. |
| Actual Dolby Spline | The original GUI request passed through native JobManager using source FFmpeg/x264, physical libplacebo Spline with RPU, and CPU encoding. `dolby-gui-metadata/independent-verification.json`: all 60 decoded frames BT.709, stale HDR/Dolby metadata absent, exact titles and actual Matroska `und` languages. All 63 copied EAC3 packet payloads and PTS/DTS/durations exactly match a literal source input-seek reference. Video starts 1.016 s, audio 0 s, container 2.017 s because the requested source-seek mode retains preroll. |
| Source-provider audio/subtitles/filters | `core-audio-subtitle-filter-gates` passes all 11 audio cases, raw-filter/trim/30-image case, and three subtitle cases. Its bitmap case exposed a test-fixture clock error: muxing had rebased the authored .250/1.500 s SUP timestamps. With explicit fixture `-copyts` and source-clock assertions, `bitmap-subtitle-authored-clock-green` passes both SDR/HDR variants with the original pixel/placement assertions unchanged. Failed fixture and host/source comparisons are retained; no production renderer change. |
| Source New Settings authority | `new-settings-diagnostic/contract-verification.json` confirms the documented source behavior: queued per-chunk encoder settings remain saved even when global requested preset changes. All 10 queued parameter arrays and the observed unfinished child use the saved veryslow preset; new global filters/concat settings are separate. Four already-completed files/receipts match across the native snapshots. There was no pre-resume hash capture. The native history wording now explicitly names preset/CRF retention. |

## Fixes under validation

- Windowless console helpers call `FreeConsole` before `AllocConsole`, after
  preserving the inherited READY pipe. Cancellation while awaiting READY joins
  helper cleanup and returns Cancelled.
- Windows directory and recovery identity opens use the access/flags required by
  the operation while retaining exclusive lock and reparse protections.
- Managed image sequences close their receipt writer before directory publication,
  compare canonical cache-container identity, and read receipts through the owned
  exclusive lease. Release uses the actual managed `Jobs` IPC state and prunes
  finished preflights under the admission lock; active work still blocks release.
- Windows concat keeps its response-file route. Recovery compares existing output
  directory identities after canonical containment, rejects parent traversal before
  normalization, and rejects intervening links/reparse points. This accommodates
  the pinned engine's deliberate removal of the `\\?\` prefix for MKVmerge.
- Source-cut stream mapping treats codec-less font attachments as required copied
  streams. Existing packet, metadata, attachment hash, and full-decode proofs remain.
- Source-cut publication relinquishes only the completed writable temporary link
  before opening its immutable source guard, then checks the original file identity.
- Explicit Matroska track language `und` accepts FFprobe's deliberate omitted-tag
  representation only when the actual probe format is Matroska/WebM. Literal mux
  commands and independent MKVmerge inspection prove the real track language;
  explicit empty/different languages, changed titles, and unknown/ISO formats remain
  strict. The positive regression was red before this fix.

Red receipts and earlier failures remain retained alongside green results.

## Downloaded HDR fixtures

`hdr-samples-manifest.json` in the native evidence root consolidates absolute paths,
hashes, source URLs, usage notes, and individual proof receipts for three originals:

| Type | Verified media |
| --- | --- |
| HDR10 | ByteDance sample, 5,409,945 bytes, 20.02 s, 3840×2160 HEVC Main10 PQ/BT.2020. All 600 decoded frames contain static mastering and content-light metadata; no Dolby/HDR10+ metadata. |
| HDR10+ | WebM Project HDR Explorer rooftop sample, 7,153,214 bytes, 12.066667 s, AV1 1280×720 PQ/BT.2020. All 362 decoded frames contain SMPTE ST 2094-40 dynamic metadata. The original signals unknown chroma sample position; strict tone-map admission rejects it rather than inventing siting. Sequence-header trace retained. |
| Dolby Vision | Official Dolby GlassBlowing2 P8.1 sample, 344,509,829 bytes, 178.112 s, HEVC Main10 PQ/BT.2020, intact RPU and HDR10-compatible base layer. SHA-256 matches the handoff. |

Original downloads remain intact. The one-frame FFmpeg FATE HDR10+ diagnostic is
retained separately and is not the playable HDR10+ sample. No redistribution grant
is inferred where the publisher supplied no sample-specific terms.

## Remaining qualification

The post-fix workspace/Clippy pass is complete. A subsequent test-only bitmap
fixture correction passed its focused native gate and targeted Clippy. Continue
selected explicit external-tool tests for held edits, pause/Stop, and complete
AV1AN Saved/New Settings recovery with the delivered
providers. `external-test-inventory.json` lists ignored integration cases and their
requirements; Linux wrapper tests need separate Windows treatment.

Fresh source-built FFmpeg 9.0.1 and x264 deliveries have native CPU/decode/import
and physical Spline receipts. Base AV1AN 805dad6 and its private frameserver now
have component receipts; its application-level media/recovery qualification and
remaining scorer/provider merges still follow. AOM and VPX standalone deliveries
also have native receipts. Do not treat host AV1AN or
mixed-provider development runs as final bundled qualification.

`target/windows-core-stage-20261102T2032` stages the ready FFmpeg, x264, mainline
SVT, base AV1AN, MKVmerge, and pinned SVT forks. Sanitized native discovery passes
all eight required roles in `core-package-discovery.result.json`, including the
source-SVT HDR binding. This is a subset stage, not the final distribution.

The app-only release executable is compiling in a fresh target with one Cargo job,
a 10 GiB target ceiling, 45 GiB free-disk floor, and 3 GiB available-RAM floor.
Its verified frontend inputs are recorded in `app-release-frontend-inputs.json`;
media tools remain external runtime resources and can be collected afterward.

After deliveries are complete: stage all tools/sources/licenses, finish the release
desktop executable, collect and verify the actual portable ZIP, run sanitized
package discovery and AV1AN/grain/media gates, execute `RestrictedHost`, and exercise
the packaged GUI and disposable Scoop lifecycle. No portable ZIP, packaged GUI,
Scoop lifecycle, or FreshImage completion is claimed by this checkpoint.

The native test target uses a fixed 10 GiB ceiling and 45 GiB free-space floor.
An earlier 4 GiB stop is retained; there was no 6 GiB budget stop. All receipts and
original media are preserved.

`anime-source-identity-after.json` confirms that the selected original video's
length and SHA-256 still match the pre-validation identity.
