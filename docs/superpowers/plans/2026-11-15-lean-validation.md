# Lean encode validation implementation plan

Apply the repository's AGENTS.md. The runtime checks use two Cargo build jobs.

**Goal**

Reduce default publication checks to the media information needed to establish a usable output, and keep repeated encoder progress in the Processing footer.

**Design**

[Lean encode validation and progress](../specs/2026-11-15-lean-validation-design.md)

**Constraints**

Keep existing tool failure handling, planned stream order, source identity protection, temporary output ownership, and cancellation. Maintain the timing evidence consumed by muxing and the checks requested by thorough verification.

## Output validation

- [x] Reproduce the old hard failure when a valid 12-frame MOV is checked against a 13-frame planning hint. Add coverage for a nonempty unreadable file and a missing planned track in `crates/media-runtime/src/jobs/output_counts.rs`.
- [x] Make `output_counts::verify` return the container's optional declared frame count after checking readable headers and stream order. Call it in `JobManager::finalize_verified_inner` after container preparation. Carry expected stream types through `container::Cadence` from the encode plan.
- [x] Gate the existing alpha and lossless pixel comparisons on thorough verification. Enable that setting in native fixtures that assert the comparison itself. Inspect the default alpha plane and lossless samples independently in native fixtures.
- [x] Run `lean_probe_checks_readable_headers_and_planned_streams_without_exact_counts`, `default_encode_checks_finished_stream_headers`, and the MP4 and MOV chapter, fractional clock, exact trim, and external timecode fixtures. Record FFprobe arguments for a default and a thorough run.

## Processing log

- [x] Reproduce 301 Patman updates evicting log diagnostics from the runtime snapshot, and 150 live updates rendering as text in the Processing window. Add regressions in `jobs/encode.rs` and `tests/frontend/encode-window.spec.ts`.
- [x] Route redraws to existing progress fields in `observe_encode_with_estimate`. Preserve warning and error lines that mention encoding, Patman's total encoding time, x264's final encoded-frame summary, and FFmpeg's terminal `Lsize=` record.
- [x] Share the saved-snapshot redraw filter between Recent activity and the Processing window. Check the latest Patman frame count, speed, ETA, final summaries, and preserved diagnostics with the affected Playwright tests.

## Integration review

- [x] Inspect direct and repacked header checks, subtitle and timecode inputs, cancellation and temporary file ownership, and the source normalization and recovery handoff.
- [x] Verify the endpoint checks and Processing window with the saved [test report](../../../target/test-artifacts/streamline-encoding-timing/report.md).
- [x] Record the AV1AN native retiming rerun with desktop-like settings and the canonical recovery rerun using a positive source start. Record the Desktop-source benchmark with fixed chunks explicitly selected. Scene Detection remains the released default; the 12.2-second fixed-chunk result does not represent it. The [runtime verification report](../../../target/test-artifacts/streamline-encoding-runtime-20261115/runtime-verification.md) records both the full recording's startup and the completed excerpt.
