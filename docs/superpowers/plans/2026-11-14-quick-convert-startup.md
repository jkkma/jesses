# Quick Convert startup implementation plan

> **Execution policy:** Apply AGENTS.md first. Execute this bounded task directly; review and verification are part of the same task.

**Goal:** Remove unnecessary Quick Convert preparation and strict verification from ordinary conversions.

**Architecture:** Reuse header planning and observe frames from the actual FFmpeg producer. Keep exact planning for operations that require frame boundaries and existing recovery workspaces. Run synthetic codec checks and exhaustive pixel comparisons only when thorough verification is enabled for Quick Convert.

**Tech stack:** Rust, FFmpeg/FFprobe, existing standalone encoders.

**Spec:** User request in this chat. A reference encoder's base `VideoEncoder.BeforeEncoding` and `AfterEncoding` provide the model for minimal preparation and practical output checks.

**Constraints:** Preserve source/output ownership, cancellation, process failures, stream selection, HDR rendering and frame-sensitive transforms. Preserve AV1AN behavior. Release publication is a separately authorized follow-up.

**Review focus:** Incorrect header counts must not truncate output; producer counts must replace estimates; saved recovery must use its original exact plan; explicit thorough verification must retain its checks; trims and per-frame metadata must retain exact boundaries.

- [x] Add regressions for estimated counts and real standalone encoding with stale frame metadata.
- [x] Run the regressions before implementation and confirm the relevant failures.
- [x] Update `encode.rs` startup policy, producer observation and optional verification; clarify `source_timing.rs` documentation.
- [x] Run Rust checks and real encoder regressions, including exact/thorough paths; review the diff.

Validation: 499 runtime unit tests passed (50 tool-dependent tests skipped in the normal suite). Eight selected native encoder tests then passed, including SVT/x264 with deliberately stale frame headers, AAC conversion into MKV/MP4, x265/VP9 output, source filters, two-pass encoding, chapter tracks, incompatible selections, saved-workspace resume and thorough rejection of decoded source defects. Native test output is in `target/test-artifacts/quick-convert-startup-20261114/real-encoder-tests.log`. `cargo clippy -p media-runtime --lib -j 2 -- -D warnings`, `pnpm check`, and formatting checks passed. No installed desktop package was rebuilt.

## Native Quick Convert release checks

Before a release, put the qualified FFmpeg, FFprobe, x264 and `SvtAv1EncApp` binaries on `PATH` and run the timing regressions from the repository root. The normal workspace suite skips these native cases. Verify that the selected standalone tests and the migration test ran and passed.

```sh
cargo test -p media-runtime --locked --lib jobs::encode::tests::quick_convert_uses_headers_and_counts_the_actual_encode -- --ignored --exact
cargo test -p media-runtime --locked --lib jobs::encode::tests::quick_convert_rejects_interior_timing_changes_despite_constant_rate_headers -- --ignored --exact
cargo test -p media-runtime --locked --test migration_encode canonical_sources_preserve_audio_while_normalizing_shift_rotation_and_variable_timestamps -- --ignored --exact
```

These tests cover stale frame metadata, an interior timestamp change with matching rate headers, and audio preservation across source normalization for shifted, rotated and variable-rate sources.

For an AV1AN release, also run the duration-mismatch regression with its qualified runtime. When running the portable Windows tools outside Jesses, add `tools/av1an/python` and `tools/av1an/python/Lib/site-packages/vapoursynth` to `PATH` and set `VSSCRIPT_PATH` to the absolute path to that VapourSynth directory's `vsscript.dll`.

```sh
cargo test -p media-runtime --locked --lib jobs::encode::tests::av1an_rejects_a_queue_clock_shorter_than_the_gapped_source -- --ignored --exact
```
