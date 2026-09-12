# av1an stop and resume validation

Windows x64 development qualification on 2026-09-12 covers durable av1an
checkpoints, explicit stop, and explicit resume with the job's original settings.
The recorded automated, native desktop, HDR excerpt, and complete-episode
results below are local evidence.
This record makes no CI, release, publication, or clean-machine qualification claim.

## Supported behavior

- Active av1an jobs offer **Stop and keep progress** during preparation,
  encoding, and finalization. The job stays **Stopping** while owned processes
  exit and the last complete checkpoint is recorded.
- A stopped job with saved work shows its retained frame count and **Resume**.
  Interrupted, failed, and canceled av1an jobs can also resume when they have a
  recovery record. Successful jobs, active jobs, standalone encodes, remuxes,
  and older history without recovery data do not offer Resume.
- Stopping before a recovery workspace exists reports that no progress was
  saved. A checkpoint with no completed chunks may repeat source preparation.
- Resume uses the existing job ID and immutable source, selected tracks,
  encoder, and settings. It joins the queue behind work already waiting.
  Reopening history does not automatically resume or start encoding.
- Completed chunks are reused only after their saved identities, lengths, and
  SHA-256 fingerprints match. The resumed completion record is rebuilt from
  persisted checkpoints; an unfinished chunk or partial concatenation is not
  treated as completed work.
- Once encoding has produced a verified AV1 intermediate, a finalization
  checkpoint retains that file. Resume can then repeat muxing and output
  verification without encoding the chunks again.
- The source and tools are checked again before resumed execution. Full frame,
  timing, color, HDR, track, and metadata checks still precede publication.
  Existing destinations are never overwritten. Successful cleanup removes the
  owned recovery workspace and clears its history locator.
- Ordinary **Cancel** and **Stop queue** retain their existing commands and
  queue behavior. A delayed stop request cannot reverse cancellation or app
  shutdown. Command errors remain visible without removing the saved job.

Recovery files remain beside the selected output. The saved source, tools, and
workspace must remain available and unchanged; moving or editing them is not a
supported way to alter a resumed job. Cleanup failures are reported on the
successful job without relabeling or deleting the published output.

## Pinned compatibility boundary

The executable queue schema and generated VapourSynth source script are pinned
to **av1an 0.5.2-unstable, revision 7df934d**, using L-SMASH Works, the existing
SVT plan, one pass, and chunks of at most 240 frames. The source script must
match the supported template after validated source/cache substitutions and
line-ending normalization. Unknown commands, script edits, unsupported queue
fields, changed settings, changed source content, and changed tool fingerprints
are rejected rather than executed as saved instructions.

Resume also requires the saved av1an version report to match. A different
av1an build or dependency report needs separate compatibility qualification;
this milestone does not promise cross-version recovery. Additional source
plugins, arbitrary saved commands, target-quality modes, audio conversion,
and crop/resize within av1an remain outside this implementation. The earlier
[av1an validation record](av1an-validation.md) describes the underlying encode
and HDR10 checks.

Two real-media findings have explicit regression coverage:

- av1an's internal audio extraction also selected font attachments. That could
  leave an invalid `audio.mkv` which its concatenator attempted to import.
  Internal extraction now excludes attachments, and resume removes only that
  stale owned sidecar after checking its identity and parent directory.
  Jesses still copies the selected attachments from the source during final muxing.
- The complete episode declares 24000/1001 fps, while every source timestamp
  verifies the exact decimal cadence 2997/125. The saved queue's nominal rate
  is checked against the source declaration; encoder parameters and output
  validation retain the independently verified cadence. Timestamp tolerances
  were not widened. Queues reordered by work size are validated by chunk index.

## Recorded automated checks

- **158 of 158 frontend tests passed** in 45.9 seconds, including 13 recovery
  tests. Recovery tests cover all three active stop phases, eligible history states,
  legacy history, immutable settings, reload without automatic resume,
  duplicate clicks, delayed channel/command replies, queue order, persistent
  errors, and unchanged Cancel/Stop queue requests.
- `pnpm check` completed with zero errors and zero warnings.
- `workspace-msvc-final.log` records **153 passing tests**: 8 core, 144 runtime
  library, and 1 additional integration test. There were zero failures and 37
  explicitly ignored opt-in or subprocess tests in the ordinary workspace run.
  Rust 1.98.1 MSVC formatting, workspace Clippy with warnings denied, generated
  contracts, and repository formatting checks also passed.
- `av1an-recovery-final-gates.log` records **2 of 2** opt-in recovery gates passing
  against the final code in 126.71 seconds, one with 5fish and one with SVT-AV1-HDR. Each uses a generated
  720-frame, 320 × 180, 10-bit source, stops with partially completed chunks,
  closes and reopens history, and resumes the original job.
- Those gates observe unchanged bytes and modification times for retained
  chunks during resumed encoding, reject settings/source/tool mismatches,
  admit only one concurrent resume request, and check queue-tail admission.
  They verify all 720 output frames, 24 fps, 10-bit AV1, expected SDR/HDR color,
  reordered selected streams, copied audio payloads, font attachment hashes and
  metadata, unchanged source bytes, and removal of the successful job's recovery
  workspace. Both cover a stale invalid internal audio sidecar; foreign chunk
  files are rejected before concatenation.
- Manager tests cover stop-state persistence, late phase updates, checkpoints
  without a phase transition, invalid history locators, preserved recoverable
  history at capacity, and delayed stop after Cancel/Stop queue or shutdown.
- The Windows MSVC desktop build with embedded frontend passed. Native checks
  below exercised its registered Tauri commands and retained binary fingerprints.

Run the real-tool gates with the pinned dependencies installed:

```text
cargo test -p media-runtime --test av1an_recovery -- --ignored --test-threads=1
```

## HDR excerpt and finalization recovery

A 3840 × 2160 HDR movie excerpt completed through the public `JobManager` API
with SVT-AV1-HDR, CRF 30, preset 8, film grain retention tune, and two workers.
The request explicitly enabled the existing HDR10 fallback for its dynamic-HDR
source. That qualification setting does not change the application's default
of leaving fallback off.

The job stopped with a **Finalizing** recovery record containing all 114 frames.
At that point the durable intermediate was 8,648,092 bytes and the requested
output did not exist. The reopened job logged reuse of the verified durable AV1
intermediate, subsequently succeeded, and its settled history had no recovery
locator or error.

`movie-hdr-final-independent.json` independently records all 114 decoded output frames at
3840 × 2160, all five selected tracks, copied DTS/AC-3/subtitle packet
fingerprints, and at most 1 ms of container timestamp rounding. The output was
10,886,967 bytes. The stopped snapshot, manifest, intermediate fingerprint,
final history, and independent report are retained outside the repository.
`movie-hdr-final-hdr-proof.json` additionally verifies the exact nearest AV1
fixed-point mastering-display values and unchanged MaxCLL 200 / MaxFALL 142.
The final frozen runner passed this check after the cadence compatibility fix.
No source names, source paths, or media are included in this public record.

## Native Windows stop and resume

The actual desktop app imported a read-only 1280 × 720 excerpt with 1,079
frames and all 27 tracks. HDR remained the default encoder, with CRF 30, film
grain tune, grain synthesis off, and HDR10 fallback off.

A real concatenation failure retained all 10 completed chunks. After fixing
attachment exclusion and the missing desktop command registrations, the same
failed job resumed through its UI button and published a verified output.
An independent check confirmed all 1,079 frames and 27 selected tracks.

A separate native job used preset 2. Clicking **Stop and keep progress** during
encoding retained **676 frames in four chunks** and left the destination absent.
The app was closed and reopened. The job remained stopped with the same saved
settings and retained count; it did not start automatically. Clicking **Resume**
continued the same job and succeeded. Read-only observations across 310 samples
found no size or modification-time change in any retained chunk, and successful
cleanup removed the recovery workspace.

`native-stop-independent.json` independently verifies the resulting 14,759,428-byte
Matroska output: all 1,079 decoded 10-bit AV1 frames at 1280 × 720, no more than
1 ms of timestamp rounding, unchanged copied AAC and ASS packet fingerprints,
and all 24 font attachment hashes and metadata. All 18 prior jobs retain their
requests, settings, states, and logs. One prior job's duration/progress values
differ only by one floating-point rounding unit after JSON serialization.

The three qualification sources retain their SHA-256 hashes, lengths, and
modification times. Media, native job snapshots, history, and detailed reports
remain outside the repository.

## Complete episode and process-crash recovery

A complete 34,047-frame, 1280 × 720 episode used 5fish, CRF 30, preset 8,
lineart bias 5, texture bias 4, grain synthesis off, and two workers. Its queue
contained 362 chunks. The first qualification attempt exposed the nominal-rate
distinction described above; uncheckpointed work from that attempt was encoded
again after the fix.

The final runner deliberately exited with code 86 after a durable checkpoint
containing **1,152 frames in five completed chunks**. Process inspection found
no remaining av1an, SVT, VapourSynth, FFmpeg, FFprobe, or qualification-runner
processes. Reopening history marked the same job **Interrupted**, retained its
checkpoint, and started no work automatically.

An explicit resume continued with the original request and settings. After
all 34,047 frames completed, the job stopped at finalization. The five chunks
saved before the crash had identical SHA-256 hashes, lengths, and modification
times. The complete durable IVF was 236,832,131 bytes; the requested output was
still absent. Resuming finalization reused that verified intermediate and
published the output under the same job ID with no error or recovery locator.

`episode-independent.json` verifies the **270,108,678-byte** output independently:

- All 34,047 decoded 10-bit AV1 frames are 1280 × 720, with **0.0 ms maximum
  presentation-timestamp difference** from the source.
- All 27 selected streams remain, including matching fingerprints for 61,159
  AAC packets and 353 ASS subtitle packets.
- All 24 font attachments retain their data hashes and metadata. Stream
  dispositions, color properties, and the source's empty chapter list match.
- Source hashes, sizes, and modification times remain unchanged.

The successful recovery workspace and final attempt's temporary files were
removed. The intentional crash left one zero-byte Matroska placeholder from
its interrupted attempt. It remains as crash evidence; fresh attempt names
avoid overwriting that file. This is separate from normal Stop cleanup.

## Remaining limits

This implements stop and durable restart, not operating-system suspension of
live workers. Linux native av1an recovery, physical power loss or OS reboot,
packaging and clean-machine installation remain unqualified. No complete
migration parity row is established by these Windows checks.

There may be a brief interval between a terminal snapshot and completion of
worker cleanup. A Resume request during that interval reports that the prior
attempt is still finishing; the saved job remains visible for another attempt.
Workspace relocation, cross-version recovery, automatic resume, and a
user-facing action to discard saved work remain outside this implementation.
