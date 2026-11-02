# Windows session handoff — 2026-11-02

**Work resumed on the user's later explicit instruction.** The original pause
and its receipts remain historical. See [the continuation ledger](windows-continuation-20261102.md)
for the completed acceptance checklist and subsequent evidence. Final audit
reported zero owned qualification processes; historical PIDs and earlier status
statements are not live inventory.
As of the resumed native qualification, the actual recovery gates are a
13-applicable-test composite across two runs, not one all-green run; 13 Windows
output-safety cases and 13 independently verified Dolby positives plus the exact
no-device negative have also passed against the staged providers. A fresh grav
delivery and its application gates, the refreshed app, full package and
RestrictedHost, separate packaged GUI smoke and disposable Scoop lifecycle have
since passed. FreshImage remains unqualified.
The RustDesk configuration session has released its UI
reservation; preserve its open connection. The
continuation ledger records evidence, qualifications and current limitations;
the sections below preserve the prior pause chronology.

The original Windows qualification scope in
[windows-local-validation.md](windows-local-validation.md) covered native desktop/media
workflows, CPU encoder providers, physical Vulkan/libplacebo processing, complete
portable ZIP, sanitized package gates, **RestrictedHost**, and disposable Scoop
lifecycle; the scoped existing-host work is complete subject to the explicit
limits in the continuation ledger. The user authorized using anime in the Windows Videos folder read only
and downloading separate HDR10, HDR10+ and Dolby Vision samples. Those downloads
are complete. The portable candidate and RestrictedHost were subsequently
qualified as described in the continuation ledger; FreshImage remains incomplete.

The checkout and all fixes are uncommitted. No commit, push, release publication,
global tool-default change or original-media edit was performed. Preserve the
working tree, source caches, failed trees, logs, receipts and generated outputs.

## Original resume plan (historical)

1. Read this handoff and the pause receipts below. Recheck actual live processes,
   resource headroom and the app/dev-server state; saved PIDs are historical.
   At pause, native/provider builds and watchdogs were confirmed absent. The
   desktop app and Vite server were left open and idle, not shut down.
2. Read the existing `owned-segment-list-focused.*` receipt/log. It was unreviewed
   at pause; handoff preparation read it: **exit 0, 29 passed, 0 failed**. Do not
   repeat that unchanged focused suite merely to rediscover its status. Finish
   the actual qualified Select/Hybrid stop/reopen/complete regression for the
   deterministic `segments.ffconcat` fix. The latest actual package recovery
   suite still has **6 passed / 2 failed**, despite its `*-green.json` filename.
3. Implement and qualify strict ownership for the Windows staged launch artifacts
   left by a hard crash. This is **diagnosis only**, not an implemented fix. Use
   concrete recorded ownership/file/directory identity proof; do not add generic
   `launch-*` or filename-only exemptions. Preserve completed chunks and reject
   injected/replaced files, directories and reparses. See the failure details below.
4. Finish grav1synth's cheap isolated-toolchain/link preflights, then rebuild its
   guarded source recipe into a **new destination from verified download cache**.
   Preserve the failed tree; do not reuse its compiled build as a fresh recipe.
   The workspace-isolation fix has synthetic/focused proof, not a qualified binary.
5. Run the applicable delivered-provider media/recovery/metric/grain gates. Review
   the added Hybrid and grav changes and later crash fix. Rerun appropriate Rust
   tests/Clippy after substantive source changes; the earlier broad pass is not a
   blanket pass for everything added afterward. Keep host, staged-provider and
   actual packaged results distinct.
6. After source fixes settle, refresh the cached release app, then stage the full
   closure including grav, collect/verify the portable ZIP, run extracted/sanitized
   probes and **RestrictedHost**, and exercise the actual packaged GUI and disposable
   Scoop install/update/uninstall/persist flow. Preserve old build/receipt hashes.
7. The pending independent checks of the 960-frame GUI recovery output and
   fresh GUI Dolby output were completed after resume; see the continuation
   ledger. Manual Shift-held desktop validation remains a gap:
   supported Computer Use exposes atomic key chords, not a sustained modifier
   hold/click across asynchronous preflight; the user cannot do that manual hold
   now. Do not automatically ask again or claim native/browser held-edit proof
   establishes the physical desktop gesture.

## Historical pause details and operating constraints

The remaining sections preserve the pause-time diagnosis and receipts, with a
few explicitly updated desktop rows. Their statements about work being pending
or fixes being unimplemented are historical; use the continuation ledger for
the completed resumed checks and current limits.

All paths below are relative to `%USERPROFILE%\repos\jesses` unless absolute.
The read-only comparison checkout is `%USERPROFILE%\repos\nmkoder`.
Base HEAD remains **`2c5dd9d6f636ff05e6fdad030c2f2a3bb007e079`**. Read-only
`git status --short` / `git diff --stat` at handoff preparation showed 39 modified
tracked files, 1,101 insertions/204 deletions, and four existing untracked docs/tests;
this new handoff adds another untracked document. Do not reset/revert others' edits.

| Symbol                         | Location                                                           |
| ------------------------------ | ------------------------------------------------------------------ |
| `N` — native receipts          | `target/test-artifacts/windows-native/20261102T185541Z-a192fabc`   |
| `D` — desktop receipts         | `target/test-artifacts/windows-desktop/20261102T191841Z-0e0fd24a`  |
| `F` — frontend receipts        | `target/test-artifacts/windows-frontend/20261102T185644Z-144003dc` |
| `E` — provider receipts        | `target/test-artifacts/windows-providers`                          |
| `P` — provider work/deliveries | `target/windows-providers-20261102`                                |
| Native compiler target         | `target/windows-small-qualification`                               |
| App release compiler target    | `target/windows-release-app-20261102`                              |

Pause evidence: `N/pause-20261102-checkpoint.json` (21:25:52 UTC, no owned active
native processes) and `E/paused-provider-status-20261102T212526Z.json` (21:25:26 UTC,
no owned provider builds/preflights/watchdogs). These are saved observations, not
a claim about processes in a future session. No cleanup was performed on pause.

The user's standing orchestration instructions apply: primary GPT-6 Astra/xhigh
only orchestrates; delegate substantive inspection, execution, editing and
verification. Use `complex_worker` (GPT-6.1 Sol/xhigh) for native/provider work,
`visual_worker` (GPT-6.1 Terra/xhigh) for GUI/visual work, and `simple_worker`
(GPT-6 Luna/max) for narrow mechanical work. Prefer named types and clean forks;
at most three spawned threads. Assign distinct ownership, tell workers they are
not alone, and allow only one GUI operator. Workers must not spawn further agents
without the parent's assignment. Delegation is already authorized; no extra
permission is needed just to use workers.

Environment recorded in receipts: Windows 11 Home 10.0.26300, PowerShell 7.6.9,
Node 24.21.0, Python 3.14.8, VS 2022 Build Tools/MSVC and WebView2 installed,
physical Radeon 880M. Use isolated Rust **1.98.1** under
`target/windows-prerequisites/{rustup,cargo}`; both MSVC and GNU standard-library
targets are installed. The ambient Scoop GNU Rust 1.99.0 is a different toolchain.
MSYS2 is `P/package-media-compiler/msys64` with `jesses-toolchain.json`.

- Do not change global PATH/default Rust/tool installs. Use child-only environment.
  Approval policy is never; do not pass `sandbox_permissions`.
- Native bounded runner: `N/run-bounded.ps1`, normally 10 GiB target ceiling,
  45 GiB free-disk floor, debug symbols/incremental off, `-BuildJobs 1` or 2.
  An earlier 4 GiB stop is retained; there was **no 6 GiB stop**.
- Provider total compiler budget is at most two jobs, 3 GiB available RAM floor,
  45 GiB free-disk floor and 50 GiB provider-root cap. Coordinate native Rust/link
  peaks; do not lower safety floors. Prefer stopping/resuming the cache-reusable
  app Cargo build over losing a fresh source-provider pass. App release uses one
  job and can use 4 GiB scheduling headroom before its unchanged 3 GiB safety floor.
- Video encoders remain CPU only. Physical Vulkan/libplacebo and optional Vship
  GPU metric processing are separate from GPU video encoding. Do not substitute
  software Vulkan for the physical-device claim or silently change Spline curves.
- Keep owned long-running tool sessions actively supervised until completion or
  explicit stop. A previous final checkpoint left session 86020 pending; it later
  disappeared with no process/result. Cause was not established. Do not return a
  final response with pending owned checks and assume they will survive.
- Do not delete repositories, original media, caches, receipts or failed builds.
  Use fresh evidence/staging/provider destinations. Do not update memories.

## Passed evidence and its limits

| Result                                  | Saved evidence / scope                                                                                                                                                                                                                                                                                                                                                                            |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Console 12/12 actual Win32 cases        | `N/console-qualification/20261102T190644Z-6cbed9570afb48bc8967b8e71dce567e/console-qualification.json`: hidden/visible, Unicode literal argv, real process pause/resume/cancel, console closure, AV1AN tail, readiness cancellation and concurrent helper shutdown.                                                                                                                               |
| Frontend                                | Frozen pnpm 11.19.0 install, Svelte check/build, 279/279 Playwright with no retries/skips/flakes. Initial layout receipts in `F/frontend-report.json`; later Copy Audio regression/fix and 279/279 rerun in `D/frontend-full.log`. Compact native control padding and Copy Audio preserving original channels are both in the current diff.                                                       |
| Observed broad Rust pass                | `N/workspace-tests-post-metadata-fix.result.json` and `-counts.json`: 468 passed, 0 failed, 328 ignored, 73 groups. `workspace-clippy-post-metadata-fix.result.json`: all-target `-D warnings` exit 0. This preceded the later Hybrid fix, test-harness updates and wording change.                                                                                                               |
| Interrupted attempt, not a pass         | `N/post-av1an-verification-interruption.json`: prior `workspace-tests-post-av1an-fixes` only compiled, no result/exit status; session 86020/PID 18272 absent. Later observed 468/0 run supersedes it.                                                                                                                                                                                             |
| Image cache release                     | `completed-preflight-release-green`: 4/4; IPC state registration 4/4. Actual GUI import/encode/remove immediately released the exact managed cache while source PNGs, output and success history remained. Active admission still rejects release.                                                                                                                                                |
| Concat/source cuts                      | `concat-owned-output-green`: 28/28; `source-cut-attachments-green`: 3/3; `source-cut-publication-green`: 4/4. Red Windows prefix, omitted-font and sharing-error-32 receipts retained.                                                                                                                                                                                                            |
| Latest focused Hybrid layout            | `owned-segment-list-focused.result.json` and stdout: exit 0, 29/29 at 21:23:01 UTC. Read during handoff preparation. Includes valid deterministic list and wrong-content/traversal, stale identity, extra-file, no-segment and NTFS junction negatives. Actual Select/Hybrid package rerun remains pending.                                                                                       |
| Source-provider audio/subtitles/filters | `core-audio-subtitle-filter-gates`: audio 11/11, source raw-filter/trim/30-image case 1/1, subtitles initially 3/4. `bitmap-subtitle-authored-clock-green`: remaining SDR/HDR bitmap case passes with unchanged pixel assertions after fixture `-copyts`; targeted `subtitle-fixture-clippy` passes. Failed fixture and host/source decoder comparison retained in `bitmap-subtitle-diagnostic/`. |
| Physical Spline and no-device behavior  | Host-development `host-spline-physical-vulkan` and `host-spline-no-vulkan-held` each pass. Source FFmpeg provider also passes physical Radeon 880M processing. The held Spline test is now Windows-enabled, explaining 328 rather than 327 ignored tests in the later broad run.                                                                                                                  |
| Source clock tests                      | `host-source-trim-matrix` 5/5; source-filter gate covers three trim modes/stateful filters/images. The Unix held-command wrapper case was explicitly not represented as Windows evidence.                                                                                                                                                                                                         |

Current uncommitted native fixes are grouped as follows; inspect the diff rather
than replacing files from any snapshot:

- `supervisor/console/windows.rs`: duplicate READY handle, FreeConsole before
  AllocConsole under CREATE_NO_WINDOW, cancellation-aware readiness and joined
  cleanup; diagnostic errors in the console fixture example.
- `jobs/files.rs`, `sequence.rs`, `jobs.rs`, `src-tauri/src/lib.rs`: directory-open
  flags, close/sync sequence receipt before publication, canonical cache-container
  comparison, read through the exclusive lease, correct managed IPC `Jobs` state,
  prune finished preflights under admission lock. No Source share-mode relaxation.
- `jobs/av1an/recovery.rs`: FILE_READ_ATTRIBUTES identity queries work with own
  exclusive lock; owned concat output handles Windows verbatim-prefix differences
  with canonical containment/identity/reparse/traversal checks. Source queue concat
  keeps Windows response files and non-Windows hierarchy without absurd usize lint.
- `source_cut_packets.rs`, `source_av1an_trim.rs`: codec-less fonts are required
  mapped attachments with the existing hash/metadata proofs; release only the
  completed writable temporary alias before immutable Source adoption, then verify
  the published identity. Original sources stay guarded.
- `external_tracks.rs`: narrowly accept FFprobe's omitted `und` language only for
  actual Matroska/WebM. MKVmerge/literal mux proof confirms the real language;
  empty/different languages, changed titles and unknown/ISO formats stay strict.
- Later `recovery_actions.rs` change is history wording only. Test changes include
  platform paths/quotes, artifact retention, explicit bitmap fixture timestamps,
  synchronous-or-queued exact rejection assertions, and absolute hidden Windows
  PowerShell process inventory for System32-only PATH. Provider scripts have their
  own reviewed job-limit/provenance/build fixes; see the provider checkpoint.

## Open native recovery failures

`N/core-av1an-recovery-green.json` has **exit 101**, not success. Its log ends
**6 passed / 2 failed**, with all 51 private runtime files unchanged. Four
reader/metric-dependent cases were deliberately skipped for the old base stage.
The first attempt `core-av1an-recovery.json` was 4/4 with stale harness assumptions;
those original red receipts remain. The corrected harness requires the exact
RECOVERY_INVALID codes/messages and unchanged foreign bytes in either rejection
branch; it does not accept generic failures.

| Failure                                                                | Current diagnosis / next action                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| ---------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `corrected_select_and_hybrid_stop_reopen_resume_exact_source`          | Hybrid creates/reuses `segments.ffconcat` for complete decoded-source comparison, then preflight formerly called it foreign. The applied fix shares `segment_list_text`, restricts numeric confined segment paths/current identities/regular files, and accepts only exact deterministic bytes with a Source guard and nonempty sealed set. Existing `load/validate_layout` and `verify_saved` retain full stamp checks. Focused unit 29/29 passes; run the real Select/Hybrid stop/reopen/complete case next. Snapshot/patch: `N/hybrid-recovery-review-snapshot.rs`, `hybrid-recovery-review.patch`. |
| `windows_hard_crash_cleanup_reopens_rejects_changed_tool_and_recovers` | After a real hard crash, preflight rejects nonempty `launch-11720-3` containing staged `av1an.exe`. Exact copied engine SHA matches the pinned engine, but there is no recorded launch directory/file identity authority in the current manifest. No implementation was added. Design durable ownership recording before execution and strict recovery/cleanup; do not merely allow a matching name or adopt arbitrary bytes. Preserve injected-file/directory/reparse/replacement negatives.                                                                                                          |

Preserved crash tree:
`%USERPROFILE%\AppData\Local\Temp\jesses-recovery-22624-1793654047727367500\.jesses-11720-1793654048522124100-1.av1an`.
The failing test intentionally changed a tool fingerprint; do not interpret that
tamper as a real installed-binary change or edit provider binaries to fix it.
`Launch::prepare` in `jobs/av1an/launcher.rs` and its calls/cleanup in
`jobs/av1an.rs` are the staging entrypoints. The latest applied fix concerns only
Hybrid; no generic launch exemption or new launch manifest field exists yet.

Historical read-only status commands (saved results, not current process state):

```powershell
$N = "${env:USERPROFILE}\repos\jesses\target\test-artifacts\windows-native\20261102T185541Z-a192fabc"
Get-Content "$N\owned-segment-list-focused.result.json"
Get-Content "$N\owned-segment-list-focused.stdout.log" -Tail 12
Get-Content "$N\core-av1an-recovery-green.json"
Get-Content "$N\core-av1an-recovery-green.log" -Tail 55
```

## Intentional Source New Settings contract

Do **not** change saved preset/CRF execution to match a new global argv display.
For `av1anOptions.sourceWorkflow=true`, the saved scene/encoder queue remains
authoritative. The pinned engine reloads it verbatim, removes only completed
entries, and executes each chunk's saved `video_params`; current FFmpeg filters,
concat settings and worker scheduling are separate. Historical explicit API
requests with the marker false/omitted use the older rebuild policy.

Authority: [source-av1an-chunks.md](source-av1an-chunks.md), Source New Settings
authority; [source-av1an-recovery-actions.md](source-av1an-recovery-actions.md).
Pinned 805dad6 `context.rs`: saved video parameters at 539–554, current filter pipe
at 630, saved queue reload at 1358–1374. The GUI's veryslow→ultrafast observation
matches this contract. `N/new-settings-diagnostic/contract-verification.json`
records all ten saved parameter arrays and the actual old-preset child, plus four
unchanged completed-file/receipt checks between native snapshots. **No pre-resume
hash snapshot was captured.** Source history wording now explicitly names retained
per-chunk preset/CRF; no queue-authority behavior changed.

## Provider closure: eight complete, ninth incomplete

The exact immutable inventory/hashes and qualification paths are in
`E/provider-checkpoint.json`; its `allProviderDeliveriesPresent=false` correctly
excludes grav. The following roots are under `P/`. SHA values below are identifying
prefixes; use the checkpoint/provenance for the full 64-character digests.

| Completed delivery             | Identity                                                                  | Native evidence                                                                                                                                                                                                                                          |
| ------------------------------ | ------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `package-ffmpeg/delivery`      | FFmpeg/FFprobe 9.0.1; `5b5a076bb55d` / `af2b4865b5d2`                     | 42/42 cases, `E/ffmpeg-native-smoke/qualification.json`; 184 encoders, zero hardware encoders, CPU 8/10-bit decode, audio/fonts and physical Vulkan/Spline.                                                                                              |
| `package-x264/delivery`        | 0.165.3222 b35605a; `3b03ce77d126`                                        | 8/8, `E/x264-native-smoke/qualification.json`.                                                                                                                                                                                                           |
| `package-svt/delivery`         | mainline SVT-AV1 4.2.0; `fd218e43476d`                                    | 8/8, `E/svt-native-smoke/qualification.json`. Source SVT binding still uses the separately pinned HDR fork.                                                                                                                                              |
| `package-aom/delivery`         | aomenc 3.14.1; `f3cdb2ef60d2`                                             | `E/aom-run.json` exit 0; native IQ/SSIMULACRA2 10-bit cases and vertical-chroma patch provenance.                                                                                                                                                        |
| `package-vpx/delivery`         | vpxenc 1.17.0; `fec66a8dc0b6`                                             | 8/8, `E/vpx-native-smoke/qualification.json`.                                                                                                                                                                                                            |
| `package-x265/delivery`        | 4.3+1-e9b8812, 8/10/12-bit; `0e1b1de99742`                                | 8/8, `E/x265-native-smoke-retry1/qualification.json`; three required GCC runtime DLLs and pinned source closure included. Initial wrong-bin-path smoke error retained separately.                                                                        |
| `package-av1an-vship/delivery` | AV1AN **805dad69143fa0a81cfe2fb89c0b9e90a828ea72**, engine `ed3276831f91` | Final merged runtime, not the older `package-av1an` base. `qualification/summary.json`: decoder, all three CPU metrics, six QTGMC presets ×16 frames, Vship finite GPU selection on Radeon 880M and CPU fallback. Eight compatibility markers unchanged. |
| `package-mkvmerge/delivery`    | MKVmerge 97.0                                                             | Pinned MSYS binary/source/DLL closure; `delivery/qualification/native-mux-and-identify.log`.                                                                                                                                                             |

Final merged AV1AN provenance SHA-256:
`c2c508b22ce90b80ea83ba71aecf75d790846c9a50d64dc0fd214daedb9a7db9`.
It accounts for 65 support files, 49 additional sources, 621 notices and 44 build
inputs. The bounded MSVC scorer fix changes only the outer libjxl ConvolveRow
forced-inline annotation; `/O2 /Ob2 /MT` and CPU targets remain. Source-built versus
matching official Julek r3 gave maximum numerical difference **0.0** across eight
identity/distortion frames for all three metrics; private runtime stayed unchanged.
See `E/scorer-metric-comparison-marked/qualification.json`, retained patch/provenance
and `scorers-retry2-run.json`. Timing is not cross-compiler reproducibility proof.

### Grav1synth must still be built and qualified

`P/package-grav1synth` is a preserved **failed** tree, not a delivery. Private
FFmpeg **8.1** and dav1d 1.5.4 built/installed, then Cargo vendor exited 101 because
the extracted standalone crate inherited Jesses' enclosing workspace.
`E/grav1synth-run.json` records exit 1 at 21:19:48 UTC, no resource stop.

The guarded recipe fix is implemented in `scripts/build-package-grav1synth.py`:
check pristine Cargo.toml SHA
`434378bf50e9ec3663f7826c75e92a7bde15a32eef508a477dec9cd13eccccd5`,
append only an empty `[workspace]` to the **new working copy**, and retain pristine
and working manifests, `Cargo-workspace.patch`, sourcePatches and build-input
records in a future fresh delivery. Root Cargo.toml and the failed source tree
were not changed. `E/grav-workspace-probe/qualification.json` is synthetic Cargo
metadata red 101 → green 0; focused regressions are 7/7. This is an **unbuilt recipe
fix**, with final fresh-delivery/provenance review pending.

Before rebuilding, do the parent-requested cheap exact isolated Cargo/Rust GNU
target/linker/libclang/private-8.1 static-link preflights. They were not started.
`E/run-grav-isolated.py` sets child Rust homes and
`LIBCLANG_PATH=%USERPROFILE%\scoop\apps\llvm\23.1.3\bin`; exact launcher and
libclang evidence is under `E/grav-*`. Verified sources are cached in
`target/tool-download-cache` and `P/grav-source-cache`. Never substitute public
FFmpeg 9 for grav's private 8.1 dependency.

After explicit resume and successful preflights, the intended **fresh** recipe is:

```powershell
python target/test-artifacts/windows-providers/bounded-provider-run.py --name grav1synth-retry1 --root target/windows-providers-20261102 --evidence target/test-artifacts/windows-providers --ram-floor-gib 3 --poll-seconds 2 -- python target/test-artifacts/windows-providers/run-grav-isolated.py scripts/build-package-grav1synth.py --destination target/windows-providers-20261102/package-grav1synth-retry1 --msys-root target/windows-providers-20261102/package-media-compiler/msys64 --cache target/tool-download-cache --jobs 2
```

After source/import/CLI/inventory proof, `grain_provider` and `utility_containers`
application media gates against the delivered binary remain mandatory. CLI alone
does not qualify grain application. Do not reuse the failed compiled tree.

## Build/package resume references

Remaining native coverage must also stay explicit: delivered-provider Dolby
HDR10-compatible-base CPU/Auto fallback, the remaining applicable source Dolby
trim/caption/rotation/VFR/AV1AN combinations, and Windows standalone/remux/mux
finalization/output-safety gates have not all been qualified. Use
[source-dolby-combinations.md](source-dolby-combinations.md) and each ignored test's
requirements to generate native inputs/jobs; Linux wrappers/logs are not Windows
evidence. The 5/5 host source-trim matrix and successful single GUI input-seek
Dolby case do not establish every combination. Complete the applicable cases,
without blindly running or claiming all ignored tests.

- `target/windows-core-stage-20261102T2032`: old base AV1AN/core subset; sanitized
  discovery passed eight required roles. The 6/2 suite used this stage, so four
  reader/metric cases were skipped.
- `target/windows-media-stage-20261102T2110`: all completed providers including
  merged AV1AN, AOM/VPX/x265; grav absent. `N/media-stage.result.json` and
  `media-package-discovery.result.json` exit 0, sanitized discovery of 11 roles.
  This is **not** the final distribution.
- The release executable already built twice, independently of runtime tool
  closure. Latest `N/app-release-clarified.result.json` exit 0 at 21:18:29 UTC,
  SHA-256 `dc6b42d528e054df31876c6197d9930c632d41a1567f404d5f25e869154e27af`,
  path `target/windows-release-app-20261102/x86_64-pc-windows-msvc/release/jesses.exe`.
  It predates the Hybrid fix and future crash fix, so **is not the final binary**.
  The first build is preserved as `N/app-release-initial.exe`; retain the current
  successful executable/hash before overwriting it during a later refresh.
- `N/run-app-release.ps1 -Resume -Name <fresh-receipt-name> -SchedulingFloorGiB 4`
  reuses the owned compiler cache, builds with one job, and keeps separate logs.
  Its artifact-only Tauri config skips the redundant frontend hook; existing
  verified dist inputs are recorded in `*-frontend-inputs.json`. If frontend
  changes, rebuild it first with the pinned CLI below.
- Use isolated pnpm **`F/tooling/node_modules/pnpm/bin/pnpm.mjs`** with Node, not
  the compatibility pnpm.cjs: the latter's implicit pre-script install selected
  the Codex global pnpm 11.25. Exact successful argv are in `F/*.command.json`.
- Use fresh receipt names with `N/run-bounded.ps1`; old
  `run-final-native-checks.ps1` contains old names from the interrupted attempt,
  so do not invoke it blindly. Expected native environment: isolated Rust homes,
  `CARGO_INCREMENTAL=0`, profile debug 0, locked dependencies and bounded jobs.
- After rebuilding the actual test executable, rerun
  `scripts/qualify-package-av1an.py --runner <current av1an_recovery test exe> --resources <stage> --receipt <fresh json>`.
  The last corrected runner was
  `target/windows-small-qualification/debug/deps/av1an_recovery-876ab79c019b04ef.exe`,
  which predates Hybrid. Do not reuse a stale hash by filename assumption.
  For the final merged runtime, review and run the previously skipped reader and
  metric cases rather than retaining base-stage skips automatically.
- Full staging uses `scripts/stage-bundled-tools.py` with all eight delivery args
  above plus `--grav1synth-build <new qualified grav delivery>` and the pinned
  SVT download cache. Use a new destination. Final collection/probes/extraction
  are in [packaging.md](packaging.md); RestrictedHost inputs/commands are in
  [windows-clean-qualification.md](windows-clean-qualification.md). Set
  `-Scope RestrictedHost` explicitly; the runner otherwise defaults FreshImage.
  Actual packaged GUI and disposable `qualify-scoop.py` lifecycle are separate.

## Desktop/media evidence and remaining independent checks

`D/desktop-evidence-index.md` is the detailed GUI index, screenshots and profiles;
some older pending paragraphs are stale. Prefer the exact native receipts below.
The active development profile was `D/pinned-base-app/jesses-data`, with launch
recipe `D/pinned-base-app/launch-pinned.ps1` and `provider-identities.json`.
That recipe uses the **base** delivery and process-only overrides, not a final
package environment. Recheck live app/server state after resume and use a fresh
isolated copy/profile for final packaged validation.

| Actual desktop result                                    | Independent status                                                                                                                                                                                                                                                                                                                                                                                            |
| -------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Anime Quick Convert `D/anime-hevc.mkv`, 2–4 s input seek | Passed full decode, 48 video frames, 25 streams, 11 copied audio/subtitle packet sequences plus exact 13 fonts; `N/gui-quick-convert-verification/`. Container 5.223 s includes preroll/subtitle ends.                                                                                                                                                                                                        |
| Five-PNG import/HEVC and PNG export                      | Encoded five frames independently decoded at 30 fps; inputs remained. All five exports subsequently matched their input frames after RGB24 decode; `D/continuation-independent/verification.json` and `assertions.json`. Cache-release retest passed without removing the older unrelated cache.                                                                                                              |
| Pinned-base AV1AN `D/pinned-anime-av1an.mkv`             | Independent 48-frame 1080p decode, all 25 streams, ten subtitles, exact 13 fonts and 95,232 matched audio samples; `N/av1an-native-retries/pinned-gui-independent-verification.json`.                                                                                                                                                                                                                         |
| Pause/Resume/Stop and Saved recovery                     | Real AV1AN/x264 CPU counters frozen for four seconds, then advanced; Stop workers exited. Saved recovery reached Done. Screenshots/process JSON in `D`; app output validators passed.                                                                                                                                                                                                                         |
| New → Saved recovery `D/pinned-new-work.mkv`             | GUI/app reports Done. Subsequent independent FFprobe/full CPU decode confirms 960 frames, 40.039 s, 1920×1080 H.264 and 24000/1001 fps; `D/continuation-independent/verification.json`. Source semantics above are intentional. Job `24520-1793652272577160800-2`.                                                                                                                                            |
| HDR10 `D/hdr10-spline-gui.mkv`                           | Independently passed 61 decoded 4K frames at 30000/1001, BT.709 SDR, monotonic 0..2.002 s, stale HDR metadata absent; `N/gui-hdr10-verification/verification.json`. Physical Spline processing, CPU x264.                                                                                                                                                                                                     |
| Original Dolby request through native JobManager         | Independently passed 60 SDR frames/RPU Spline, correct Matroska titles/und languages, full audio decode; all 63 E-AC-3 packet payloads and PTS/DTS/durations match literal source-seek reference. `N/dolby-gui-metadata/independent-verification.json` and `literal-source-seek-audio-verification.json`. Video 1.016..2.000 s/audio 0/container 2.017 s is verified source preroll, not a widened tolerance. |
| Fresh actual GUI Dolby `D/dolby-spline-fixed-gui.mkv`    | Independent full video/audio decode passed; 60 BT.709 frames, expected titles and Matroska `und` languages, and 63 exact audio packets against the literal input-seek reference. `D/continuation-independent/verification.json`. Job `24520-1793653798668700000-14`.                                                                                                                                          |
| Layout/imports/held gesture                              | Native compact/wide panels and separate audio/subtitle picker imports observed; CPU-only encoder menu observed. Manual Shift hold still pending for the reason above. Browser/native held-edit results remain separate.                                                                                                                                                                                       |

## Original media and all three downloaded samples

Original anime is in `%USERPROFILE%\Videos`; do not copy the library or expose
unnecessary personal filenames. The selected source is 304,077,073 bytes, 1080p
AV1 10-bit BT.709, Japanese AAC, ten subtitles and 13 font attachments (25 streams).
`N/anime-source-identity-before.json` / `-after.json` prove unchanged length and
SHA-256 after validation. The working excerpt `N/anime-copy-excerpt.mkv` is a
generated stream copy with preroll; it is not an exact eight-second cut.

**Consolidated absolute paths, full hashes, URLs/usage notes and probe receipts:**
`N/hdr-samples-manifest.json`.

| Original sample | Verified contents / location                                                                                                                                                                                                                                                                                                                         |
| --------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| HDR10           | `target/test-artifacts/windows-hdr-samples/20261102T191204Z-7e122e02/bytedance-hdr10-4k30-original.mp4`; 5,409,945 bytes, 20.02 s, HEVC Main10 3840×2160 PQ/BT.2020, static mastering/CLL on all 600 decoded frames, no HDR10+/DV data.                                                                                                              |
| HDR10+          | `N/hdr10plus-playable/webmproject-rooftop-hdr10plus-original.mp4`; 7,153,214 bytes, 12.066667 s, AV1 1280×720 PQ/BT.2020, ST 2094-40 on all 362 frames. AV1 chroma_sample_position=0 is unknown; strict app Spline rejection is expected. Do not invent chroma siting. The older one-frame FATE diagnostic is supplemental, not the playable sample. |
| Dolby Vision    | `N/hdr-fixtures/Dolby-GlassBlowing2-P81-original.mp4`; 344,509,829 bytes, 178.112 s, HEVC Main10 1080p59.94, P8.1 RPU and HDR10-compatible base. SHA-256 `8e86ed6bb71cc71ec0394dad274c335fcfeaaa6801aed3abcea5ee98b6cd1168` matches the original handoff.                                                                                            |

Original downloads are preserved. Public publisher provenance/license notes are
retained; do not infer redistribution permission where no sample-specific grant
was supplied. Host FFprobe 9.0.2 sample metadata evidence is not a package-provider
qualification. The standalone source FFmpeg delivery is 9.0.1; keep them distinct.

## Review and reporting status

Independent read-only review found no actionable regression in the earlier
31 tracked diffs plus four new docs/tests. Later review checked metadata fixes,
fixture corrections, scorer patch/provenance and the stable Hybrid snapshot
(`0a4f40d385cf8463180070e614d6f47f9c70dacffa3aa93c56fb74699dcb93cb`)
without an actionable code finding. That is not the missing actual Hybrid gate
or final integrated review. Final Hybrid/grav delivery/integration review and
the as-yet-unimplemented crash fix review remain pending.

[windows-native-validation-20261102.md](windows-native-validation-20261102.md)
and `N/native-checks-ledger.json` retain the earlier chronology but are not fully
current at the pause. This handoff, exact result JSON/logs, provider checkpoint and
desktop index supersede stale status paragraphs. Never infer success from a
filename, an old binary, a compile-only result or the count of ignored tests.
