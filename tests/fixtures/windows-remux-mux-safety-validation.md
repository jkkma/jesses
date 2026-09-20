# Windows remux and multi-source mux safety

This follow-up covers both copy workflows through final-container conversion in
the installed Windows x64 portable app. It complements the standalone x264/SVT
checks in [Windows output safety](windows-output-safety-validation.md).

## Corrected behavior

- Reopening an interrupted remux or multi-source mux reports every entry at its
  former temporary pathnames, including jobs already restored by an older app.
  Those entries are preserved: ownership cannot be established again after a
  crash. Copy jobs require a new run and do not offer encoder checkpoint Resume.
- Cleanup warnings show the retained path in current status and history. Every
  failed cleanup also logs its own path, including simultaneous primary and
  final-container cleanup failures. A verified published output stays successful.
- MP4/MOV can represent a small imported AAC timestamp seam by extending a
  sample's duration to the next unchanged decode timestamp. That representation
  is accepted only with matching packet bytes, count, PTS/DTS, and bounded decoded
  samples, start/end timing, and continuity residuals. Other copied audio codecs
  retain their original continuity checks. Accumulating gaps still fail closed.

## Installed-app matrix

The acceptance harness drives the installed app's bundled frontend and real
backend through a local WebView2 debugging connection. It imports fixture paths
through recent-media controls and starts/cancels jobs through visible controls;
there are no mocked jobs. Native file-picker automation is outside this receipt.
The installed-app cases publish MP4; MOV pathname restoration has separate
runtime regression coverage.

| Case, repeated for remux and multi-source mux                        | Required result                                                                                                                            |
| -------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| Existing destination                                                 | `OUTPUT_EXISTS`; sentinel unchanged.                                                                                                       |
| Destination held with an exclusive Windows handle                    | `OUTPUT_EXISTS`; locked sentinel unchanged.                                                                                                |
| Destination created after a nonempty final-container artifact exists | Publication returns `OUTPUT_EXISTS`; the new sentinel survives unchanged.                                                                  |
| Cancel with the actual owned container converter suspended           | Canceled; no published output, temporary files, or surviving converter.                                                                    |
| Locked final-container temporary                                     | Verified output succeeds with `OUTPUT_CLEANUP_FAILED`; the retained path is visible and its bytes equal the published output.              |
| Forced parent exit with the container converter suspended            | No surviving converter or published output; reopen is Interrupted, with both retained paths reported and no automatic execution or Resume. |
| Graceful close with the converter suspended, then reopen             | Canceled state persists; no published output, temporary files, or surviving converter.                                                     |
| Fresh run after reopening                                            | Output passes independent mapped-packet, metadata, timing, and decode checks.                                                              |

Two additional real-media single-seam AAC conversions and two longer source-boundary
cases complete the 20-job matrix. The repeated combined source exceeds the AAC
continuity bound and must return `AUDIO_TIMELINE_INVALID`. The independently
prepared video/audio sources remain within that bound and produce a valid mux.
The expected result depends on each source's measured timeline.

Every source is checked against its original SHA-256, length, and modification
time. Deliberately retained crash and locked-file artifacts are inventoried with
their reported paths; they are not counted as successful cleanup. Observer
receipts identify the actual installed executable, converter command, parent,
and process exit. History observers share deletion on Windows so they cannot
obstruct the application's atomic history replacement.

## Regression gates and scope

The ordinary Windows recovery regression needs no external tools. The five
opt-in gates use `JESSES_TEST_TOOL_RESOURCES` pointing at a portable package;
the real AAC seam gate also needs `JESSES_AAC_LOOP_SEAM_INPUT`.

```powershell
cargo test -p media-runtime --test windows_remux_mux_finalization_safety
cargo test -p media-runtime --test windows_remux_mux_finalization_safety -- --ignored --nocapture --test-threads=1
```

Qualification is limited to the recorded Windows host and exact candidate
package. It does not establish fresh-image readiness, every container/track
combination, other encoder recovery routes, or Linux/macOS support.
