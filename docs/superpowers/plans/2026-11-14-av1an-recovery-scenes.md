# AV1AN Recovery and Scene Prepass Implementation Plan

> **Execution policy:** Apply AGENTS.md first. Execute this assigned workstream directly and verify it before reporting to the primary.

**Goal** — Resume or discard a crashed launch, clean up completed and cancelled prepasses promptly, and avoid repeated reader-cache hashing.

**Architecture** — Recovery verifies the entire tree while holding ownership and sweeps only recognized launch contents. Scene detection joins every slice before checked scratch removal. An open reader-cache guard verifies metadata on periodic checkpoints and a full digest verifies the required checkpoint and resume.

**Tech stack** — Rust, Tokio, pinned av1an and VapourSynth L-SMASH.

**Spec** — AV1AN encoding review from 2026-11-14, findings 1–3 and 8–9.

## Recovery

- [ ] Reproduce resume and discard rejection using a recorded launch with scene scratch and staged video. Keep tests for unrelated entries, filesystem links, and replaced identities.
- [ ] Extend `verify_action_tree` and checked removal in `recovery.rs` to accept only recognized launch contents. Sweep stale launch reservations before launching av1an, with checks for existing concat control files that name the stale output.
- [ ] Verify both actions preserve the original source, neighboring files, and admitted completed chunks on resume.

## Scene detection

- [ ] Exercise two real L-SMASH slices and assert scratch has been removed before the main encode starts. Exercise a failed slice while a sibling is running.
- [ ] In `scenes.rs`, prepare slice commands before spawning work and use a shared cancellation signal to drain all slice tasks. Remove scratch explicitly after both success and failure, and stop the encoding attempt on checked cleanup failure.
- [ ] Verify the pinned av1an fresh startup reuses the prebuilt index and scene list through `--resume`, which it clears internally when both recovery markers are absent. Unknown revisions use av1an's own scene detection. Exercise the behavior against the release binary, including a hard-crash resume.

## Reader cache

- [ ] Count SHA-256 invocations for first seal, repeated periodic checks, required verification and saved receipt verification.
- [ ] Use the existing open cache guard to verify periodic metadata, retaining the full digest at the required boundaries. Coordinate the periodic timer change in `av1an.rs`.
- [ ] Run the focused recovery and scene tests, the packaged Windows tests where available, and formatting checks.
