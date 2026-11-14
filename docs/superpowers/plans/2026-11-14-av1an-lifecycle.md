# AV1AN encode lifecycle implementation plan

> **Execution policy** Apply AGENTS.md. The encode lifecycle worker owns these changes and coordinates the supervisor and recovery interfaces with their owners.

**Goal** Keep the AV1AN workspace resumable after mkvmerge failures or cancellation and let active long-running encodes continue.

**Approach** Verify the completed chunk receipts before copying staged output through the owned file handles. Clean up the staged mkvmerge file for every supervised outcome and retire an empty launch only after its guard is released.

**Spec** `C:/Users/Deydy/findings/jesses findings/av1an-encoding-review-2026-11-14.md`, findings 2, 6, 7 and the checkpoint interval in 8.

## Staged Matroska output

- [x] Add a Windows package regression that locks the durable intermediate during an unchanged command review, verifies copy failure and staged file removal, and resumes with the original source and a neighboring file intact.
- [x] Run `cargo test -p media-runtime --locked --test av1an_jobs mkvmerge_copy_failure_cleans_stage_and_resumes -- --ignored --nocapture` against the extracted 1.2.5 tools. The failing copy left `encoded.mkv` in its launch directory before the fix.
- [x] Add a deterministic cancellation test that signals Stop after the first copied buffer. Assert `JOB_CANCELED` and no further read of the staged bytes. The old copy ignored the cancellation and completed successfully.
- [x] Use `Recovery::open_staged_video` and the held `Source::file` in the copy task in `crates/media-runtime/src/jobs/av1an.rs`. Check cancel per 1 MiB buffer and before and after synchronization. Reset a partial destination on failure when possible. Only a successful encode can advance its receipt to finalizing.
- [x] Pair the post-run result with checked `Recovery::remove_staged_video` before releasing the launch guard. Preserve operational error information if cleanup also fails. Keep the launch directory until checkpoint has verified its `chunks/options.json` reference.
- [x] Repeat the package regression and run the cancellation unit test and representative mkvmerge success paths. The packaged default, thorough, source-filtered and frame-resampled workflows all published output.

## Progress and inactivity

- [x] Configure `MissedTickBehavior::Delay` on the one-second job checkpoint interval.
- [x] Route the AV1AN stage through `supervisor::run_with_environment_until_inactive`, passing a six-hour interval, `chunks/done.json`, and the internal AV1AN log. The supervisor worker verifies active, silent and paused children.
- [x] Propagate the scene-prepass cleanup error before the fallback path. Verify the prebuilt input immediately before spawning and release its file guards when the supervisor returns. Enable fresh `--resume` initialization for the bundled engine when the prepass has primed its index and scenes.
- [x] Confirm formatting, all-target Clippy and the Windows runtime unit suite after the supervisor and recovery interfaces settle. Check the Linux target without running it on this Windows host.
