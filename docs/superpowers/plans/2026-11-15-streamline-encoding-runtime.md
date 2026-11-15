# Streamline encoding runtime implementation plan

Use the repository's AGENTS.md and execute this runtime workstream directly. Rust commands use two build jobs.

**Goal**

Reduce the wait before AV1AN encoding and after Quick Convert and AV1AN encoding by removing redundant file reads.

**Design**

[Runtime design](../specs/2026-11-15-streamline-encoding-runtime-design.md)

**Implementation and verification**

- [x] Extend the native AV1AN regression tests to cover a multi-second AAC conversion and a source with a late audio gap. Confirm a prefix inspection and successful publication in default mode and an audio timeline error before publication in thorough mode.
- [x] Choose the primary AAC prefix scan using `thorough`. Move synthetic capability checks and independent canonical source verification behind thorough mode. Retain bounded color checks, tool and artifact validity checks, and the byte fingerprints required to admit recovery.
- [x] Reproduce the 240 fps timestamp grid on a generated recording and on the user's Desktop source. Choose the average rate before the nominal rate in `jobs/source_normalization.rs`. Add guarded compressed retiming for nearly constant-rate AV1 recordings and regressions for the clock and single-video eligibility.
- [x] Run the native VFR encode and record time to encoder startup, source frame rate, and published frame count. Exercise shifted, rotated, and variable-clock sources and the thorough path.
- [x] Run the final native suite, formatting and lint checks. Confirm first-sample timing, track order, subtitle and chapter compatibility, and publication after header validation.
