# AV1AN SVT-AV1-HDR PMod Review Implementation Plan

> **Execution policy:** Apply AGENTS.md first. Use superpowers:executing-plans directly for one bounded, coherent task. Use superpowers:subagent-driven-development only when substantial separable workstreams warrant workers. Review and verification are steps, not automatic worker assignments. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve all eight AV1AN/PMod review findings based on current code and packaged-tool evidence, then ship Jesses 1.3.0.

**Architecture:** Validate what the AV1AN launcher will run and make the forms reflect it. Keep explicit whole-input encodes and exact saved command bindings. Qualify changes to encoder behavior against the packaged Windows tools.

**Tech stack:** Rust media-core and media-runtime, Svelte/TypeScript, Playwright, Tauri, GitHub Releases, Scoop.

**Spec:** `C:\Users\Deydy\findings\jesses findings\av1an-svt-av1-hdr-pmod-review-2026-11-15.md`.

## Global constraints

- Use Jesses 1.2.12 (`602d307`) as the baseline and version 1.3.0 for delivery. Use the selected bundled binaries for behavior measurements.
- Keep the default verification path lean and keep the existing Thorough-only scene prepass. Preserve exact command and artifact bindings in recovery; never reuse chunks known to carry mismatched per-frame metadata.
- Cap local Rust builds at two compiler jobs. Preserve unrelated edits and use the repo owner's required author and committer identity for release commits.

## Review focus

- A custom source AV1AN request may submit fixed splitting and zero frames; the source workflow still derives a nonzero split, so reject per-frame HDR before launching.
- A complete, explicit fixed/zero AV1AN encode may still use external per-frame HDR. A one-segment grain table may have a finite, delayed or gapped range, so test the actual encoder behavior before assuming it is safe in parallel jobs.
- Quiet encoders and paused jobs must not look the same as stalled processes. Preserve bounded cleanup of silent, inactive processes.
- A pre-1.3.0 recovery recipe may contain both the generated and overriding keyframe interval and require one SVT lane. Preserve exact verified bindings and reject unrelated changes.
- An identity that merely contains the Patman marker or looks similar to the qualified build cannot authorize a change to the serial fallback.

## Implementation and verification

- [x] **1 — Dynamic HDR:** Validate true single-chunk AV1AN rather than raw fixed/zero fields alone. Both Dolby Vision and HDR10+ imports remain supported in standalone and explicit fixed/zero mode. Source AV1AN directs users to Quick Convert. The form omits an inert resize preference when the picture is unchanged and rejects actual transforms or denoising.
- [x] **2 — Grain tables:** Packaged Patman, mainline SVT, and 5fish used only the first model, even in unsplit encodes. AOM preserved timing unsplit and restarted it per chunk. Require a single zero-origin segment covering the full video for SVT and parallel AOM. Use an exact plan clock when available and the existing reconciled clock otherwise. Normalize fresh text imports to LF for AOM while retaining exact saved staging bindings. Built-in film-stock presets now generate a single full-span model; native tests verified the distinct 16mm and 16mm-3 models in AOM and Patman through all 360 frames.
- [x] **3 — Watchdog:** Measure normal output, detail-log changes, and chunk writes in both Windows progress modes. Watch for quiet chunk-file growth, keeping the existing pause-aware timeout for actual stalls. Short-threshold tests cover growth and same-size rewrites.
- [x] **4 — Workers:** Share the 64-worker maximum between backend validation and resource estimates. Cover 64 accepted, 65 rejected and agreement with frontend limits.
- [x] **5 — Batch tune consistency:** Initialize the dormant `BatchEncode.svelte` form from the encoder's declared `defaultHdrTune`. Its mount reset already yielded Visual Quality; a before/after encoder-switch regression records that behavior.
- [x] **6 — Detection slices:** Keep Thorough-only prepass (review Option A). Explain the dependency alongside the field and log when requested parallel slices are skipped because Thorough is off. Verify the lean and Thorough states.
- [x] **7 — SVT lanes:** The pinned Windows Patman build passed 152 short paired-option runs, including concurrent runs at two, four, and eight lanes. Let it honor AV1AN worker threads under default verification; retain the safety cap elsewhere and preserve exact old commands when resuming. Test lane selection and exact identity checks.
- [x] **8 — Keyframe interval:** An explicit SVT `keyint` produces one encoder flag and leaves AV1AN chunk sizing tied to the preference. Keep old receipted AV1AN commands exact and the version-one standalone recovery signature stable. Verify override, default, and saved command cases.
- [x] Run focused regressions, Rust and frontend checks, and representative real-media cases. Keep commands and receipts in `target/test-artifacts`; note material limits in the release summary.
- [ ] Bump tracked versions to 1.3.0 and build/package the release. Inspect the final diff and both Git identities, push the inspected local commit and tag, and verify the published release and assets.
- [ ] Update the Scoop manifest from the verified release artifacts, check hashes and installation, inspect both Git identities, and publish the bucket update.
- [ ] Verify publication and Scoop state, then perform the requested shutdown as the last action.
