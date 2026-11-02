# Nmkoder migration parity ledger

**Current user scope: CPU encoding only.** The explicit latest request removes all hardware video encoding, including NVENC, from Jesses. Legacy hardware job records remain readable but active hardware encoding requests must be rejected; packet copy/imported media remains supported. Vulkan processing, libplacebo Spline tone mapping, and GPU-assisted metrics/scoring are explicitly preserved. Historical NVIDIA receipts are evidence of past exploration, not a current capability or pending qualification.

This ledger uses the local `jkkma/nmkoder` fork at `a7e9731` and Jesses at baseline
`d088796`. It records behavior, including the fork's inherited workflows, rather
than treating file-for-file copying as migration.

## Coverage denominator and completion rule

The README source inventory contains **103 claim units**: all **46** bullet
claims in Features (source lines 89–238) and all **57** bullet claims in What's
new since the fork (lines 249–551). Multi-sentence bullets remain one traceable
source unit; their named subfeatures are included in the listed implementation.
The full grain-usage prose at source lines 360–390 is covered by the grain rows.
Packaging and compatibility details at source lines 555–761 are tracked below.
Additional baseline flows found in code are tracked separately.

`Existing` means current code and prior checked-in evidence were identified; it
does not mean this session reran its hardware/native qualification. `In progress`
and `Investigating` remain open. A row becomes implementation-complete only when
its behavior, UI and focused checks agree. No percentage is reported until every
functional gap and deliberate replacement has an explicit disposition. Platform
or GPU qualification must never be counted as passed without execution evidence.

## Status and remaining qualification — 2026-11-02

This section and the latest linked phase receipts supersede older dated
checkpoints and earlier “pending” statements later in this ledger. The current
checkout retains the source-shaped UI and CPU-only video encoders. Vulkan and
libplacebo Spline remain available for processing. Native Windows execution has
not been performed in this Linux session; compilation for the MSVC target is
not a native run. The final revision and Windows test instructions belong in
[the Windows handoff](windows-local-validation.md).

The Linux source-workflow checks now cover direct and copied trim, the MOV_TEXT
overlap/finalization case, raw filters through image/QTGMC and recovery paths,
image-sequence packet/bitrate analysis, measured utility outputs, VP9/MP4 chroma
proof, AV1AN None split defaults and source-matched Windows console implementation.
They do not by themselves qualify Windows. The source Dolby matrix now records
13 cases, including three trim modes, captions, rotation, VFR, literal calibrated
CPU fallback and two exact AV1AN packet comparisons; see
[source Dolby combinations](source-dolby-combinations.md). Source AV1AN New/Saved queue retention, typed frame rate and Finalizing resume
are now qualified locally, including cancellation/rollback and independent
sealed-chunk concat proof; see [source AV1AN chunks](source-av1an-chunks.md).
The Linux CPU Select rotated copied-cut case is qualified against the literal
source engine, including first-frame pixels and complete copied packets;
see [source AV1AN copied cut](source-av1an-copied-cut.md).
The [final Linux validation](final-linux-validation.md) records exact commands,
counts, resource limits and the distinction between current bounded checks,
earlier aggregate evidence and native Windows work still to run.

## Earlier checkpoint and context

Earlier `/tmp/jesses-toolchain/` evidence paths below describe the pre-shutdown
session and may no longer exist after reboot. The checkpoint under
`.git/checkpoints/20261101T083132Z` retains its validation logs; the tool binaries
themselves were not backed up there. Current phase evidence is linked separately.

The user's latest instruction is authoritative: **the fork's exact controls,
settings, option lists, defaults and screen behavior, with paper-and-ink
theming and compact two-column / three-column layouts**. Earlier visual-theme
preferences are superseded; earlier intentional control/default substitutions remain revoked.
The source UI inventory now covers 149 main-window controls/actions and all
18 auxiliary windows: `docs/nmkoder-ui-parity.md` and
`docs/nmkoder-dialog-inventory.md`. This expanded denominator is in addition to
all 103 README claims; the README alone is not a complete screen specification.

- Source-faithful shell/encoding/utility UI is implemented. The old frontend
  counts below are historical; the current full browser run and visual audit are
  recorded separately in [the current UI audit](source-ui-current-audit.md).
- Quick/AV1AN output uses source-style extensionless stems and numbered
  collision resolution; image and encode batches use source templates and
  per-source/default destinations. Fixed-image caption selection is wired for
  primary/external and checked/unchecked subtitle tracks. Focused browser and
  Linux native naming evidence is in `docs/source-output-image-ui.md` and
  `docs/source-batch-naming.md`; Windows/macOS native paths remain unqualified.
- The resize dialog's mode/fill/SAR/alignment/reset matrix and trim frame,
  preview, stale-keyframe and close-race behavior have focused mocked-media
  browser checks. They do not establish every native pixel/rotation/platform
  case. The 800/1360 layouts retain the requested compact paper-and-ink style.
- Shift+Run editable command UI and held review now have Linux real-media
  qualification: the command suite passed 11/11, and the scoped image suite
  passed 34/34, including edited execution and cancellation. See
  `docs/source-command-editing.md` and `docs/source-image-command-edit.md`.
  Windows/macOS native execution remains unqualified. Image Pause/Resume has
  mocked UI coverage and Linux process-state proof; see `docs/source-image-pause.md`.
- Source batch preview and naming have real-media 7/7 coverage for 101 inputs,
  mixed video/audio/subtitle-only output and cancellation during a live probe.
  Copy mapping now retains multiple checked primary/external video tracks,
  with focused UI and 2/2 native proof. MOV data has focused real packet/order proof; hardware encoding is excluded
  by the latest user scope; see `docs/source-encode-contract-gaps.md`.
- New source Settings persist with exact defaults and bounded validation.
  Auto-crop consumes its saved sample count; encoder GOP and final MP4 faststart
  consume an immutable `encodingPreferences` snapshot captured at new native
  preview/admission. Existing frozen requests and resume/history paths are never
  rewritten from mutable preferences. Zero-index display/reset categories have
  frontend consumers. The Windows console consumers are implemented and have
  cross-compilation and Linux policy/supervisor checks; their actual windows and
  process behavior still require the [native Windows fixture](windows-local-validation.md).
- Exact Quick time trim input-seek/decode-from-start strategies, Spline/Area
  resamplers, optional source modification times, selected attachment extraction
  and automatic utility output naming are implemented. Expanded real workflows passed **2/2** (10.46s) in
  `/tmp/jesses-toolchain/source-ui-controls-real.log`, including actual crop
  count3, automatic progressive skip, explicit selected-preset QTGMC readiness
  and its explained fallback, and actual auto-named 72-frame BWDIF output.
  Latest `cargo test -p media-runtime --lib source_ --locked` passed32 with
  3 installed-tool ignores (`/tmp/jesses-toolchain/source-controls-focused-unit.log`).
  Settings/GOP/filter unit tests passed 2/2, automatic collision-name test 1/1,
  all-eight QTGMC preset script mapping 1/1, and deinterlace decision unit 1/1.
- Automatic Utility Deinterlace now uses actual field detection: progressive or
  undetermined input skips only in automatic mode; explicit modes remain forced
  with unknown fields treated top-first. Selected QTGMC preset qualification
  falls back to BWDIF with its reason, matching `Media/Deinterlace.cs:24–91`.
  Native `cargo check -p jesses --lib --locked` passed (17.22s); IPC registration
  tests passed3/3. Logs: `/tmp/jesses-toolchain/source-native-settings-check.log`
  and `source-ipc-registration.log` in that directory.
  Both missing source QTGMC presets (Placebo, Very Fast) have exact provider names.
  The restored Linux R79 provider positively rendered all 16 output frames for
  each of the eight presets, recorded in
  `target/toolchains/qtgmc-linux-20261101/provenance/build-logs/qualification.log`.
  This result does not establish Windows or macOS qualification.
- Source format and console entrypoints are described in the phase handoffs;
  the Windows console implementation is ready for native qualification. Source-pinned grav1synth IVF/film-stock
  qualification is now positive on Linux; native Windows/macOS checks remain. All 27 source software ladder pixel-format choices now have
  actual retained-decode/metric/no-clobber proof; hardware-encoder menu choices
  are removed by the latest CPU-only user requirement. Cut/grain auto
  outputs now preserve compatible input containers; six cut families and three
  grain families passed actual packet/metadata/frame/source/no-clobber checks.
- Linux image caption qualification covers text and bitmap captions, VFR and
  shifted-origin clocks, cancellation and historical full-source image exports.
  The historical image-sequence suite passed 4/4 with the restored qualified
  FFmpeg 9 provider (`target/migration-validation/image-native-20261101/qualified-image-sequence.log`);
  the earlier FFmpeg 6.1 metadata rejection remains a provider-specific result.
  All six fixed-image caption gates, including actual QTGMC text/bitmap output,
  passed with the restored Linux provider (`target/migration-validation/
image-native-20261101/qualified-image-captions.log`). See
  `docs/source-image-caption-validation.md`. Full-source quality metrics, YADIF,
  and selective/in-place Color Data remain tracked in their respective evidence.
- Typed dynamic/static HDR catalog rows are implemented with positive native,
  FFmpeg and single/multichunk AV1AN proofs, including actual per-frame RPU/HDR10+
  and MP4/MKV metadata verification. See `docs/encoding-parity.md`; automatic
  preservation alone was not counted as the six manual source controls.
- Portable SSIMULACRA2/Butteraugli actual ladder adapter passed both metrics with
  pinned L-SMASH/vszip/julek and retained 24-frame clips. Proof:
  `/tmp/jesses-toolchain/vs-plugins-linux/qualification.json`. Production FFmpeg
  passed all 7 wrapper/model/package smokes. Six actual Jesses AOM/SVT VMAF
  standard/NEG/4K workflows passed with retained samples and no-clobber proof:
  `/tmp/jesses-toolchain/final-av1-ladder-models-lp1.log`.
- Source-normalization MPEG-TS audio-origin/duration review findings are fixed
  with a real 48-frame/exact 10 ms audio-lead regression. Initial six ordinary
  unit failures were resolved (17 core + 338 runtime passed at that checkpoint).
  The later missing-x264 fixture/precedence regression is repaired by its owner.
  The final browser run is recorded in the current UI audit; workspace and
  platform checks are tracked separately from this earlier checkpoint.

**Hardware video encoding is excluded by explicit user request.** Historical
NVIDIA command/provider receipts describe removed capability, not pending work.
Native hosted platform/package checks remain separate from local tests.

## Technical replacements and source authority

- Tauri/Rust/Svelte replaces Avalonia/.NET. Media outcomes and source user-facing
  controls/defaults remain in scope; the user now requests paper-and-ink styling with the compact source layout.
- Source audio defaults are Opus/stereo, not the old Jesses copy default. Main
  source defaults and conditional control lists are pinned in the UI inventory.
- Typed requests, immutable internal queue entries, source protection, atomic
  no-clobber publication, complete output checks and verified recovery remain
  internal integrity requirements; they do not justify extra source UI settings.
- Existing input restrictions are not automatically intentional replacements.
  Known VFR/rotation/nonzero-origin/frame-rate/dimensional workflows require
  concrete implementation and evidence, as recorded by their owning phase.

## Current reconciliation — 2026-11-01

The 103 rows below are individually retained. Their **State** cells have been
reconciled against the linked phase evidence; an unchanged “Existing; current
rerun pending” is an implementation pointer, not a fresh test pass. The 149
main controls and 18 auxiliary-window mappings are a separate denominator; see
[current UI audit](source-ui-current-audit.md). The historical 252-pass/one-pending
run predates the copied-trim fix. The final Linux browser rerun passed 279/279;
that mocked-browser result does not establish native Windows behavior.

The 2026-11-02 status above supersedes this earlier reconciliation's open-work
list. In particular, direct overlapping MOV_TEXT finalization now has positive,
mutation and faststart evidence in [source subtitle trim](source-subtitle-trim.md),
and the source-compatible Dolby combination cases are in
[source Dolby combinations](source-dolby-combinations.md). The final Linux results and their snapshot limits are recorded in
[final Linux validation](final-linux-validation.md). Actual Windows desktop, provider, console and package checks are
the next platform handoff, not an implied pass from a Linux or cross-build.

## Source claim inventory

Paths below are repository-relative to Jesses. Source line numbers refer to
`../nmkoder/README.md`. Evidence links are references, not fresh pass assertions.

| ID  | Source line | Claim                                                                                                | State                                                                                                         | Implementation / evidence                                                                                                                                                                                                 |
| --- | ----------: | ---------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| G01 |          89 | Supports all formats that ffmpeg can decode                                                          | Investigating known input gaps                                                                                | crates/media-runtime/src/analysis.rs; crates/media-runtime/src/jobs/encode_plan.rs; tests/fixtures/analysis-validation.md                                                                                                 |
| G02 |          90 | Either use "Muxing Mode" to convert a single file or merge multiple files into one, or               | Source batch/naming and mapped tracks qualified; remaining routes/platforms open                              | crates/media-core/src/jobs.rs; crates/media-core/src/container.rs; crates/media-runtime/src/jobs/encode.rs                                                                                                                |
| G03 |          92 | Supports image sequence inputs (PNG/WEBP/JPEG/BMP) without requiring sequential filenames            | Linux sequence import/export and original-packet bitrate/chart analysis qualified; Windows remains open       | crates/media-runtime/src/jobs/images.rs; tests/fixtures/README.md; crates/media-runtime/tests/image_jobs.rs                                                                                                               |
| G04 |          94 | Drag and drop anywhere in the window, an "Add Folder" button, and a recent-files list                | Existing; current rerun pending                                                                               | src/App.svelte; crates/media-runtime/src/batch.rs; tests/frontend/workspace.spec.ts                                                                                                                                       |
| G05 |         101 | View codec, language, title and more (depending on stream type) of the selected media stream         | Existing; current rerun pending                                                                               | src/lib/components/shared/TrackLayout.svelte; src/lib/components/shared/ExternalTracks.svelte; tests/fixtures/mux-validation.md                                                                                           |
| G06 |         102 | Enable or disable streams with checkboxes - disabled streams are not included when encoding/muxing   | Source selected-track UI and mapped-copy native checks pass; other routes remain open                         | src/lib/components/shared/TrackLayout.svelte; src/lib/components/shared/ExternalTracks.svelte; tests/fixtures/mux-validation.md                                                                                           |
| G07 |         103 | Re-order streams, and set the default audio and subtitle track                                       | Existing; current rerun pending                                                                               | src/lib/components/shared/TrackLayout.svelte; src/lib/components/shared/ExternalTracks.svelte; tests/fixtures/mux-validation.md                                                                                           |
| G08 |         110 | Video Formats: H.264 (x264 or NVENC), H.265 (x265 or NVENC), VP9 (vpxenc), AV1 (SVT-AV1 or AOM)      | CPU encoders exposed; all hardware video encoders intentionally excluded                                      | docs/av1an-svt-x264.md; docs/standalone-encoders.md; docs/cpu-only-encoding.md                                                                                                                                            |
| G09 |         111 | The re-encoders are the standalone encoder binaries — the same x264, x265, SvtAv1EncApp, aomenc      | Source SVT provider pinned/exercised; other routes retain linked evidence                                     | docs/av1an-svt-x264.md; docs/standalone-encoders.md; tests/fixtures/av1an-options-validation.md; crates/media-runtime/tests/source_format_jobs.rs                                                                         |
| G10 |         116 | Image Formats: Animated GIF, PNG Sequence, JPEG Sequence                                             | Linux image 38 gates and source-safe implicit nonvideo suppression; other platforms open                      | crates/media-runtime/src/jobs/images.rs; tests/fixtures/README.md; crates/media-runtime/tests/image_jobs.rs                                                                                                               |
| G11 |         117 | Audio Formats: AAC, Opus, Vorbis, E-AC-3, MP3, FLAC, globally or configured per track                | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/audio.rs; crates/media-runtime/src/loudness.rs; tests/fixtures/audio-validation.md; tests/fixtures/loudness-validation.md                                                                   |
| G12 |         118 | Text-based Subtitle Formats: Mov_Text for MP4/MOV, SRT for MKV, WebVTT for WEBM                      | Linux direct overlapping MOV_TEXT and AV1AN WebVTT/MOV_TEXT qualified; Windows remains open                   | crates/media-runtime/src/jobs/subtitles.rs; tests/fixtures/subtitle-validation.md                                                                                                                                         |
| G13 |         119 | All media types also have the option to strip (remove) or copy (mux without re-encoding)             | Quick Copy trim and selected primary/external mappings qualified; other routes remain open                    | crates/media-core/src/jobs.rs; crates/media-core/src/container.rs; crates/media-runtime/src/jobs/encode.rs                                                                                                                |
| G14 |         120 | Set metadata (title and language) for each track, and copy metadata or chapters from any loaded file | Source metadata UI and 34 real outputs qualified; other platforms remain open                                 | src/lib/components/shared/TrackLayout.svelte; src/lib/components/shared/ExternalTracks.svelte; tests/fixtures/mux-validation.md                                                                                           |
| G15 |         121 | Encoder Options: set quality and speed/effort aka preset, set colour format                          | Existing; current rerun pending                                                                               | docs/av1an-svt-x264.md; docs/standalone-encoders.md; tests/fixtures/av1an-options-validation.md; crates/media-runtime/tests/source_format_jobs.rs                                                                         |
| G16 |         122 | Advanced encoder arguments on a tab of their own, the same grid the AV1AN tab has: every             | 375 source rows: 297 CPU scalar, 11 typed resources, 67 hardware exclusions                                   | src/lib/content-presets.json; src/lib/components/shared/AdvancedEncoderOptions.svelte; crates/media-runtime/src/jobs/parameters.rs                                                                                        |
| G17 |         128 | Quality Modes: a constant quality, a target bitrate, or a target filesize                            | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/rate_control.rs; tests/fixtures/rate-control-validation.md                                                                                                                                  |
| G18 |         129 | Video Options: resample the frame rate, resize from presets or an exact size, manually or            | Source geometry/trim UI and focused checks pass; real combined/platform cases remain                          | crates/media-runtime/src/jobs/encode_plan/framing.rs; src/lib/components/shared/FramingOptions.svelte; src/lib/components/shared/SourcePreview.svelte; tests/fixtures/framing-validation.md                               |
| G19 |         132 | HDR to SDR tone mapping for PQ and HLG sources, with the roll-off built around the picture's         | Linux source Dolby matrix 13 cases including CPU fallback, Spline, trim, captions, VFR and rotation           | crates/media-runtime/src/jobs/encode_plan/tone_map.rs; tests/fixtures/tone-map-validation.md                                                                                                                              |
| G20 |         135 | Deinterlacing, its row shown only for files whose fields warrant it - a tape, DVD or camcorder       | Field detection and utility QTGMC fallback checked on Linux; native platforms remain                          | crates/media-runtime/src/analysis.rs; crates/media-runtime/src/jobs/encode_plan.rs; tests/fixtures/analysis-validation.md                                                                                                 |
| G21 |         139 | Film grain synthesis for the AV1 encoders, the same row as the AV1AN tab — the encoder's own         | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/av1an/grain.rs; crates/media-runtime/src/utilities/grain_measure.rs; crates/media-runtime/tests/utility_jobs.rs                                                                             |
| G22 |         143 | Audio Options: set quality and channels/layout                                                       | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/audio.rs; crates/media-runtime/src/loudness.rs; tests/fixtures/audio-validation.md; tests/fixtures/loudness-validation.md                                                                   |
| G23 |         144 | Loudness normalization to a standard target (-14 / -16 / -23 LUFS, EBU R128), measured per track     | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/audio.rs; crates/media-runtime/src/loudness.rs; tests/fixtures/audio-validation.md; tests/fixtures/loudness-validation.md                                                                   |
| G24 |         147 | Subtitle Options: optionally burn in a subtitle track                                                | Linux image captions, Direct ASS matrix and overlapping MOV_TEXT finalization qualified                       | crates/media-runtime/src/jobs/subtitles.rs; tests/fixtures/subtitle-validation.md                                                                                                                                         |
| G25 |         154 | Encode video using [av1an](https://github.com/rust-av/Av1an) and supported encoders                  | Existing; current rerun pending                                                                               | docs/av1an-svt-x264.md; docs/standalone-encoders.md; tests/fixtures/av1an-options-validation.md; crates/media-runtime/tests/source_format_jobs.rs                                                                         |
| G26 |         155 | Video Formats: AV1 (SVT-AV1 or AOM), H.265 (x265), H.264 (x264), VP9 (VPX)                           | Existing; current rerun pending                                                                               | docs/av1an-svt-x264.md; docs/standalone-encoders.md; tests/fixtures/av1an-options-validation.md; crates/media-runtime/tests/source_format_jobs.rs                                                                         |
| G27 |         156 | Quality Modes: a constant quality, or target a VMAF, SSIMULACRA2, Butteraugli or                     | Existing; current rerun pending                                                                               | docs/av1an-svt-x264.md; docs/standalone-encoders.md; tests/fixtures/av1an-options-validation.md; crates/media-runtime/tests/source_format_jobs.rs                                                                         |
| G28 |         160 | The same framing options as Quick Convert, trim included. Audio is one setting for every track       | Linux AV1AN copied-cut trim 9/9 and rotated CPU Select proof; Windows remains open                            | crates/media-runtime/src/jobs/encode_plan/framing.rs; crates/media-runtime/src/jobs/trim.rs; crates/media-runtime/src/jobs/audio.rs; crates/media-runtime/tests/framing_jobs.rs; crates/media-runtime/tests/audio_jobs.rs |
| G29 |         167 | Deinterlacing too, its own setting, though without QTGMC: av1an filters each chunk with ffmpeg,      | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/qtgmc.rs; crates/media-runtime/src/utilities/temporal_exports.rs; tests/fixtures/temporal-validation.md                                                                                     |
| G30 |         173 | HDR to SDR tone mapping as well, with the colour the encoder is told about following the             | Linux P8.1 Spline and source AV1AN CPU compatible-base timing/packet cases qualified                          | crates/media-runtime/src/jobs/encode_plan/tone_map.rs; tests/fixtures/tone-map-validation.md                                                                                                                              |
| G31 |         182 | An encode that stays HDR keeps its brightness metadata: the mastering display and the content        | Existing; current rerun pending                                                                               | docs/av1an-svt-x264.md; docs/standalone-encoders.md; tests/fixtures/av1an-options-validation.md; crates/media-runtime/tests/source_format_jobs.rs                                                                         |
| G32 |         185 | Film grain synthesis for AV1 (the row is disabled for H.264/H.265/VP9, which have none): the         | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/av1an/grain.rs; crates/media-runtime/src/utilities/grain_measure.rs; crates/media-runtime/tests/utility_jobs.rs                                                                             |
| G33 |         191 | Advanced encoder arguments in a grid grouped by category, each with a full explanation and           | 375 source rows: 297 CPU scalar, 11 typed resources, 67 hardware exclusions                                   | src/lib/content-presets.json; src/lib/components/shared/AdvancedEncoderOptions.svelte; crates/media-runtime/src/jobs/parameters.rs                                                                                        |
| G34 |         193 | Av1an Options: change the splitting method, chunk creation method, number of workers, and more       | None split, New/Saved queue retention and typed FPS qualified on Linux; native Windows pending                | docs/av1an-svt-x264.md; docs/standalone-encoders.md; tests/fixtures/av1an-options-validation.md; crates/media-runtime/tests/source_format_jobs.rs                                                                         |
| G35 |         194 | Encodes can be paused and resumed live, or stopped entirely and picked up again later from the       | Linux recovery and image-pause receipts; Windows native lifecycle remains                                     | crates/media-runtime/src/supervisor/pause.rs; crates/media-runtime/src/jobs.rs; tests/fixtures/pause-validation.md                                                                                                        |
| G36 |         202 | Utilities are "shortcuts" for actions that normally require long (and/or multiple) CLI commands      | All 11 cards mapped; source sequence analysis and measured utility output receipts qualified on Linux         | crates/media-runtime/src/utilities.rs; crates/media-core/src/utilities.rs; docs/media-utilities.md                                                                                                                        |
| G37 |         203 | Read Bitrates: calculates stream size and average bitrate for each stream                            | Original image-sequence packet analysis qualified on Linux with registered frame rate                         | crates/media-runtime/src/bitrate.rs; tests/fixtures/bitrate-validation.md                                                                                                                                                 |
| G38 |         204 | Sample Encodes (CRF ladder): answers "what CRF for this source" by trying it, in minutes rather      | Implemented; AOM/SVT three VMAF models, presets and retained clips passed; hardware encoders excluded by user | crates/media-runtime/src/utilities/ladder_options.rs; src/lib/content-presets.json; crates/media-runtime/tests/utility_jobs.rs                                                                                            |
| G39 |         219 | Get Metrics: calculate quality metrics like VMAF, SSIM, PSNR                                         | Existing; current rerun pending                                                                               | crates/media-runtime/src/quality.rs; tests/fixtures/quality-validation.md                                                                                                                                                 |
| G40 |         220 | Transfer Color Metadata: copy colour properties and HDR metadata from one file to another            | Existing; current rerun pending                                                                               | crates/media-runtime/src/utilities.rs; crates/media-runtime/tests/utility_jobs.rs                                                                                                                                         |
| G41 |         222 | Deinterlace Video: exports a deinterlaced copy - QTGMC over the whole file into a                    | Linux eight QTGMC presets and utility fallback qualified; Windows remains                                     | crates/media-runtime/src/jobs/qtgmc.rs; crates/media-runtime/src/utilities/temporal_exports.rs; tests/fixtures/temporal-validation.md                                                                                     |
| G42 |         227 | Film Grain (AV1): the parts of a grain workflow that are not an encode. Measure a grain table        | Implemented; utility real-tool test passed                                                                    | crates/media-runtime/src/jobs/av1an/grain.rs; crates/media-runtime/src/utilities/grain_measure.rs; crates/media-runtime/tests/utility_jobs.rs                                                                             |
| G43 |         232 | Cut Video: keep only a chosen section, copied out without re-encoding, with the start and end        | Visual editor browser proof and native copied trim; platform cases remain                                     | crates/media-runtime/src/jobs/encode_plan/framing.rs; src/lib/components/shared/FramingOptions.svelte; src/lib/components/shared/SourcePreview.svelte; tests/fixtures/framing-validation.md                               |
| G44 |         234 | Concatenate Into Single MKV: merge any amount of any compatible video format into a single MKV       | Existing; current rerun pending                                                                               | crates/media-runtime/src/utilities.rs; crates/media-runtime/tests/utility_jobs.rs                                                                                                                                         |
| G45 |         236 | Show Bitrate Chart: samples the bitrate across the entire video and plots it, so you can see         | Source image-sequence chart analysis qualified on Linux; native Windows remains open                          | crates/media-runtime/src/bitrate.rs; tests/fixtures/bitrate-validation.md                                                                                                                                                 |
| G46 |         238 | OCR Bitmap Subtitles: converts selected bitmap-based subtitle tracks into text subtitles             | Existing; current rerun pending                                                                               | crates/media-runtime/src/utilities.rs; crates/media-runtime/tests/utility_jobs.rs                                                                                                                                         |
| F01 |         249 | Ported from WinForms to Avalonia on .NET 10, with native builds for `win-x64`, `linux-x64`,          | Replacement framework; platform delivery still pending                                                        | src-tauri/tauri.conf.json; .github/workflows/package.yml; docs/packaging.md                                                                                                                                               |
| F02 |         251 | A dark theme throughout, rather than the system's default control colours.                           | Intentional visual replacement                                                                                | src-tauri/tauri.conf.json; .github/workflows/package.yml; docs/packaging.md                                                                                                                                               |
| F03 |         252 | A release pipeline: every build is published self-contained and portable, with the external          | Existing Windows packaging; platform delivery pending                                                         | src-tauri/tauri.conf.json; .github/workflows/package.yml; docs/packaging.md                                                                                                                                               |
| F04 |         254 | The log is its own panel - per-line severity colouring, copy, save, clear, and a button that         | Source log actions/persistence focused browser checks; native platform remains                                | src/App.svelte; src-tauri/src/lib.rs; tests/frontend/workspace.spec.ts                                                                                                                                                    |
| F05 |         256 | The window remembers itself: size, position, open tab and log height, plus the last browsed          | Source window/tab/log-height persistence implemented; native platform remains                                 | src/App.svelte; src-tauri/src/lib.rs; tests/frontend/workspace.spec.ts                                                                                                                                                    |
| F06 |         268 | Sources are checked rather than assumed. The container's field-order flag is read first - an         | Source field detection and automatic utility fallback checked; wider platform remains                         | crates/media-runtime/src/analysis.rs; crates/media-runtime/src/jobs/encode_plan.rs; tests/fixtures/analysis-validation.md                                                                                                 |
| F07 |         272 | The Deinterlace row only appears for files whose fields warrant it, so a modern download never       | Conditional row exposed; focused source UI checks; native platform remains                                    | crates/media-runtime/src/analysis.rs; crates/media-runtime/src/jobs/encode_plan.rs; tests/fixtures/analysis-validation.md                                                                                                 |
| F08 |         274 | QTGMC, the motion-compensated deinterlacer, run through VapourSynth (bundled on Windows) with        | Linux QTGMC provider rendered all eight presets; Windows native remains                                       | crates/media-runtime/src/jobs/qtgmc.rs; crates/media-runtime/src/utilities/temporal_exports.rs; tests/fixtures/temporal-validation.md                                                                                     |
| F09 |         277 | Both encode tabs deinterlace. Quick Convert pipes QTGMC straight into its encoder; the AV1AN         | Source Quick/AV1AN controls mapped; QTGMC Linux proof and combined cases remain                               | crates/media-runtime/src/jobs/qtgmc.rs; crates/media-runtime/src/utilities/temporal_exports.rs; tests/fixtures/temporal-validation.md                                                                                     |
| F10 |         281 | A Deinterlace Video utility for when the deinterlaced file itself is what you want, with its         | Linux automatic deinterlace and selected QTGMC fallback passed; Windows remains                               | crates/media-runtime/src/jobs/qtgmc.rs; crates/media-runtime/src/utilities/temporal_exports.rs; tests/fixtures/temporal-validation.md                                                                                     |
| F11 |         289 | HDR sources can be converted to SDR, on both encode tabs. The row only appears for a file whose      | Linux source Dolby matrix includes Vulkan Spline and calibrated CPU fallback; Windows remains open            | crates/media-runtime/src/jobs/encode_plan/tone_map.rs; tests/fixtures/tone-map-validation.md                                                                                                                              |
| F12 |         292 | The roll-off is built around the picture's measured brightness, not the metadata's claim. The        | Spline measured peak path qualified for real P8.1; broader combinations remain                                | crates/media-runtime/src/jobs/encode_plan/tone_map.rs; tests/fixtures/tone-map-validation.md                                                                                                                              |
| F13 |         302 | Which of the two backends runs is Quick Convert's question alone. There it is the machine's:         | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/encode_plan/tone_map.rs; tests/fixtures/tone-map-validation.md                                                                                                                              |
| F14 |         308 | It runs before any crop, scale or burnt-in subtitle, so subtitles are not dragged through a          | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/encode_plan/tone_map.rs; tests/fixtures/tone-map-validation.md                                                                                                                              |
| F15 |         312 | The previews show the tone-mapped picture. Thumbnails, the Cut window's scrubber and the crop        | Existing; current rerun pending                                                                               | crates/media-runtime/src/analysis.rs; tests/fixtures/metadata-preview-validation.md                                                                                                                                       |
| F16 |         315 | Dolby Vision is read off the file. A profile whose base layer plays without the RPU is               | Linux single-layer P8.1 combinations qualified; dual-layer reconstruction and Windows not claimed             | crates/media-runtime/src/jobs/encode_plan/tone_map.rs; tests/fixtures/tone-map-validation.md                                                                                                                              |
| F17 |         334 | The Grain Synthesis row is now a mode selector, and that is not cosmetic: SVT-AV1 has _three_        | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/av1an/grain.rs; crates/media-runtime/src/utilities/grain_measure.rs; crates/media-runtime/tests/utility_jobs.rs                                                                             |
| F18 |         340 | A grain table can be measured off your own source rather than guessed at by the encoder. The file    | Implemented; utility real-tool test passed                                                                    | crates/media-runtime/src/jobs/av1an/grain.rs; crates/media-runtime/src/utilities/grain_measure.rs; crates/media-runtime/tests/utility_jobs.rs                                                                             |
| F19 |         345 | Measuring happens once per source, not once per encode. It used to be an encode mode - hours of      | Implemented; utility real-tool test passed                                                                    | crates/media-runtime/src/jobs/av1an/grain.rs; crates/media-runtime/src/utilities/grain_measure.rs; crates/media-runtime/tests/utility_jobs.rs                                                                             |
| F20 |         352 | The row is what the encoder does while it encodes, and costs no pass of its own. Grain written       | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/av1an/grain.rs; crates/media-runtime/src/utilities/grain_measure.rs; crates/media-runtime/tests/utility_jobs.rs                                                                             |
| F21 |         397 | Resize is a dropdown with presets - 2160p through 360p as boxes the picture is fitted inside,        | Source resize modes/fills/SAR/reset browser matrix; real/platform cases remain                                | crates/media-runtime/src/jobs/encode_plan/framing.rs; src/lib/components/shared/FramingOptions.svelte; src/lib/components/shared/SourcePreview.svelte; tests/fixtures/framing-validation.md                               |
| F22 |         401 | Anamorphic sources are de-squeezed where the encoder cannot carry the flag, upscaling is stated      | SAR and preset browser matrix; native real-media/platform cases remain                                        | crates/media-runtime/src/jobs/encode_plan/framing.rs; src/lib/components/shared/FramingOptions.svelte; src/lib/components/shared/SourcePreview.svelte; tests/fixtures/framing-validation.md                               |
| F23 |         404 | Crop keeps the rectangle inside the frame and on the chroma grid, and a crop too large for the       | Source crop geometry UI checks; smaller real-media/platform cases remain                                      | crates/media-runtime/src/jobs/encode_plan/framing.rs; src/lib/components/shared/FramingOptions.svelte; src/lib/components/shared/SourcePreview.svelte; tests/fixtures/framing-validation.md                               |
| F24 |         407 | Borders: pad out to a target aspect ratio (16:9, 4:3, 1:1, 9:16, 21:9) without scaling. Which        | Source border readouts/geometry browser checks; native combinations remain                                    | crates/media-runtime/src/jobs/encode_plan/framing.rs; src/lib/components/shared/FramingOptions.svelte; src/lib/components/shared/SourcePreview.svelte; tests/fixtures/framing-validation.md                               |
| F25 |         410 | Trim is picked visually. The in/out dialog shows the frame at the playhead while you choose the      | Selected-video trim browser proof, Quick Copy 6/6 and Direct MOV_TEXT finalization qualified on Linux         | crates/media-runtime/src/jobs/encode_plan/framing.rs; src/lib/components/shared/FramingOptions.svelte; src/lib/components/shared/SourcePreview.svelte; tests/fixtures/framing-validation.md                               |
| F26 |         426 | Target SSIMULACRA2, Target Butteraugli and Target XPSNR quality modes alongside Target VMAF,         | Existing; current rerun pending                                                                               | docs/av1an-svt-x264.md; docs/standalone-encoders.md; tests/fixtures/av1an-options-validation.md; crates/media-runtime/tests/source_format_jobs.rs                                                                         |
| F27 |         428 | Vship staged per machine on Windows: the GPU check runs before a metric-targeted encode and          | Existing; current rerun pending                                                                               | docs/av1an-svt-x264.md; docs/standalone-encoders.md; tests/fixtures/av1an-options-validation.md; crates/media-runtime/tests/source_format_jobs.rs                                                                         |
| F28 |         431 | x264 joins aomenc, SVT-AV1, x265 and vpxenc as an encoder av1an can drive from here.                 | Existing; current rerun pending                                                                               | docs/av1an-svt-x264.md; docs/standalone-encoders.md; tests/fixtures/av1an-options-validation.md; crates/media-runtime/tests/source_format_jobs.rs                                                                         |
| F29 |         432 | The advanced argument grid is grouped into categories, and every argument carries a full             | 375 source rows: 297 CPU scalar, 11 typed resources, 67 hardware exclusions                                   | src/lib/content-presets.json; src/lib/components/shared/AdvancedEncoderOptions.svelte; crates/media-runtime/src/jobs/parameters.rs                                                                                        |
| F30 |         434 | Content presets for the argument grid - Anime / Cel Animation and Game Capture / Gameplay -          | Built-in presets and source Advanced grid exposed; integrated rerun remains                                   | src/lib/content-presets.json; src/lib/components/shared/AdvancedEncoderOptions.svelte; crates/media-runtime/src/jobs/parameters.rs                                                                                        |
| F31 |         438 | Workers and threads-per-worker are derived together from the core count instead of one being a       | Source worker/thread controls mapped; None split/max-chunk native case open                                   | crates/media-runtime/src/av1an_resources.rs; src/lib/components/shared/Av1anResources.svelte                                                                                                                              |
| F32 |         441 | Scene detection runs in parallel slices ahead of av1an. Detection is the one phase the workers       | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/av1an/scenes.rs; docs/av1an-svt-x264.md                                                                                                                                                     |
| F33 |         446 | Progress is measured from av1an's own temp folder - chunk counts out of `scenes.json` and            | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/av1an.rs; crates/media-runtime/src/jobs.rs; tests/fixtures/av1an-options-validation.md                                                                                                      |
| F34 |         448 | Frame-rate resampling no longer kills a run partway through, av1an's log lands in the encode's temp  | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/av1an.rs; crates/media-runtime/src/jobs.rs; tests/fixtures/av1an-options-validation.md                                                                                                      |
| F35 |         457 | The re-encoders are the real binaries now. x264, x265, SvtAv1EncApp, aomenc and vpxenc - the         | Source SVT provider pinned and actual Quick/AV1AN exercised; broader codecs remain                            | docs/av1an-svt-x264.md; docs/standalone-encoders.md; tests/fixtures/av1an-options-validation.md; crates/media-runtime/tests/source_format_jobs.rs                                                                         |
| F36 |         463 | The command is built in one place, in an order that holds together: stream maps that know            | Source mapping, metadata and copy-trim focused native proofs; combined cases remain                           | docs/av1an-svt-x264.md; docs/standalone-encoders.md; tests/fixtures/av1an-options-validation.md; crates/media-runtime/tests/source_format_jobs.rs                                                                         |
| F37 |         467 | Target Filesize divides by the right numbers - the trimmed duration, and a copied track's own        | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/rate_control.rs; tests/fixtures/rate-control-validation.md                                                                                                                                  |
| F38 |         469 | Per-track audio configuration seeds from the channel layout you asked for, and the dropdown it       | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/audio.rs; crates/media-runtime/src/loudness.rs; tests/fixtures/audio-validation.md; tests/fixtures/loudness-validation.md                                                                   |
| F39 |         471 | Paths survive the shell and the filtergraph. Burning in a subtitle track from a path containing      | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/subtitles.rs; tests/fixtures/subtitle-validation.md                                                                                                                                         |
| F40 |         480 | Loudness normalization (EBU R128) on the Quick Convert tab, to -14, -16 or -23 LUFS. Each track is   | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/audio.rs; crates/media-runtime/src/loudness.rs; tests/fixtures/audio-validation.md; tests/fixtures/loudness-validation.md                                                                   |
| F41 |         492 | Sample Encodes - the CRF ladder: short sections from across the source, encoded at several CRFs,     | Implemented; AOM/SVT three VMAF models, presets and retained clips passed; hardware encoders excluded by user | crates/media-runtime/src/utilities/ladder_options.rs; src/lib/content-presets.json; crates/media-runtime/tests/utility_jobs.rs                                                                                            |
| F42 |         503 | Cut Video - keep a chosen section, copied out without re-encoding, picked in the same visual         | Shared visual cut editor and native cut-family proof; Windows remains                                         | crates/media-runtime/src/jobs/encode_plan/framing.rs; src/lib/components/shared/FramingOptions.svelte; src/lib/components/shared/SourcePreview.svelte; tests/fixtures/framing-validation.md                               |
| F43 |         505 | Deinterlace Video - a deinterlaced copy into a near-lossless MKV with audio and subtitles            | Linux QTGMC provider and utility fallback proof; Windows remains                                              | crates/media-runtime/src/jobs/qtgmc.rs; crates/media-runtime/src/utilities/temporal_exports.rs; tests/fixtures/temporal-validation.md                                                                                     |
| F44 |         507 | OCR Bitmap Subtitles is now a card on the Utilities tab; the code was there before but nothing       | Existing; current rerun pending                                                                               | crates/media-runtime/src/utilities.rs; crates/media-runtime/tests/utility_jobs.rs                                                                                                                                         |
| F45 |         515 | Pause actually pauses, freezing the whole process tree rather than the launcher, and Stop ends       | Linux process-tree and image pause proof; Windows native lifecycle remains                                    | crates/media-runtime/src/supervisor/pause.rs; crates/media-runtime/src/jobs.rs; tests/fixtures/pause-validation.md                                                                                                        |
| F46 |         517 | Live progress in the footer with an ETA, whichever tool is running. QTGMC's progress is              | Implemented; live event/ETA Rust and focused browser tests passed                                             | crates/media-runtime/src/jobs/av1an.rs; crates/media-runtime/src/jobs.rs; tests/fixtures/av1an-options-validation.md                                                                                                      |
| F47 |         521 | The OS notifies you when a run ends and the window is not in the foreground, through                 | Existing; current rerun pending                                                                               | src-tauri/src/completion.rs; docs/completion-actions.md                                                                                                                                                                   |
| F48 |         523 | What the encode did to the file size is reported when it finishes.                                   | Encode size proof; utility artifact summaries added, some result sizes still missing                          | crates/media-core/src/jobs.rs; crates/media-runtime/src/jobs.rs; crates/media-runtime/src/jobs/mux.rs; src/lib/components/shared/JobStatus.svelte                                                                         |
| F49 |         524 | Shutdown when done, with a 60-second countdown you can call off.                                     | Existing; current rerun pending                                                                               | src-tauri/src/completion.rs; docs/completion-actions.md                                                                                                                                                                   |
| F50 |         525 | Batch mode works, with per-file status in the file list and output-name templates                    | Source batch 101 inputs and naming 7/7; Windows native filesystem remains                                     | crates/media-runtime/src/utilities/batch.rs; src/lib/features/batch/BatchEncode.svelte; tests/fixtures/batch-operation-validation.md                                                                                      |
| F51 |         534 | The Metrics utility had been passing the VMAF model file as libvmaf's _log path_, which scored       | Existing; current rerun pending                                                                               | crates/media-runtime/src/quality.rs; tests/fixtures/quality-validation.md                                                                                                                                                 |
| F52 |         537 | The bitrate readout reported `Size: 0B (0.0%)` for every stream, ffmpeg having renamed the unit in   | Original image-sequence packet readout and chart qualified on Linux                                           | crates/media-runtime/src/bitrate.rs; tests/fixtures/bitrate-validation.md                                                                                                                                                 |
| F53 |         539 | Colour metadata is read using the names ffprobe actually prints and re-spelled per encoder, aomenc   | Existing; current rerun pending                                                                               | docs/av1an-svt-x264.md; docs/standalone-encoders.md; tests/fixtures/av1an-options-validation.md; crates/media-runtime/tests/source_format_jobs.rs                                                                         |
| F54 |         541 | Every HDR-preserving SVT-AV1 encode had been coming out declaring no peak brightness at all - the    | Exact SVT provider and HDR resources exercised; integrated/platform proof remains                             | docs/av1an-svt-x264.md; docs/standalone-encoders.md; tests/fixtures/av1an-options-validation.md; crates/media-runtime/tests/source_format_jobs.rs                                                                         |
| F55 |         545 | A failed av1an run quotes the tail of av1an's own log in the failure message; the visible-console    | Existing; current rerun pending                                                                               | crates/media-runtime/src/jobs/av1an.rs; crates/media-runtime/src/jobs.rs; tests/fixtures/av1an-options-validation.md                                                                                                      |
| F56 |         548 | A missing mkvmerge - routine on Linux and macOS - is detected before it is needed, rather than       | Existing; current rerun pending                                                                               | docs/av1an-svt-x264.md; docs/standalone-encoders.md; tests/fixtures/av1an-options-validation.md; crates/media-runtime/tests/source_format_jobs.rs                                                                         |
| F57 |         551 | Animated GIF could not be produced at all unless some other filter happened to be configured.        | Linux image output and filter/lifecycle checks qualified; Windows remains open                                | crates/media-runtime/src/jobs/images.rs; tests/fixtures/README.md; crates/media-runtime/tests/image_jobs.rs                                                                                                               |

## Additional baseline flows and package scope

| Flow                                                                   | Baseline evidence                                                    | Migration / acceptance work                                                                                                                                          |
| ---------------------------------------------------------------------- | -------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Audio-only conversion, strip-video and copy-video with converted audio | Nmkoder/UI/Tasks/QuickConvert.cs:1041; Nmkoder/Data/Containers.cs:65 | Encode/copy/drop video mode; M4A/AAC and OGG/Opus/Vorbis/FLAC; selected-track metadata and timing; no-overwrite and cancellation.                                    |
| All explicit source argument grids                                     | Nmkoder/BinFiles/encoderArgs/{av1an,ffmpeg}/*.json                   | Compare every named source control with the correct Jesses route and record unsupported/owned primary controls; built-in presets must apply actual supported values. |
| Ladder full controls                                                   | Nmkoder/UI/Tasks/UtilCrfLadder.cs:46                                 | AOM/software choices (hardware excluded), content presets, VMAF standard/NEG/4K models, retained reference/candidate clips, recommendation and output readback.      |
| Padded cadence utility                                                 | Nmkoder/UI/Tasks/UtilRepairCadence.cs                                | Existing BestSource timeline/content-aware export, separate from exact-duplicate encoding.                                                                           |
| Concrete VFR input                                                     | Nmkoder FFmpeg encode path                                           | Investigate timestamp-preserving or explicitly normalized conversion; test known variable timestamp fixture.                                                         |
| Rotated/nonzero-start input                                            | Nmkoder FFmpeg encode path                                           | Investigate display matrix handling and timeline rebasing without losing A/V offsets.                                                                                |
| Small/large/high-rate input                                            | Nmkoder FFmpeg encode path                                           | Review finite source cases below 64 pixels, above 8192 and above 120 fps; encoder/tool capability gates must remain explicit.                                        |
| Linux/macOS portable builds                                            | Source README:249,555–578                                            | Previously deferred in Jesses; active Windows implementation does not prove platform parity. Restore only with actual native package/tool/runtime evidence.          |
| Windows portable tools and source notices                              | docs/packaging.md                                                    | Existing pinned delivery recipes; qualify exact final archive, sources/licenses, tool discovery and clean environment.                                               |
| Windows lifecycle and fresh image                                      | docs/windows-clean-qualification.md                                  | RestrictedHost evidence exists; FreshImage, native UI and Scoop update/uninstall are separate gates.                                                                 |
| Hardware video encoders                                                | docs/source-nvenc-provider.md                                        | Removed by explicit CPU-only user scope; old records remain readable and active encoding is rejected. GPU processing is preserved.                                   |

## Historical session verification record

- Baseline repository/status inspection: both checkouts clean before implementation.
- Installed official Rust **1.98.1**, including rustfmt and Clippy.
- Built and hash-verified official FFmpeg/FFprobe **9.0.1** using
  `scripts/ci-install-ffmpeg.py`; local tools are under
  `/tmp/jesses-toolchain/ffmpeg-9.0.1`. System FFmpeg 6.1.1 is retained separately.
- Source-built-in preset extraction: all six preset sets and all 48 original
  parameter pairs retained in `src/lib/content-presets.json` (13 x264, 19 x265,
  16 SVT controls). Unsupported installed-build controls must be reported.
- Focused utility checks completed:
  - `cargo test -p media-runtime --lib utilities:: --locked`: 13 passed,
    2 existing installed-SVT tests remained ignored in that filtered run.
  - `cargo test -p media-runtime --lib live_pause --locked`: 2 passed; native
    descendants stop/resume, suspended time is excluded, and cancellation reaps trees.
  - `cargo test -p media-runtime --lib scoped_pause_ --locked`: 1 passed;
    both pipeline trees and all six fixture processes stop/resume and are reaped.
  - `cargo test -p media-runtime --test utility_jobs fork_ --locked -- --ignored
--test-threads=1`: 1 passed with actual FFmpeg 9.0.1 and grav1synth 0.2.0;
    all eight x264 Anime controls, SSIM ladder, three retained 24-frame clips,
    source-only grain measurement with private/kept references, source preservation.
  - `cargo test -p media-runtime --test utility_jobs utility_batch_ --locked --
--ignored --test-threads=1`: 1 passed; failed-source continuation, valid
    per-file cut, repeated-output rejection and unchanged original/output bytes.
  - `python3 scripts/test-ipc-registration.py`: 3 passed; every native handler
    is declared in the application manifest, permitted and routed.
- Native app checks reported by frontend worker: `cargo check -p jesses --lib`
  passed, Svelte typecheck passed and focused browser visual-trim/framing/audio-only
  request tests passed. Final integrated counts will replace this interim report.
- Parameter catalog source inventory now covers all 375 rows (364 fixed scalar
  specifications plus 11 typed resource rows). Six manual HDR metadata/import
  rows remain pending with the encode owner; see `docs/encoder-catalog-parity.md`.
  `cargo test -p media-runtime --lib jobs::parameters::tests --locked`:8 passed,
  1 installed-tool test ignored in that unit filter. Real `parameter_jobs` rerun
  with FFmpeg9.0.1, x264, libx265, libvpx-VP9 and all 3 pinned SVT builds:2 passed.
  New x264 Boolean flags/pairs, x265 nested values and VP9 controls actually encode.
- `cargo check -p jesses --lib --locked` passed after utility progress and size
  schemas. Source file sizes refer original inputs; output sizes are read from the
  validated temporary before publication. Both fields are optional decimal text.
- Live utility progress uses ticket-scoped `utility-progress` events, per-row
  lifecycle, five-second FFmpeg machine positions and one-second elapsed/ETA
  heartbeat. Grain measurement uses the fork's explicit approximate 7.2 MP/s
  reference rate. Paused time is excluded. Terminal results remain authoritative.
- Corrected package capability distinction: actual FFmpeg package recipes already
  pin libvmaf and smoke-test built-in model identities. The lightweight local
  CI provisioner originally omitted libvmaf. The completed v3 production build
  now includes libaom/libsvtav1, with all 7 wrapper/model smokes and receipt checked
  by the package worker; six actual Jesses AOM/SVT model workflows also passed.
- Remaining GUI phase implemented log copy, remembered chooser folder,
  automatic field detection/application, explicit upscale warning, source grain
  estimate display, utility live progress and completed size comparison.
  Frontend check/build and 15 focused Chromium tests passed; the full suite is pending.
- Broad integration and native platform/package qualification remain open.

## Required checks

Run focused tests first for changed behavior, followed by one coordinated full
pass after integration:

```sh
cargo fmt --all --check
cargo test -p media-core --locked
cargo test -p media-runtime --lib --locked
cargo test -p media-runtime --test utility_jobs --locked -- --ignored --test-threads=1
cargo test -p media-runtime --test temporal_utility_jobs --locked -- --ignored --test-threads=1
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
pnpm contracts:check
pnpm format:check
pnpm check
pnpm build
pnpm test
```

Real-tool suites require the named executable and plugin capabilities. A missing
tool is a failed/unavailable qualification, not a successful skip. Existing
packaging recipes and Windows safety harnesses provide additional targeted gates;
platform-dependent execution cannot be inferred from Linux compilation.

## Historical source native UI checkpoint — container/P010 phase

- `source-ladder-formats-real.log`: all27 software source color menu choices
  passed actual Jesses workflows, independent complete retained decoding, selected
  metrics, alpha retention and source/no-clobber. Source32-choice admission table
  unit passed1/1; P010 native packing/probe/admission tests passed2/2.
- `source-utility-containers-real.log`: six actual cut formats (MKV/MP4/MOV/WebM/
  AVI/TS), plus MKV/MP4/WebM grain apply/rewrite/remove with chapters, track tags,
  attachment/header hashes, encoded packet/base-frame/source preservation.
- `source-container-runtime-unit.log`: ordinary runtime units367 passed,23
  installed-tool ignores,0failed (4.37s). `source-native-tests-check.log`: runtime
  all-test-target compilation passed. Logs reside under `/tmp/jesses-toolchain`.
- The initial container phase left IVF/film-stock provider qualification pending.
  The source-pinned provider phase below closes those runtime gaps on Linux;
  Windows console consumers and native platform qualification remain pending.

## Historical source-pinned grain provider checkpoint — 2026-11-01

The complete fork bundler applies no grav1synth patch: commit1044228 embeds its
14 stock resources and fixes the released tool's IVF clock handling. Production
discovery now includes the explicit override and verified bundled manifest.
The maintained recipe builds the exact source with private static FFmpeg8.1 and
dav1d1.5.4, complete sources/Cargo vendor/licenses/frozen build inputs and hash
receipt. Windows/Linux packaging stages it and requires discovery/media gates;
macOS retains the explicit external-provider route.

Current qualified delivery:
`/tmp/jesses-toolchain/grav1synth-static-qualified-v4/delivery`.
Manifest-staged discovery proof:
`/tmp/jesses-toolchain/grain-provider-resources-v4`.
No grav1synth PATH override is used in the bundled-provider test.

- `packaged-grain-provider-stocks.log`: 2 actual tests passed (8.55s), all14
  immutable preset tables matched their embedded stock parameter states, plus
  320×240 padded-stride measurement at8/10bits. Unknown modifiers are refused.
- `packaged-grain-provider-containers.log`: 3 actual tests passed (6.17s), all14
  IVF stock writes/read-backs; photon apply/rewrite/extract/remove at24,
  24000/1001 and30000/1001 with identical header clocks, frame counts, packet
  ticks and base pixels; original bytes/no-clobber; existing six cut and
  MKV/MP4/WebM grain container invariants remain positive.
- `packaged-grain-provider-measure.log`: source denoise/measurement and retained
  reference/ladder regression passed1/1 with the static provider.
- Recipe integrity tests passed4/4; toolchain regressions4passed/1Windows-only
  skip; standalone staging regressions6/6. All logs are under
  `/tmp/jesses-toolchain`; no full-workspace or hosted rerun was started.

Stopping point requested by the user: native Windows/macOS provider build and
media execution, final combined Rust/frontend/package checks, and separate
Windows console consumers remain for resume. No active provider process or
background build remains. See[provider documentation](grav1synth-provider.md).
