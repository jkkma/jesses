# AV1AN supervisor implementation plan

> **Execution policy** Apply AGENTS.md. The assigned worker implements and verifies the supervisor and Windows console changes. The AV1AN job worker updates the caller.

**Goal** Keep AV1AN draining when its progress console stalls, return its exit result as soon as diagnostics are drained, and let active encodes continue beyond the former 24-hour limit.

**Approach** Route only VT presentation bytes through a bounded queue. Keep the viewer and writer under the existing Windows console registry through post-exit cleanup. Observe advancing frame counters and progress file changes in the AV1AN supervisor using the existing pause-aware clock.

**Scope** Findings 4–6 in `C:/Users/Deydy/findings/jesses findings/av1an-encoding-review-2026-11-14.md`. Keep the diagnostic rotation policy, visible-window cancellation and standalone tool time limits.

## Console relay and lifetime

- [ ] Add a supervisor regression that fills an unread relay pipe and checks child success and every numbered diagnostic record. Add coverage for relay closure during an active process.
- [ ] Update the Windows fixture to require a prompt result with the viewer still registered. Check a window close during encoding, a Stop after encoding, and application shutdown with a lingering viewer. Run the fixture against the original implementation to capture the timing failure.
- [ ] In `supervisor.rs`, queue complete raw reads with `try_reserve` and feed every read to the existing diagnostic path. In `supervisor/console/windows/av1an.rs`, give the registered presenter task ownership of the relay and the post-exit deadline. On cancellation or timeout, stop the viewer to release pending pipe writes and join the relay. Disable QuickEdit through `CONIN$` before reporting readiness.
- [ ] Run the focused supervisor tests and native Windows qualification script.

## Inactivity watchdog

- [ ] Add supervisor subprocess tests for advancing frame counters with blocked UI events, steady redraws with stalled counters, a quiet child updating a progress file, a silent process tree and a paused silent tree. Keep watchdog timeouts short in these tests.
- [ ] In `supervisor.rs`, implement `run_with_environment_until_inactive(spec, cancel, events, log_path, inactivity_limit, progress_paths, environment, pause)`. Restart the pause-aware timeout after AV1AN's printed encode or scan frame count increases, or when the length or modification time of either caller-provided progress path changes. Continue using the same cancellation and process-tree teardown paths.
- [ ] Run the watchdog regressions for both output and file activity, timeout and process cleanup, and pause.
