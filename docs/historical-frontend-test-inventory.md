# Historical frontend test reconciliation

The fork's six-screen UI and the user's exact-control requirement supersede the
former Jesses-only inspector, separate Batch Encode and Remux workspaces, sample
loader, and extra settings. Removing those controls is not evidence that the
underlying encoding, track, utility or job behavior can be discarded. Historical
tests are retained until their meaningful assertions are transferred to a fork
surface or a native test, or a source-specific rationale establishes that an
assertion concerns only a removed control.

## Inventory and evidence

`pnpm exec playwright test --list` listed 467 tests across 26 files before this
reconciliation; the current list contains 393 tests across 29 files, reflecting
both source-based replacements and added coverage. An initial whole-suite
classification run was stopped after 56
repeated failures in the first two obsolete shared setups; it is **not a full
suite result**. Its partial output is in
`target/migration-validation/frontend/historical-suite.log`.

A bounded representative run selected the first test location per file (31
expanded cases). Six passed and 25 failed/timed out with a six-second limit.
The exact inputs, output and machine-readable results are in
`target/migration-validation/frontend/representatives-input.json`,
`representatives.log` and `representatives.json`. This run classifies setup
failures; it does not prove that all remaining assertions in each file are
obsolete or correct. Some source files changed during the broader investigation,
so each migrated group is rerun independently with the normal timeout.

The representative failures group as follows:

- `analysis`, `bitrate`, `containers`, `external-tracks`, `files`, `images`,
  `metadata`, `mux`, `parameters`, `quality`, `remux`, `subtitles`, `temporal`
  and `utilities` first stopped at the old `Add files` setup. Their actual
  assertions require case-by-case migration; this setup failure alone is not a
  reason to drop them.
- `batch` and `encode` stopped at the old Workspace navigation and separate
  Batch Encode surface. The fork runs batches through File List mode and the
  selected Quick/AV1AN task. Assertions about fixed-source requests, collisions,
  snapshots and cancellation remain relevant; explicit SVT fork-selection and
  separate preview/queue controls do not match the source UI.
- `layout` stopped at the retired synthetic sample loader. Viewport, compact
  controls and keyboard-navigation checks remain relevant to loaded media.
- `preferences` reached the old `Include subfolders` checkbox, which the fork's
  Settings screen does not expose. Other preference persistence, recent media
  and output-folder assertions must be considered separately.
- `recovery` stopped at the former lowercase `av1an`/jobs workspace; source
  recovery has a Resume dialog and shared footer. Pause, stop and recovery
  persistence assertions remain relevant.
- `session` stopped at the former Activity log disclosure; `workspace` stopped
  at former landing-page shortcuts. The source has a persistent log and six
  direct tabs. Layout persistence or other source-relevant assertions need their
  own mapping.
- `progress-estimate`, `source-advanced`, both source encode layouts, the source
  encode menu/default test and the source shell/track test passed their selected
  representative cases. This is not a complete pass of those files.

## Completed source-shaped replacements

- `images.spec.ts`: replaced the old Utilities still-frame depth selector test
  with Quick Convert's full-source PNG/JPEG sequence controls. It still asserts
  RGBA 16-bit reaches the PNG request and JPEG uses its own YUV format and
  quality rather than retaining a PNG-only value. The fork exposes sequences,
  so retaining a still-image utility solely for the old test would add a control
  the user explicitly rejected.
- `bitrate.spec.ts`: the old adjustable-window inspector and SVG export are not
  source controls. The migrated tests retain original stream identity, lazy
  execution, cancellation and late-result rejection via Read Bitrates and Show
  Bitrate Chart. The source chart uses one-second windows.
- `layout.spec.ts`: replaced the synthetic sample/Filters panels with loaded
  media, the two source encode tabs and their actual subtabs. It retains the
  800/1280/1920 viewport, narrow numeric fields, visible Run and keyboard
  navigation assertions. Arrow/Home/End navigation was restored on the actual
  source subtabs.
- `containers.spec.ts`: replaced the separate batch preview and destination
  summary controls with Quick/AV1AN's source stem and selected-container fields.
  It preserves selected extension and asynchronous single/batch snapshot
  assertions, including dotted stems.
- `subtitles.spec.ts`: replaced independent per-track burn selectors and a
  separate batch review with the source's single Burn In selector and subtitle
  codec. It preserves original stream identities, retained burned captions,
  workflow state and immutable per-file batch subtitle settings.
- `metadata.spec.ts`: replaced the retired expanded probe inspector with the
  source Track List's stream descriptions and metadata-edit request checks, and
  retained audio-only thumbnail behavior. Exact large-byte IPC serialization
  and missing HDR metadata are also covered in `media-core/src/lib.rs` tests.
- `preferences.spec.ts`: replaced the old import/export, recursive-folder and
  manual Save controls with the fork's Recent menu and autosaved output folder.
  It retains destination persistence, existing-draft stability, unavailable
  recent removal and duplicate-probe protection.
- `session.spec.ts`: replaced the old log disclosure and numeric height field
  with the persistent source log and its keyboard-adjustable splitter; tab and
  height persistence remain tested.
- `analysis.spec.ts`: the fork uses a trim editor and Track List thumbnail,
  rather than the former inspector, manual detected-crop/field proposal and
  apply actions, separate batch preview or typed thumbnail seek. Source Trim
  tests retain exclusive frame bounds, keyframe snapping, pending-registration
  cancellation and stale preview/probe replies. Per-file automatic crop failure
  and frozen naming are tested during batch execution. The replacement checks
  source switching, late thumbnail rejection and orientation requests in the
  actual Track List. Manual proposal fingerprint and inspector-seek assertions
  were tied to the retired surfaces; native source/crop validation remains in
  its focused runtime suites.
- `files.spec.ts`: replaced the retired search and Stop import controls. The
  old case-insensitive search had no fork control; the three Stop import cases
  depended on a button absent from the fork, including their stopped-probe and
  recents races. The old recursive folder scan also differs from the fork's
  Add Folder action, which imports an image sequence (`Nmkoder/Views/
MainWindow.FileList.cs`, `AddFolder_Click`). The replacement retains mixed
  successful/failed Unicode media, Windows spelling aliases, canonical media
  identity and loading the surviving track list. Source shell tests separately
  cover folder sequence import, duplicate folders and recents; the preference
  suite checks recent persistence. A second test checks reordered tracks with
  colliding source indices in copy conversion.
- `workspace.spec.ts`: the old landing-page shortcuts, sample loader, separate
  source selector, review-buttons, Filters panel, Tools inspector and expanded
  HDR inspector are absent from the fork. Independent Quick/AV1AN draft and
  validation behavior is retained in `temporal.spec.ts` and the source encode
  suite; mixed import and stream identity are covered by `files.spec.ts` and
  `metadata.spec.ts`, and Color Data report behavior by `source-shell.spec.ts`.
  HDR metadata serialization and detection retain native coverage. The old
  orange/white palette was superseded by the requested paper-and-ink styling. Three
  replacement tests preserve empty browser behavior, disabled desktop actions,
  Ctrl+O and log behavior, and the requested palette/minimum-width shell under
  a dark OS preference. The persistent log's reload behavior is in `session.spec.ts`.
- `remux.spec.ts`: the fork has no separate Remux workspace, source picker,
  sample loader, job history panel or retained-temporary-path panel. Its copy
  conversion uses Track List and Quick Convert instead. Source tests retain
  chosen destination and extension, ordered source-scoped tracks, draft/source
  changes, cancel and terminal updates, reload of an existing job, and failed
  admission with editable retry. Empty destination/no-selection rejection and
  no-false-Done checks are now explicit. Windows retained-path reports remain
  tested by `windows_remux_mux_finalization_safety.rs`. The conversion cases
  also cover no-video audio/subtitle/external selection at the frontend contract;
  native subtitle-only/external-only execution is being qualified separately.
- `mux.spec.ts`: replaced the old separate combined-remux workspace, per-track
  forced flag and history UI with Track List and Quick Convert. The replacement
  retains colliding stream indices, order, independent source-scoped Unicode
  metadata, metadata/chapter owners and preservation of surviving edits when
  another source is removed. Source default-track selection is covered by the
  shell suite. Empty selection, failed admission, retry and minimum-width layout
  are covered by `remux.spec.ts` and `workspace.spec.ts`; native mux behavior
  remains covered by its runtime suites.
- `recovery.spec.ts`: the fork has an AV1AN Resume dialog and footer Pause,
  Resume and Stop, not the old job-history panel, standalone recovery controls,
  separate “Stop and keep progress” action or its completed-job size comparison.
  The replacement checks saved settings survive reload independently of editor
  drafts, one held resume admission, and footer pause/resume/stop. Source encode
  tests separately cover selected IDs, frozen clear IDs, delete/retry, new
  settings, keyboard selection and Shift review. Runtime recovery tests retain
  live-pause/cancel, durable checkpoints and stop/reopen/resume coverage;
  held-review native recovery qualification is tracked separately.
- `utilities.spec.ts`: the old utility picker, custom output picker, manual
  image-sequence ordering/import, foreign-saved-job inspector and expanded
  finish-action/history controls are not fork surfaces. The replacement checks
  the exact eleven cards and shared Run action. Source-shell tests retain
  source grain and cadence requests, automatic output naming, cut range,
  deinterlace choices, per-file batch identities, active-ticket-only progress,
  pause/resume, cancellation and late-result suppression. Additional focused
  tests retain missing remembered-picker fallback, readable log copy, and the
  fork's Shutdown when done checkbox with native error/retry behavior. Folder
  image-sequence ordering is covered by native sequence tests; fork UI imports
  a folder rather than a manually reordered image list. Runtime completion and
  saved-job compatibility tests remain independent of the removed extra UI.
- `external-tracks.spec.ts`: the former per-source donor panels, separate MOV
  timecode selector, manual subtitle offset/burn rows, audio editors, preview
  and CPU/GPU/peak/HDR-base-layer controls are not fork screens. Source Track
  List/Metadata coverage in `mux.spec.ts` retains source ownership, ordering,
  donors and surviving edits; source encode/subtitle tests retain external
  caption burn and source controls. The replacement tests Quick and AV1AN HDR
  curve menus/defaults/requests, per-track audio on colliding indices, and
  AV1AN external-audio codec mapping. Native external audio/subtitle, timeline,
  Dolby Vision and tone-map suites retain execution/validation checks. Missing
  source-valid multi-video and external video/data routes remain open in the
  encode contract ledger; this test migration does not claim them complete.
- `parameters.spec.ts`: the old free-text pre-queue editor, separate command
  preview, runtime tool-refresh control, custom persisted-preset editor and
  separate batch-review queue are not fork controls. The source Advanced grid,
  source presets/help and Shift-at-run editor are covered in the source encode
  suite. The replacement checks frozen AV1AN grid values in single and batch
  requests, late-catalog isolation after encoder switches and the source VP9
  alpha option in a WEBM request. Native encoder catalog/parser, typed HDR,
  alpha-container preflight and custom-filter tests remain separate; gaps in
  accepted source filter strings are still recorded in the contract ledger.
- `quality.spec.ts`: replaced the retired frame-range, frame-rate-fix and graph
  export controls with the fork's full-file Get Metrics dialog. It retains
  original video stream indices, metric configuration, cancellation and late
  result suppression, including preservation of an already completed result.
- `temporal.spec.ts`: the former automatic-field suggestion/apply controls,
  explicit field-order picker and cadence-repair controls are not present in
  the fork. The replacement exercises its QTGMC preset and field cadence,
  validated rational frame rate, separate Quick/AV1AN drafts and AV1AN BWDIF
  request. Source YADIF request mapping is also covered in the source suite.

These replacements use mocked desktop contracts. Native media/tool proof is
tracked separately; Linux held-command routes have real-media evidence and
Windows qualification remains open. The earlier active Playwright suite passed
252 tests with one then-pending copied-audio trim case (54.9 seconds); the log
is `target/migration-validation/frontend/full-source-ui-01.log`. That trim
case now has active browser and Linux native coverage; see
[source copied trim](source-copied-trim.md). The 201 expanded historical
batch/encode cases have a
[case-by-case inventory](historical-encode-batch-cases.md), distinguishing
covered, retired, and open native/source gaps. Both obsolete inspector files
are archived and the active replacements exercise source Run. The inventory is
a dated record: historical references to NVENC are superseded by the CPU-only
scope, and no full platform or overall migration pass is claimed here.
