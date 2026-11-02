# Native fixed-image Pause/Resume

Quick Convert's existing footer Pause/Resume now controls the native image
review session. The IPC method is `set_command_edit_paused {id, paused}` and
the client helper is `setCommandEditPaused(id, paused): Promise<void>`. `id` is
the stable `CommandEditReview.id`, not a transient analysis-call identifier.
There is no additional visible setting or timeout.

Each native `Session` owns a fresh image `PauseControl`. The spawned image task
explicitly enters that exact control's scope before preparation begins, and
keeps it through held review, unchanged/edited confirmation, rendering, complete
validation and publication. A task-local pause inherited only by the caller
would be insufficient: Tokio spawned tasks do not inherit task-local state.
All supervised FFmpeg/FFprobe children therefore share the session control,
including both direct QTGMC stages and descendants of an edited shell pipeline.
Native reservation/publication checkpoints also wait when paused; cancellation
can interrupt those waits. Source/caption/metadata identities are verified after
resume before publication. Assets, process memory and source guards remain
owned while paused.

The setter is idempotent. A held image can be paused before confirmation; any
subsequent child starts suspended until Resume. Completed/canceled/stale review
IDs are rejected. An ordinary queued encode review rejects this image-only API
with `COMMAND_EDIT_PAUSE_UNSUPPORTED`; after confirmation that review ID expires
and the existing queue job ID/Pause API remains authoritative. A new batch file
has a new control and review ID, so a stale request cannot pause the next file.
The UI tracks only the current image session through confirm settlement and
clears its local state on completion/cancel. Pause errors do not turn the footer
into a false paused state.

Stop, the confirm call's cancellation signal, renderer navigation and native
shutdown cancel and join the image task even when its children are suspended.
They do not require a preceding Resume. Child process trees terminate before
owned outputs/assets are released; no incomplete output is published. Linux
cleanup requires every executable worker/thread to have exited. An orphaned
one-thread zombie is already dead and owns no executable work; init owns its
reaping, matching the existing supervisor policy. Tests explicitly reject a
zombie leader that still has live worker threads.

`crates/media-runtime/tests/image_pause_jobs.rs` supplies real Linux gates:

- All nine ordinary/edited/implicit × GIF/PNG/JPEG cases pause an actual encoder
  after it has consumed CPU. `/proc` confirms stopped state, CPU ticks and owned
  output bytes remain unchanged for 200 ms, and Resume completes all 60 frames.
- Stop, confirm cancellation, navigation and shutdown while paused join the
  native task/processes without publishing or leaking owned files.
- Pause set during held review remains attached after confirmation. Completed
  batch IDs cannot affect the next real image conversion. Ordinary queue-job
  pause/resume still works through its existing job API and completes output.
- Normal and edited QTGMC pipelines pause both real VSPipe/FFmpeg stages after
  nonzero CPU work. Both stages' CPU/output progress stop; Resume completes the
  expected frames, while Stop joins/cleans both suspended stages.

This uses the rebuilt pinned FFmpeg 9.0.1 and native R79/QTGMC provider; no stub,
missing-provider skip or provider rebuild establishes those assertions. Evidence
is retained under `target/migration-validation/image-native-20261101` in
`image-pause-native.log`, with the complete image regression run in
`image-pause-final-regression.log`. Native Windows/macOS execution of this new
review-session route remains separate qualification; Linux process-state tests
do not establish those platform claims.

```sh
. target/toolchains/qtgmc-linux-20261101/env.sh
cargo test -p media-runtime --test image_pause_jobs --locked -- \
  --ignored --nocapture
```
