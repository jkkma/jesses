# Quick Convert SVT review implementation plan

Apply the repository's AGENTS.md. Keep Rust builds at two jobs during local verification and coordinate CPU-intensive checks.

**Goal**

Resolve the Quick Convert SVT review findings while keeping encoding startup and finalization lightweight.

**Constraints**

The default verification path gets two 32-byte reads from the completed IVF for encoder builds with a confirmed final count rewrite. Use the existing encoder identity and the final planned or observed input count. Keep the existing criteria for packet and decoded scans. Keep literal CRF 0 and previously accepted fractional values. Preserve existing recovery and edited-command contracts.

## Implementation and verification

- [x] Probe the Windows package's mainline SVT, 5fish and Patman builds with seekable files and pipes. Check a shorter input against a larger requested frame limit, plus Patman's final two-pass and direct-path output. Record codec identities and results.
- [x] In `crates/media-runtime/src/jobs/encode.rs`, verify the completed standalone IVF after final frame reconciliation and before checkpointing or muxing. Gate on the tested Windows identities from the existing version read. Document the seekable-output requirement. Test a matching count, a short encode and an appended header.
- [x] In `crates/media-runtime/src/jobs/encode.rs` and `encode_plan.rs`, format the plan and encoder arguments consistently for fractional CRF and signed presets. Use the selected bit depth in the running phase, report lossless and bitrate accurately, and describe the accepted SVT CRF ranges in validation errors.
- [x] In `crates/media-runtime/src/jobs/parameters.rs` and the standalone command path, reproduce the high-bit-depth mode decision and overlays combination. Allow parallel processing for fresh default-verification jobs on the exact tested Windows Patman build. Keep the existing one-lane behavior for AV1AN, thorough and recoverable standalone jobs, sample encodes, and other builds.
- [x] In `src/lib/components/shared/SingleEncode.svelte` and `encoder-options.ts`, use the selected video format for the SVT summary and explain CRF 0 and presets 10–12. Describe the conditional one-lane effect in both content preset descriptions. Verify the existing controls and request values in the frontend tests.
- [x] Run targeted native jobs, the relevant Rust and frontend suites, formatting and lint checks. Update the x264 timing fixture to check the compatible-source phase introduced in v1.2.11, alongside successful canonicalization. Record commands and receipts under `target/test-artifacts/quick-convert-svt-review-20261115`.

The Linux HDR and 5fish source pins include a seekable-file rewrite. Live probing was performed with the Windows package, which bounds the initial enforcement scope. Imported dynamic HDR uses its existing seekable path writer, with the verification applied to the copy consumed by muxing. Reused recovery checkpoints retain their existing artifact verification.
