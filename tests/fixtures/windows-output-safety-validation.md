# Windows output safety validation

The 2026-09-20 Windows x64 acceptance checks use the verified portable toolset:
FFmpeg/FFprobe 9.0.1, x264 0.165.3222 b35605a, mainline SVT-AV1 4.2.0, and the
packaged av1an/VapourSynth runtime. Generated media, destinations and job history
are isolated from user files.

## Corrected behavior

- Final-container conversion attempts remain owned by the job through errors,
  cancellation and publication. Locked temporary files report their cleanup
  failure instead of disappearing from the job's diagnostics.
- Fresh standalone Matroska attempts use the same explicit cleanup reporting.
- A committed final-stage recovery file stays durable across repeated
  cancellation. Canceling a resumed finalization no longer deletes its checkpoint.
- Recovery cleanup failures appear in the job's error display. A verified,
  published output remains successful and is never deleted because cleanup failed.

## Runtime acceptance

| Case                                          | Verified behavior                                                                                                                  |
| --------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| Exact and case-insensitive source destination | `SOURCE_OUTPUT_COLLISION`; source bytes, SHA-256 and modification time unchanged.                                                  |
| Hardlink source alias or existing destination | `OUTPUT_EXISTS`; the existing file remains unchanged.                                                                              |
| Destination created after queue admission     | Execution rejects the collision without replacing its sentinel.                                                                    |
| Source locked against reading                 | `FILE_UNREADABLE`; no output is published.                                                                                         |
| Active source handle                          | Concurrent reads work; writes and deletion are denied.                                                                             |
| Locked MP4 or fresh Matroska attempt          | Verified output succeeds; `OUTPUT_CLEANUP_FAILED` identifies the retained temporary file.                                          |
| Two final-container cancel/reopen cycles      | Standalone x264 and SVT retain the verified Matroska checkpoint, then explicitly resume to the correct codec and all 2,400 frames. |

The final-publication race also passed for x264 and av1an/mainline SVT: a
destination created after execution preflight survives encode, mux and decoded
validation unchanged. Publication returns `OUTPUT_EXISTS` and retains verified
120-frame recovery. Locked recovery artifacts preserve the successful output
and workspace while displaying the cleanup error for both recovery routes.

The process checks are scoped to test-owned descendants. Shutdown/reopen leaves
no observed owned encoder tree. Successful repeated resume removes its recovery
workspace and partial files. Failed fixtures are retained for diagnosis.

Run the opt-in gates with `JESSES_TEST_TOOL_RESOURCES` set to the absolute root of
an unpacked portable package:

```powershell
cargo test -p media-runtime --test windows_output_safety -- --ignored --nocapture --test-threads=1
cargo test -p media-runtime --test windows_finalization_safety -- --ignored --nocapture --test-threads=1
```

## Native application evidence and limits

The baseline portable package, installed through the isolated Scoop lifecycle,
rejects source and existing-output destinations in Quick Convert. Native x264
cancellation leaves no destination or observed x264/FFmpeg process. Completed
x264 and SVT outputs independently preserve 1,079 frames, the 27-stream type
sequence, 24 attachments, copied audio/subtitle payloads and frame timing, while
the original source bytes and modification time remain unchanged.

These are current-host checks. They do not establish fresh-Windows-image
qualification, ordinary user PATH registration, every optional reader/plugin,
or Linux/macOS support. Cancellation during an uncommitted checkpoint copy is
outside the deterministic final-container cases; unexpected recovery entries
are preserved and reported rather than deleted by filename.
