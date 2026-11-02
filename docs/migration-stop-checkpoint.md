# Migration stop checkpoint

**Current user scope: CPU encoding only.** The explicit latest request removes all hardware video encoding, including NVENC, from Jesses. Legacy hardware job records remain readable but active hardware encoding requests must be rejected; packet copy/imported media remains supported. Vulkan processing, libplacebo Spline tone mapping, and GPU-assisted metrics/scoring are explicitly preserved. Historical NVIDIA receipts are evidence of past exploration, not a current capability or pending qualification.

## Resumed session — 2026-11-01 (current)

The user resumed work after the shutdown below. The migration goal remains
active and unfinished; this checkpoint does **not** claim 100% parity. The
checkout contains concurrent, unstaged migration work; preserve it and consult
the phase ledgers rather than restoring the historical backup over it.

- Source-facing UI exposes the six screens and mappings for all 18 auxiliary
  windows, with the requested compact paper-and-ink theme. Output stem,
  batch naming, fixed-image caption selection, resize and trim wiring are
  described in [current UI audit](source-ui-current-audit.md),
  [output/image UI](source-output-image-ui.md) and
  [batch naming](source-batch-naming.md). The historical full Playwright run passed 252
  tests with one pending copied-audio trim expectation; the pending case has since passed focused tests but no new full run has been made; the log is
  `target/migration-validation/frontend/full-source-ui-01.log`.
  Fixed-image Pause/Resume is wired and has focused mocked UI coverage.
  Both historical inspector suites are archived with individual dispositions;
  see [inventory](historical-frontend-test-inventory.md).
- Shift+Run and ordinary Run use the held-review contract. Linux command-edit
  real-media tests passed 11/11 and the scoped image suite passed 34/34,
  including edited execution, recovery and cancellation. Windows native execution remains unqualified; macOS remains separate source-platform work. Linux fixed-image Pause/Resume now has
  mocked UI coverage and native process-state proof; see
  [image pause](source-image-pause.md). See [command review](source-command-review-ui.md)
  and [native command editing](source-command-editing.md).
- The restored Linux FFmpeg 9 provider is at
  `target/toolchains/ffmpeg-full-20261101/delivery`. Linux R79/QTGMC provider
  qualification rendered all eight presets, with proof in
  `target/toolchains/qtgmc-linux-20261101/provenance/build-logs/qualification.log`.
  Real image-caption gates passed 6/6, including QTGMC, in
  `target/migration-validation/image-native-20261101/qualified-image-captions.log`;
  the historical sequence suite passed 4/4 with that FFmpeg provider. The
  final image suite passed 34/34 real-media gates, including explicit/implicit
  metadata, sequence numbering, automatic input selection, trim, edited
  execution and cancellation (`target/migration-validation/image-native-20261101/
image-final-scoped-regression.log`). Its
  nonvideo-output suppression is an intentional fix for the fork's corrupt
  image-sequence/GIF edge, detailed in [encode gaps](source-encode-contract-gaps.md).
- Source-selected subtitle-only and external-only audio now have frontend
  request checks and actual Linux held-FFmpeg output proof; unchecked video is
  omitted. Broader source-valid external-track paths remain open.
- Source batch now refreshes known track selection independently for each
  input, applies per-file video-drop mode for Quick nonvideo inputs, and uses
  cancellable native preview/naming requests. Twenty-seven focused mocked-browser
  cases cover 101 inputs, mixed and all-nonvideo inputs, and Stop/retry during
  analysis registration and late preview replies. Native source-batch cap,
  mixed-input and cancellation qualification passed 7/7; see
  `target/test-artifacts/source-command-edit/source-batch-final.log`.
- Quick Copy trim now has Linux native 6/6, browser 2/2, helper negative
  1/1 and held lifecycle 1/1; see [source Copy trim](source-copy-trim.md).
  The source-selected preview/video consistency is an intentional bug fix.
  AV1AN copied-cut trim passed 9/9 including converted WebVTT/MOV_TEXT;
  rotated input remains unqualified ([receipt](source-av1an-copied-cut.md)).
  Direct overlapping MOV_TEXT ISO finalization is still being repaired.
- Source filters have browser 11/11 and bounded positive native SVT, x264/x265,
  empty-grid VFR and AV1AN cases; image/lifecycle cases are still active
  ([receipt](source-custom-filters.md)). The exact SVT provider is pinned and
  qualified ([receipt](source-svt-provider.md)). Four real Dolby P8.1 CPU-x264
  Vulkan Spline variants passed; trim/burn/rotation/VFR combinations remain
  unqualified ([receipt](vulkan-libplacebo-provider.md)).
- Open handoff: Windows console behavior, VP9/MP4 chroma declaration, AV1AN
  None split/max-chunk and transformed-target, remaining custom-filter/image
  lifecycle and combined DV paths, utility image-sequence bitrate/chart analysis
  and utility result-size receipts. Native owners need focused source comparisons
  and real media/lifecycle evidence. Final integration needs Rust, Clippy, fmt,
  contracts/IPC, browser, package and native Windows gates after the code freeze.
  Windows qualification is required for this Windows-focused goal; Linux and
  macOS are source-advertised platforms whose separate native qualification is
  additional work, not evidence already obtained or a new macOS requirement.
- The image output safety fix deliberately suppresses fork-corrupt nonvideo
  implicit image outputs. Hardware video encoders are excluded by explicit user
  scope; Vulkan/libplacebo and GPU processing remain in scope.

## Historical shutdown checkpoint

Saved 20261101T083132Z. User requested a stopping point and laptop shutdown. All three migration workers reported stopped with no owned live processes. The migration remains **unfinished**, with no 100% completion claim; positive NVIDIA hardware qualification is explicitly user-deferred.

## Resume authority and repository state

- Latest UI preference: **paper-and-ink theme**, exact Nmkoder controls/settings/options/defaults and compact two-/three-column layouts. This supersedes older theme sentences in handoff documents.
- Jesses: `~/repos/jesses`, branch `main`, HEAD `d0887967e48e8c1fd3cedfcec37d5564b3149001`. Large saved uncommitted migration tree; index was empty. Nothing was staged, committed, pushed, released, or discarded for this checkpoint.
- Source: `~/repos/nmkoder`, clean at `a7e9731c14a584b493894ff2e4601206c37bbc94`.
- Non-destructive local backup: `~/repos/jesses/.git/checkpoints/20261101T083132Z`. It contains `tracked.patch` (binary HEAD diff), `untracked.tar.gz`, Git status, SHA-256 manifest, and retained validation logs with original-path mapping. Saved working files remain the primary resume point.

## Last reported validation

- UI: 72 focused browser tests before final text change; four geometry tests afterward; Svelte check and frontend build passed.
- Native sequence: three real tests; .NET 10 ordering comparisons across 52 names; seven browser lifecycle tests and focused checks passed. Full strict Clippy still reports 26 earlier/shared lints: `/tmp/jesses-toolchain/sequence-final-clippy.log` (retained in the backup).
- Output resolver/image subtitle burn: compiles; three real caption/guard/race tests, contracts, IPC and Svelte checks passed.
- Final v4 provider: provider 2/2, containers 3/3, recipe 4/4, standalone 6/6, toolchain 4 passed with one Windows skip.
- Additional retained evidence: `vs-plugins-linux/qualification.json`, `portable-adapter-test.log`, and FFmpeg v3 `result.json`/`verification.log` under the backup's validation log tree.

## Historical pending work at shutdown

- Shift+Run editable command/pipelines; Windows console consumer.
- External video/data/multi-video allowed routes; single-SVT dynamic-provider mapping; custom-filter conformance.
- Output resolver/image-burn UI wiring; bitmap/QTGMC/shifted-VFR/cancel-code/prior-image regressions.
- Final broad workspace/browser/native tests and native hosted CI; resolve recorded strict Clippy findings.
- Historical checkpoint: NVIDIA qualification was deferred at shutdown; the later CPU-only request removes that capability. Consult `docs/migration-parity.md`, `docs/source-ui-current-audit.md`, `docs/source-encode-contract-gaps.md`, and `docs/nmkoder-native-ui-handoff.md` for detailed open items.

## Temporary tool artifacts

The user's urgent shutdown request superseded optional large artifact retention. Verified deliveries remain at `/tmp/jesses-toolchain/grav1synth-static-qualified-v4/delivery` (63 MB) and `/tmp/jesses-toolchain/ffmpeg-full-package-v3/delivery` (259 MB); no durable copy is claimed. Temporary toolchains and media may be cleared at reboot. Validation logs were retained under `.git/checkpoints` above. Reconstruct required providers from checked-in recipes if temporary artifacts are absent.

## Shutdown preflight

Confirmed a physical Linux test workstation, systemd PID 1, virtualization `none`, and an active local X11/Cinnamon session; logind `CanPowerOff=yes`. Normal poweroff scheduling follows file sync. No blanket process kills were used.
