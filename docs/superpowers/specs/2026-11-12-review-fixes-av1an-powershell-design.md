# Review fixes and PowerShell 7 progress

Restore the seven behaviors found in the desktop review and expose AV1AN’s live
chunk progress in PowerShell 7 on Windows. Deliver the changes together in the
next Windows portable patch release.

## Desktop behavior

- Capture the donor, target, stream indexes and transfer options before reading
  color metadata. Each in-place transfer uses the target selected at Run.
- Exclude attached cover art from Quick Convert and AV1AN batch video inputs.
  The AV1AN batch form uses a row with encodable video for validation, and the
  preview reports audio-only rows individually.
- Preserve an explicit Default Subtitles selection of None through unrelated
  track edits, track reloads and request construction.
- The footer displays native shutdown countdown, disarm and dispatch errors.
  Its cancel control remains available until native dispatch.
- Reserve the complete File List import with the completion monitor, including
  gaps between its probes. An ordinary media probe without an import session
  also registers as active work. Stop and window close cancel owned FFprobe
  work and wait for its process cleanup. A completed or canceled import releases
  its reservation so a fresh countdown can start.
- A supported edit to an AV1AN chunk command, such as a CRF-only edit with
  MKVmerge concatenation, completes and publishes the output. Keep the verified
  launch directory until the checkpoint validates the engine's chunk options.
  Stop and Saved Resume continue to preserve eligible verified work.

## Windows AV1AN console

When the existing AV1AN console setting is enabled, launch a visible PowerShell
7 window for the AV1AN engine stage. Run the AV1AN media process with its exact
arguments and environment under existing process ownership. A pseudoconsole
provides the real terminal output to the PowerShell 7 window and feeds the app’s
log and progress path. Keep Stop, Pause, process cleanup and recovery ownership
connected to the original job. Report an unavailable PowerShell 7 executable
and continue the encode without the window.

Windows ownership and process tests must establish launch, live output, Stop
and window-close behavior. Verify a real bundled AV1AN encode in a disposable
portable profile. Existing non-Windows behavior and the Windows console setting
continue to apply.

## Release

Build and package the exact inspected commit on Windows. Reuse the verified
tool payload and corresponding sources from the published v1.2.1 portable ZIP.
The v1.2.2 ZIP, Scoop manifest, receipts, SHA-256 sums and release notes identify
the same release. Retain the Scoop Start menu shortcut from the current main
branch. Keep verification and test jobs in disposable paths. Validate the
shutdown state machine with a recording callback.
