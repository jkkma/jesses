# Source AV1AN chunk and resume authority

The source's visible Split Method `None` disables scene detection, while `-x`
still subdivides long scenes (`Av1anUi.cs:1093–1098`, `Av1an.cs:510–540`).
`CodecUtils.cs:171–181` calculates that frame limit from the typed post-filter
rate and global keyframe seconds, using float32 rounding to nearest/ties-even
and a12–480-frame clamp. Arbitrary raw filters execute later and do not retune
this captured value. Source scene mode derives an even adaptive analysis height
from the original height (`Av1an.cs:1251–1293`); None omits this scene-only flag.

Optional `Av1anOptions.sourceWorkflow` selects the source contract without a UI
control. It is omitted/false for historical explicit API requests. Source draft
maximum/minimum/downscale/method values are inert: effective source cap, standard
algorithm, minimum24 and adaptive downscale are derived before the command,
scene prepass and saved-receipt validation. Source None emits no scene-only
flags. The typed rate is retained separately from later observed raw-filter
output. Preview, held requests and batches carry the same marker and immutable
preferences.

Current green evidence in
`target/migration-validation/source-av1an-trim-20261101/`:

- `chunk-options-unit.log`:6/6, including source float32 rounding/clamps,
  adaptive-height thresholds, inert hidden drafts and unchanged explicit API.
- `chunk-rate-unit.log`:1/1, typed30fps×2seconds remains60 after observing raw
  filter output12fps; None omits all scene-only flags.
- `chunk-browser.log`:2/2, hidden values are inert in source validation and the
  AV1AN request submits the source marker without another visible setting.
- `source-chunks-native.log`:2/2, including real native None/Saved execution
  and the initial pinned-engine partial-resume investigation.
- `source-none-chunks.json`:30fps×1second gives cap30; the pinned engine balances
  the120-frame scene into5×24-frame chunks. A visible scene change does not
  create another split under None. Source-equivalent independent engine scene
  output matches, Saved retains exact queue bytes, and the final output retains
  all120 lossless decoded frames.

## Source New Settings authority

The source Resume window imposes no extra New Settings restrictions
(`Av1anResumeWindow.axaml.cs:66–79`). Both source resume buttons eventually set
`-r` (`Av1an.cs:37–63,722`). The pinned engine reads existing scenes and queue
verbatim (`context.rs:897–913,1360–1374`), including each saved chunk's encoder
parameters and source command. Global new FFmpeg filters are applied separately
when unfinished chunks execute (`context.rs:630`). New cap/quality argv does not
repartition old work or replace those saved encoder parameters.

`source-engine-new-probes.log` is1/1 green with three real subcases. The harness
uses an invocation delay around the actual x264 binary to stop after one of five
chunks; it changes neither encoder arguments nor encoded pixels. Detailed
source-equivalent argv, saved scenes/queue, completed bytes and final complete
frame metadata are retained in `source-engine-{case}.json` and corresponding
`-first.log`/`-resumed.log` files:

- `new-filters`: cap30→60 and CRF0→39 argv retain the original queue/CRF0. New
  hflip/48×32 scaling applies only to the96 unfinished frames; the24 preserved
  frames remain96×64. Exact region pixels, all120 finite30fps timestamps and
  complete final decode pass. Saved completed bytes and modification time are
  unchanged.
- `trim-same-frames`: replacing the existing copied-input slot with a new
  120-frame cut preserves24 old frames and reads the96 unfinished ordinals from
  the new cut. The source final output is a valid hybrid of those regions.
- `trim-shorter`: replacing that same slot with a60-frame cut retains original
  queue boundaries and preserved chunk bytes; the engine fails with
  `FRAME MISMATCH` and produces no final output.

`sourceWorkflow=true` now preserves that engine contract. An immutable internal
queue recipe seals the original source command, encoder parameters, source
ordinal boundaries, typed rate/cap, count policy and tool/resource hashes.
Newly requested settings/argv are stored separately: the saved queue continues
to own old encoder commands, while current global filters and selected concat
arguments apply as the pinned engine specifies. New Settings does not delete
verified completed chunks or repartition their original scene queue.

Ordinary exact-count encodes and Saved resumes with an unchanged source
recipe use the frozen chunk count and a joined-video header check. The muxed
output is checked for its planned streams, video packet count, codec and
dimensions. Declared video duration is checked for a known cadence starting
at zero.
Saved chunks still have identity, size and SHA-256 receipts.

New Settings obtains decoded property/count receipts for retained chunks when
the user selects that action. Mixed-source or resampled queues and the Dolby
Vision base-layer clock still need decoded region or timing evidence. Those
jobs and jobs with Thorough verification enabled independently reproduce the
join with the selected concat method. Packet/header/order/timestamp equality
and a strictly increasing decoded clock establish the actual per-region
geometry and frame count. Older workspaces with decoded contracts and no
mixed-chunk provenance use this detailed join check conservatively. Thorough
verification also checks complete final decode and exact copied video packets.

For source copied cuts, a stable owned decoder slot preserves the original
queue's source path while immutable cut variants preserve rollback authority.
New trim preparation verifies the original contents and exact requested recipe
before changing that slot. A journal restores the last committed cut, settings
and tool stamps after canceled review, interrupted review or a canceled partial
copy, retaining only completed receipts whose actual files remain. A shorter
replacement retains original boundaries and fails with the source's EOF or
`FRAME MISMATCH` result rather than inventing a new partition. Changed slot
identity, queue bytes, completed payloads and foreign workspace files fail
closed. An initial queue made without a cut keeps its original source command.

Saved resume from an already Finalizing checkpoint reuses the sealed joined
video. Mixed or otherwise detailed queues reconstruct their sealed regions and
independent concat clock before the final mux. The regression reproduced the
former fixed-geometry rejection before
the repair. `source-finalizing-green.log` passes1/1 in62.78s and
`native-source-finalizing-saved.json` proves120 mixed-size frames and region
pixels, identical stage/final packets, unchanged chunk invocation count,
original preservation and no-clobber behavior.

The older copied-cut New/cancel/restart receipts and ordinary explicit API
requests use `sourceWorkflow=false`. They continue to qualify the historical
rebuild policy, which deletes old encoded work and derives a new queue. They
are kept separate from the source-profile receipts. See
[recovery actions](source-av1an-recovery-actions.md).

The final native source-profile run, `native-source-new-final.log`, passes2/2
in225.07s on the final rebuilt binary with the default thread stack:

- New cap/quality argv retain the saved queue and encoder parameters; new raw
  hflip/scale rows affect only unfinished regions. Foreign files, missing frozen
  recipe authority, changed queue bytes and changed completed payloads reject
  reuse without changing preserved work.
- New copied cuts retain actual saved source ordinals. A shorter cut fails at
  the engine's next ordinal with no final output; changed slot identity rejects.
  Canceled review restores committed bytes/settings. A real child process is
  killed after preparing held New Settings, then startup restores the committed
  recipe and prior cut. A subsequent equal-length New completes with the exact
  retained prefix and new-cut suffix pixels, unchanged completed file bytes and
  modification times, original preservation and workspace cleanup.

Detailed final probes are `native-source-new.json` and
`native-source-new-cut.json`. `legacy-new-unit.log` and
`legacy-new-native.log` each pass1/1 and keep the explicit-API rebuild/cancel
contract distinct. The current-source browser route also passes5/5 in
`source-workflow-browser-final.log`, including frozen held/batch requests and
source-selected concat/tool requirements.

## Typed Frame Rate and count policy

Source typed resampling adds `--ignore-frame-mismatch`
(`Av1an.cs:542–565`), using the source relative rate tolerance and the typed
`fps=fps=…` graph (`Av1anUi.cs:448`). Original source ordinals still own scenes
and queued chunk boundaries; Jesses does not globally prerender the new rate
and split that different frame sequence. A chunk receipt separately records
its original source span and its actual decoded output count. The original
serialized per-chunk ignore flag remains frozen across New Settings. Current
global resampling/concat flags cannot grant an old exact-count queue permission
to accept a mismatching chunk.

`source-typed-fps.log` passes1/1 in21.62s. The literal pinned source engine and
native job both read96 source frames at24fps, preserve four24-frame chunks
under the typed30fps cap30, and produce120 decoded frames. Full decoded pixels
and packet PTS match the independent engine output; argv, queue, scenes and
probes are in `source-typed-fps.json`. `source-count-policy.log` passes1/1:
old false/current true remains exact-count, and old true/current false retains
the saved resampling permission. `source-raw-fps-failure.log` passes1/1 in3.28s:
an untyped raw `fps=12` graph retains the actual engine `FRAME MISMATCH` failure
and publishes no output.

`source-partial-cancel.log` passes1/1 in4.38s, canceling an actual16MiB slot copy
after a partial write, restoring exact committed bytes and immediately reopening
Saved Settings in the same process. `atomic-write-failure.log` separately
injects a real kernel partial-write/EFBIG failure: the committed receipt remains
unchanged and only that write's exact uncommitted temporary file is removed.

The selected-concat proof has seven positive routes plus mutation and
pause/cancel checks (`target/test-artifacts/native-backlog/source-queue-concat-real.log`,
3/3). A literal unforced mkvmerge82 mixed-rate IVF result with a duplicate
boundary timestamp remains a measured invalid-result negative; it is not a
general mixed-rate restriction. Rotated copied cuts are qualified separately in
[the copied-cut note](source-av1an-copied-cut.md#rotated-copied-cuts).

One measured source bug correction remains deliberate: the source x265+MP4
selection forces FFmpeg concat, but the pinned engine rejects raw HEVC with the
explicit diagnostic that mkvmerge is required (`source-x265-mp4-rejection.log`).
Jesses retains its working MKV join followed by final MP4 publication for that
combination. Valid source-selected FFmpeg routes for x264 and VPX are honored;
the x265 correction is not described as literal engine equivalence.
