# Windows qualification continuation — 2026-11-02

The user explicitly resumed the paused Windows qualification. The
[session handoff](windows-session-handoff-20261102.md) preserves the pause state,
prior failures, exact paths and constraints. The scoped Windows host, staged
providers, portable package, RestrictedHost, actual GUI and disposable Scoop
checks described below have completed. FreshImage and the other explicit limits
below remain separate and unqualified. The final
[combined provider/package/GUI/Scoop receipt](../target/test-artifacts/windows-providers/provider-package-gui-scoop-final-qualification-resume1.json)
passed, and the [post-reopen host, artifact and process audit](../target/test-artifacts/windows-providers/final-scoped-host-artifact-process-checks-resume1.json)
confirmed unchanged scoped host and candidate/extraction payloads, with zero
owned processes. These do not replace the individual receipts.

Base HEAD was `2c5dd9d6f636ff05e6fdad030c2f2a3bb007e079` at handoff. Preserve
the uncommitted shared worktree, original media, caches, failed trees, and old
receipts. No commit, push, publication, global defaults change, or cleanup is
authorized by this continuation. Encoders are CPU only; physical Vulkan and
libplacebo processing remain in scope. At resume the parent observed about
365.5 GiB free, so cleanup is unnecessary. Saved PIDs are historical.
`N` below is `target/test-artifacts/windows-native/20261102T185541Z-a192fabc`.

## Qualification checklist

- [x] Win32 console lifecycle, 12/12 actual cases; see the session handoff for
  the receipt and limits.
- [x] Development-host frontend check/build and 279/279 Playwright tests at the
  earlier snapshot; later changes require checks appropriate to those changes.
- [x] Development-host desktop import, track/picker, CPU menu, compact/wide
  layouts, Quick Convert, image sequence, AV1AN and Pause/Resume/Stop, including
  independent output checks described in the handoff and below.
- [x] Physical Radeon 880M Spline processing and no-Vulkan explicit rejection
  on the host; this is separate from final bundled-provider qualification.
- [x] Actual staged-provider recovery qualification: 13 applicable tests across
  two compatible runs, including Select/Hybrid and hard crash. This is a
  composite, **not a single all-green run**; see the native evidence below.
- [x] Fresh grav source delivery, provenance and application grain/container
  gates: final delivery passed 2/2 grain and 3/3 utility cases. Earlier failed
  trees and diagnostic probes remain separate evidence.
- [x] Windows standalone, output, and remux/mux safety: 13/13 in three suites,
  including the native AAC loop fixture. These are staged-provider results; final
  package checks remain separate.
- [x] Delivered-provider Dolby source matrix: 13 positive combinations and the
  exact Spline no-device rejection independently verified; scoped to the staged
  runtime, not the final portable package. See the aggregate below.
- [x] Refreshed release app, full staging, portable ZIP, sanitized extracted
  probes and `RestrictedHost` passed; receipts and scope below.
- [x] Actual extracted portable GUI smoke using a fresh isolated profile: 12/12
  tools reported, CPU encoder menu observed and Quick Convert plus AV1AN outputs
  independently decoded. See evidence below.
- [x] Disposable Scoop lifecycle and actual shim-launched media job, with
  post-reinstall preference and history observations. RestrictedHost is not
  FreshImage; the isolated Scoop adapter does not qualify user PATH registration.
- [ ] FreshImage qualification remains separate and unperformed.
- [ ] Physical desktop Shift-held editing remains unverified: supported computer
  use provides atomic chords, not a continuous hold across async preflight, and
  the user cannot perform the hold now. Native and browser checks are separate
  evidence; do not claim they establish that physical gesture or ask again now.

The native follow-up matrix is anchored to
[source Dolby combinations](source-dolby-combinations.md): P8.1 CPU Hable and
Auto/Hable no-Vulkan fallback; InputSeek, Exact and Frame trim with source-clock
audio; text/PGS burn; rotation and VFR derivatives; FLAC sample proof; legacy
explicit API; and source AV1AN compatible-base positive-origin/VFR. The physical
Spline and explicit no-device host proofs already exist, but do not establish
every delivered-provider or package combination. Relevant Windows-specific
ignored cases are in `crates/media-runtime/tests/windows_finalization_safety.rs`
(four), `windows_output_safety.rs` (four), and
`windows_remux_mux_finalization_safety.rs` (five). The latter included both
AAC-loop cases with a native fixture. Under `N/external-gates-resume1`, receipts
`windows-output-safety-short-temp.json`, `windows-finalization-safety.json` and
`windows-remux-mux-safety-aac.json` record 4/4, 4/4 and 5/5 respectively, with
all 65 staged runtime files unchanged and a sanitized System32-only environment.
The native AAC fixture's applicability is recorded in
`aac-loop-fixture-applicability.json`. The initial 2/4 long-TEMP output-safety
failure is retained separately in `windows-output-safety.json`.

## Resumed native and provider status

`N/native-recovery-qualification-resumed.json` is the composite receipt: 13
applicable recovery tests passed across two compatible source snapshots. The
earlier all-12 attempt remains **10 passed / 2 failed**; the targeted follow-up
passed Select/Hybrid, Segment fixed-chunk recovery, and the exact absent optional
reader rejection, 3/3. FFMS2 and BestSource are optional external readers, not
bundled; their positive workflows remain unqualified. All 65 staged runtime
files remained unchanged. Preserve both runs and their different runner hashes.

The real forced hard-crash case retained a completed chunk, rejected a changed
tool, reopened and completed with full decode and source unchanged. Strict launch
ownership records a directory reservation before preparation and seals exact
image identity before execution; guarded cleanup rejects injection, replacement
and reparses. The separate `N/launch-reservation-recovery-green.result.json`
records 39/39 unit cases. Unsealed child-created concat or scene-prepass artifacts
after a crash still fail closed; this does not claim every crash point recovers.
Independent source review found no remaining actionable finding for its reviewed
snapshot. The broader `N/workspace-tests-post-launch-ownership.result.json`
records 479 passed, 0 failed, 328 ignored across 73 groups, before two new
ignored tests and a Clippy-only syntax update; the subsequent
`N/workspace-clippy-post-launch-ownership-green.result.json` exited 0.

An AV1AN working directory over 260 characters created under a deliberately
long nested TEMP failed with Windows error 267; a standard TEMP passed. This is
a retained limitation. The earlier long-argv console qualification proves a
different path and does not establish long AV1AN working-directory support.

[External gate receipt](../target/test-artifacts/windows-native/20261102T185541Z-a192fabc/external-gates-resume1/qualification.json), SHA-256
`0f3d3df214f2828464d48c01694902f47d5c0946cd96b4f2b4f83aac170f89f8`,
records 13 safety cases, 13 independently verified Dolby positives and the
expected Spline no-device rejection: 27 logical passes. It covers the matrix
above using the delivered-provider media stage, CPU video encoders and physical
Vulkan where applicable. Both source AV1AN outputs match every one of 240 coded
video payloads and their timestamps against fresh pinned-engine references; the
FLAC output matches 192,000 decoded 24-bit PCM samples per channel across six
channels. All 65 runtime files and the original Dolby hash remained unchanged;
all 11 tool binary hashes were verified. The worker's final inventory records
no owned media workers at 22:47:21 UTC. Fixture and reference corrections,
including the initial track-order and keyint mismatches, are documented in the
aggregate and preserved alongside their red evidence; no product edit or
relaxed assertion is claimed.

The second fresh grav1synth build failed in actual bindgen; its preserved
`target/test-artifacts/windows-providers/grav1synth-retry2-run.json` and log are
not a delivery. The exact pinned-bindings probe and restricted/provenance checks
subsequently passed: `target/test-artifacts/windows-providers/grav-exact-bindings-probe-resume3-run.json`
and `grav-exact-bindings-restricted-qualification-resume3-utf8.json`. The reviewed
recipe beginning `bfb0aead` was used for fresh retry3, which produced the final delivery in
`target/windows-providers-20261102/package-grav1synth-retry3/delivery`; the
executable SHA begins `59009323` and provenance SHA begins `aa13d519`. An earlier cheap
link preflight did not prove actual binding generation. Diagnostic probe2
application coverage passed 2/2 grain and 3/3 utility-container cases, with 14
retained tables, in `N/grav-diagnostic-probe2/summary.json`. The final delivery
then passed 2/2 grain and 3/3 utility cases in
`N/grav-final-provider-resume1/qualification.json` (SHA prefix `a615f44a`),
with stage and delivery inventories unchanged. The previous release executable
was preserved and the refreshed app built; frontend inputs were unchanged.
`N/final-native-release-and-provider-gates-resume1.json` binds the release,
frontend and five final application gates. The current
package probe SHA-256 is
`bcb25a01313ad2be28b037a8e8c4d1039e9ec702b4c6c11a09f960b7dd937f4a`,
superseding the prior `7ca…` probe. `target/test-artifacts/windows-providers/`
contains `provider-final-readiness-resume1.json`,
`final-full-stage-qualification-resume1.json`,
`final-package-nonui-qualification-resume1.json`,
`restricted-host-resume1/qualification.json`, and independent final-stage and
final-ZIP audits. RestrictedHost is an existing host, not FreshImage. The
subsequent GUI and Scoop checks are recorded below.

## Independent GUI output checks after resume

Evidence root `D` is `target/test-artifacts/windows-desktop/20261102T191841Z-0e0fd24a`.
The executable tools, exact commands, raw stdout/stderr, stream metadata, and
hashes are recorded in `D/continuation-independent/verification.json` and the
adjacent files; explicit assertions passed 16/16 in `assertions.json`. All
decodes used the source-built FFmpeg 9.0.1 delivery with two
decoder threads; these are checks of earlier development GUI outputs, not a final
portable GUI run.

- `pinned-new-work.mkv`, SHA-256
  `d46e57f0c899dc0dbeb0a179275a0ad19b13976019fbd8a543fc1cb0b6a719b5`:
  full video decode exit 0; 960 H.264 1920×1080 frames at 24000/1001, container
  duration 40.039 s. The saved-queue contract is described in the handoff.
- `dolby-spline-fixed-gui.mkv`, SHA-256
  `e38aae820119fe5dcd40546a1da2ef79c8fd61c3f1b27d9c3e48d94e30814e5a`:
  full video/audio decode exit 0; 60 H.264 1920×1080 BT.709 frames with monotonic
  video timestamps 1.016–2.000 s. Decoded frame side data contains only an
  unregistered SEI entry; stale HDR/Dolby frame metadata is absent. MKVmerge
  identifies the expected Bento4 video/audio titles and actual `und` languages.
  All 63 E-AC-3 packet payload hashes, sizes, PTS, DTS and durations match the
  preserved literal FFmpeg 11-second input-seek reference exactly. Its 2.017 s
  container and audio at 0 are source-seek preroll, not a widened tolerance.
- Five source PNGs and five exports compare equal as decoded RGB24 pixel bytes,
  one paired frame at a time; SHA-256 values and commands are recorded. The
  earlier index's pending pixel check is superseded by this result.

## Portable and Scoop GUI results

The parent confirmed the RustDesk configuration session released its exclusive
UI reservation. The separately verified fresh GUI copy was used without altering
the audited extraction.

The collected [portable ZIP](../target/windows-portable-package-20261102-resume1/jesses_1.0.1_x86_64-pc-windows-msvc_release_portable.zip) is
1,821,427,766 bytes, SHA-256
`16c6ebe788ed65f1d5a527498a19da3a830b13fc10e6770bd280b39faa660e62`;
the candidate app SHA-256 is
`b0759443b4592f976005d97c28fe909512cedc4171def67571b0a781daaaae05`.
These identities are reported by the packaging worker. Sanitized extracted-package,
RestrictedHost, the separate GUI smoke and disposable Scoop lifecycle passed.

The verified separate GUI copy, `target/windows-packaged-gui-20261102-resume1`,
matched 1,228 files; `target/test-artifacts/windows-providers/final-gui-copy-ready-resume1.json`
records its app and manifest identities. It launched with a fresh portable data
root, child-only System32 PATH, isolated profile directories and no `JESSES_*`
overrides (`final-gui-launch-resume1.json`). The actual window reported 12/12
tools and showed x264, x265, VPX, SVT and AOM software encoder choices; its
film-stock selector exposed Super8, though no GUI grain job was run. Screenshots
`final-gui-start-resume1.png`, `final-gui-encoders-resume1.png`,
`final-gui-grain-controls-resume1.png`, and the two `*-done-resume1.png` files
are under `target/test-artifacts/windows-providers/`.

The packaged app completed an x264 Quick Convert and a one-worker AV1AN
LSMASH/MKVMerge x264 job. Independent packaged FFmpeg/FFprobe full decodes and
explicit assertions in `final-gui-independent-resume1.json` report 192 H.264
1920×1080 frames at 24000/1001 for each output. Actual requests, success history
and logs remain in the GUI copy's `jesses-data/`; the input fixture hash and
output hashes are in the independent receipt. The field is labeled “Output
Path” without visible stem help; the documented source contract treats it as an
extensionless stem and appends the selected container extension. The entered
`.mkv` was retained, so both exact resulting files end `.mkv.mkv`; these observed
paths were not renamed. The GUI copy closed normally and a process inventory
showed no `jesses.exe` before the disposable Scoop snapshot.

Historical read-only inventory at approximately 22:01 UTC returned no targetable windows
or running Jesses entry through either computer-use inventory. The helper
responded; Explorer existed in Windows session 1, and no LockApp/LogonUI process
was observed. At that point capture and input were unproved; it was not a
determination that the desktop was locked. Subsequent actual GUI work passed.

The disposable Scoop prepare/install succeeded (`target/test-artifacts/windows-providers/scoop-install-ready-resume1.json`).
The actual generated shim launched the matching app using the disposable root
and isolated child environment (`scoop-gui-launch-resume1.json`). It reported
12/12 tools, completed an x264 Quick Convert job and closed normally. Before
closure the GUI's zero-indexed stream preference was enabled; afterward the
persisted `preferences.json` contained `zeroIndexedStreams: true`, and the
persisted `jobs.json` recorded succeeded job `19556-1793661645244982400-2`.
Required config, data, cache, logs, WebView cache and job-history paths existed.
`scoop-gui-independent-resume1.json` records full decode of the resulting 192
H.264 1920×1080 frames at 24000/1001, exit 0, and an unchanged input hash
relative to the earlier GUI read. `scoop-gui-setting-resume1.png` and
`scoop-gui-done-resume1.png` record the UI. No Jesses process remained when the
provider began the update/reset/uninstall/reinstall checks. All five lifecycle
commands exited 0. At the closed-app boundary, the
[Scoop lifecycle receipt](../target/test-artifacts/windows-providers/scoop-lifecycle-boundary-resume1.json)
records exact equality of 181 persisted
files, 8,881,908 bytes, and the scoped host snapshot. Both test version labels
use the same archive; this establishes relocation/persistence, not compatibility
between different release binaries or user PATH registration.

The reinstalled `0.1.1-scoop-test` shim target had the same app SHA and reopened
successfully. The preference was visibly checked in Settings, and the original
successful job was verified in persisted `jobs.json` while the app was open;
the current UI did not expose a history list. See
`scoop-gui-reopen-observation-resume1.json` and
`scoop-gui-reopened-preference-resume1.png`. The app closed normally and
`scoop-gui-reopen-closed-resume1.json` recorded zero Jesses processes at
23:25:44 UTC. This reopening occurred after the exact CLI persistence boundary;
normal startup changed WebView/cache files; the final audit records the separate
delta of 30 changed, 20 added and 1 removed file.
