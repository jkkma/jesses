# Review Fixes and PowerShell 7 Progress Implementation Plan

> **Execution policy:** Apply AGENTS.md first. The frontend, runtime and desktop
> integration workstreams have separate owners. Verify the combined result
> before publishing.

**Goal:** Fix the seven review findings, show native AV1AN chunk progress in
PowerShell 7 on Windows, and publish the verified v1.2.2 Windows portable patch.

**Architecture:** The frontend freezes destructive request inputs and reflects
native completion state. The Tauri analysis registry admits file imports through
the same lock as shutdown, and the runtime owns cancellable tools and AV1AN
finalization. On Windows, the native supervisor relays a pseudoconsole’s output
to a PowerShell 7 window while retaining ownership of the AV1AN process.

**Tech stack:** Svelte, Playwright, Tauri, Tokio, Win32 Job Objects and ConPTY.

**Spec:** [Review fixes and PowerShell 7 progress](../specs/2026-11-12-review-fixes-av1an-powershell-design.md)

## Global constraints

- Integrate upstream main a542de0 before publishing.
- Preserve existing saved requests, user configuration, installed app and media.
- Keep source/tool ownership and lifecycle checks in all Windows process paths.
- Keep release verification and smoke fixtures under ignored artifacts.
- Use repository-local owner identity for both commit author and committer.

## Frontend

Owner: frontend worker. Modify SourceUtilities.svelte, SingleEncode.svelte,
TrackList.svelte and the completion footer in App.svelte.

- [ ] Add failing browser regressions for target mutation during donor
      inspection, cover art on both batch routes, audio-first AV1AN, persistent
      Default Subtitles None and native shutdown status changes.
- [ ] Freeze the color-transfer request before any await, filter non-encodable
      video from the batch request, choose a usable AV1AN representative, preserve
      explicit None and synchronize the footer with native status.
- [ ] Run the focused tests in source-shell.spec.ts, batch.spec.ts and
      track-list.spec.ts. Run the complete frontend suite and Svelte check.

## Desktop import admission

Owner: integration worker. Modify src-tauri analysis and command handlers, the
runtime probe and client.ts. The frontend worker owns the App.svelte wiring.

- [ ] Reproduce native shutdown claim with a controlled FFprobe still running.
      Add regression coverage for a reserved import that survives an idle gap,
      can reuse one ticket and cancels an in-flight probe on window close.
- [ ] Extend begin_media_analysis with optional forImport. Return a pending
      ticket while sharing completion admission. A synchronous claim_media_import
      acknowledgement makes the ticket reusable for the import lifetime. Pending
      tickets expire after 30 seconds. probe_media accepts optional
      importId and creates a one-shot registration when omitted. Pass the
      registration’s cancellation receiver to the supervised FFprobe process.
- [ ] Wire beginMediaImport, endMediaImport and probeMedia(path, importId)
      across the File List flow. Cancel the ticket on Stop and release it on
      completion, abandoned selection and failure.
- [ ] Run the native completion/analysis regressions and a browser regression
      that uses one ticket across two probes and releases it exactly once.

## Runtime

Owner: runtime worker. Modify AV1AN engine finalization and Windows supervisor
console ownership.

- [ ] Confirm a CRF-only edited AV1AN chunk command using MKVmerge
      concatenation and reproduce RECOVERY_INVALID after the engine exits.
- [ ] Keep the verified launch directory through the checkpoint that reads
      chunks/options.json. Repeat the edit and independently decode the published
      output. Exercise Stop and Saved Resume against the bundled engine.
- [ ] Exercise the Windows AV1AN console route with a real PowerShell 7 process.
      Verify streaming terminal output, Stop, window-close cleanup and headless
      fallback when PowerShell 7 is unavailable.
- [ ] Run the native workspace suite and Clippy using the shared build cache.

## Integration and release

Owner: integration worker, with primary review.

- [ ] Set v1.2.2 in package.json, workspace Cargo.toml, the app package entry
      in Cargo.lock, and src-tauri/tauri.conf.json.
- [ ] Verify the full suite, generated IPC contracts, formatting, and final
      diff. Inspect the exact Git commit, including author and committer.
- [ ] From the clean commit, run pnpm tauri build --target
      x86_64-pc-windows-msvc --no-bundle -- --locked. Record the executable hash.
- [ ] Match the prior published ZIP digest before reusing its tool tree. Verify
      the complete prior manifest, collect the new package, qualify extracted tool
      paths, generate the Scoop manifest and record artifact digests.
- [ ] Verify the packaged GUI in a fresh isolated portable profile. Exercise
      AV1AN native progress and a successful independently decoded output.
- [ ] Publish inspected local Git objects to main and v1.2.2. Upload release
      assets to a draft, compare server and local digests, then publish the release.
