# Streamline Encoding UI Implementation Plan

> **Execution policy:** Apply AGENTS.md first. Use superpowers:executing-plans directly for one bounded, coherent task. Use superpowers:subagent-driven-development only when substantial separable workstreams warrant workers. Review and verification are steps, not automatic worker assignments. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make waits visible, surface common blockers, and apply the requested encoding defaults and Tune control.

**Architecture:** The shared encoder exposes focused blocker messages and pre-job stages to the shell. A visible HDR Tune selector reflects advanced overrides, and workflow-specific startup defaults flow into existing requests.

**Tech Stack:** Svelte 5, TypeScript, Playwright, pnpm.

**Spec:** `docs/superpowers/specs/2026-11-15-streamline-encoding-ui-design.md`

## Global Constraints

Preserve source-change reset preferences, explicit in-session choices, advanced controls, collision handling, one-click Run, Shift review, and batch/image behavior. Do not clear saved preferences, commit, or publish. Do not edit EncodeWindow or its log tests while another worker owns them.

## Review Focus

- A saved AV1AN infrastructure preference remains intact when the new default loads.
- An SDR source retains the requested 10-bit selection.
- Switching sources retains explicit quality and tune changes under existing reset preferences.
- Advanced HDR tune overrides and the Video selector agree, including non-0/5 values.
- Stopping preparation prevents a later job launch, and native progress takes over.

### Task 1: Focused blockers and preparation

**Files:** `src/lib/components/shared/SingleEncode.svelte`, `src/lib/source-workspace.ts`, `src/App.svelte`; tests `tests/frontend/encode.spec.ts`, `tests/frontend/command-review.spec.ts`.

- [ ] Test invalid quality, unavailable required encoder and empty output in applicable modes, including correction and Video navigation.
- [ ] Confirm the tests fail on the old unexplained states; implement focused messages without rewriting all eligibility logic; re-run.
- [ ] Test held pre-job resolution, Stop and no launch in both workflows; test progress takeover after a job is published.
- [ ] Confirm the tests fail on the generic running label; implement preparation labels and preserve normal and Shift review; re-run.

### Task 2: Requested defaults and visible Tune

**Files:** `src/lib/components/shared/SingleEncode.svelte`, `src/lib/components/shared/encoder-options.ts`; test `tests/frontend/encoding-defaults.spec.ts` and affected regressions.

- [ ] Test both imported-source workflows against the requested codec, container, quality, preset, color, frame rate, framing, grain and VQ values, plus emitted request settings.
- [ ] Test the Video Tune selector against Advanced overrides and switching to an incompatible encoder, saved infrastructure preferences, and source switching.
- [ ] Confirm tests fail against old defaults; implement the scoped defaults and synchronized Tune selector; re-run.
- [ ] Inspect both defaults at a narrow desktop viewport using isolated test fixtures and screenshots.

### Task 3: Final verification

- [ ] Use the existing first-track menu rule for initial video, audio and subtitle selection, preserving manual choices on loaded rows; test the UI and submitted stream indices for both workflows.
- [x] Preserve AV1AN's Scene Detection default and saved split preferences without migration, as requested before release. Keep fixed chunks available explicitly and distinguish its benchmark from the default.
- [ ] Run `pnpm check`, `pnpm build`, and the project frontend suite. Inspect and report all failures by name.
- [ ] Review the diff against the spec and mode-specific guards. Report evidence and limitations without committing.
