# Windows external-reader recovery qualification

This record covers the installed Windows x64 app with explicitly configured
external FFMS2 and BestSource readers. The optional readers are not bundled by
this milestone. Sources, destinations, history, reader runtime, and recovery
workspaces are kept separate throughout the qualification.

## Recovery contract

External-reader checkpoints include the reader index's exact owned path, file
identity, length, and SHA-256. FFMS2 uses
`chunks/split/cache.ffindex`. The qualified BestSource runtime appends a track
number and extension to its configured base, producing
`chunks/split/cache.bsindex.0.bsindex` for the first video track.

Resume verifies and guards the saved index alongside the source, settings,
tools, queue, script, and completed chunks. Missing, changed, redirected,
unexpected, or multiple reader indexes are rejected. An external-reader
checkpoint from an older app that lacks the index fingerprint cannot safely
mix its saved chunks with a newly generated index, so it is rejected. This
restriction does not change legacy L-SMASH checkpoint compatibility.

The job log records the actual reader index path. Stop, cancellation, and an
interrupted process preserve recoverable work in the reported workspace.
Reopening history does not start a job. Explicit Resume retains the original
request and reuses verified completed chunks. Successful verified publication
removes the workspace and its reader index; a cleanup failure reports the
retained path.

## Qualification evidence

On 2026-09-20, both readers completed the lifecycle below in the native Windows
x64 app installed through an isolated Scoop update. App controls were exercised
through Windows computer use. Process, file, cache, and media checks were
independent of the UI. Forced exit terminated only the verified app process;
child processes were not manually terminated.

The candidate was built from base `e43102c` plus this change. Its installed
executable SHA-256 was
`7f8ba263e3bba3f1f0542cfc5f00ed7c8b0861689f6d4ec2e7a9e7898be59a74`.
Build-input hashes, package verification, and the Scoop receipt bind the native
results to that candidate. The update preserved existing job history.

The read-only fixture contains 1,079 progressive 1280 × 720, 10-bit frames at
24000/1001 fps, AAC audio, ASS subtitles, and 24 font attachments. Both jobs used
SVT-AV1-HDR, preset 1, CRF 30, one worker, and film grain 0. FFMS2 used fixed
120-frame chunks; BestSource used fixed 60-frame chunks.

| Native lifecycle check                                                   | FFMS2                 | BestSource             |
| ------------------------------------------------------------------------ | --------------------- | ---------------------- |
| Pause: stable saved progress and worker CPU over 8 seconds               | 480 frames / 4 chunks | 600 frames / 10 chunks |
| Continue, then stop with saved progress                                  | 600 frames / 5 chunks | 780 frames / 13 chunks |
| Close and reopen stopped job, remaining idle                             | Passed                | Passed                 |
| Explicit resume, then cancel with saved progress                         | 960 frames / 8 chunks | 840 frames / 14 chunks |
| Close and reopen canceled job, remaining idle                            | Passed                | Passed                 |
| Explicit resume, then forced app exit                                    | 960 frames / 8 chunks | 900 frames / 15 chunks |
| Reopen as interrupted, waiting for explicit resume                       | Passed                | Passed                 |
| Resume with saved chunks unchanged, then verified output                 | Passed                | Passed                 |
| Worker cleanup after stop/cancel; all descendants gone after forced exit | Passed                | Passed                 |
| Completed workspace/index cleanup and final app-close cleanup            | Passed                | Passed                 |

Installed FFmpeg/ffprobe 9.0.1 independently verified both outputs: complete AV1
decode, 1,079/1,079 frames, zero video timestamp difference, unchanged stream
order, copied audio/subtitle payloads, and all attachments. The 1,938 AAC packets
and 15 ASS packets also had zero timestamp difference. Source SHA-256, size, and
modification time remained unchanged.

Cross-snapshot checks covered 12 FFMS2 and 14 BestSource transitions. Every
previously saved chunk retained its hash, size, and modification time. Each
reader index retained its exact path and fingerprint, and that path appeared
in the job log. Source-folder, reader-runtime, and four configured user-cache
directory inventories matched the pre-job baseline, with no additional reader
caches. Both final closed-app snapshots contained no surviving job processes.

| Artifact          | Bytes      | SHA-256                                                            |
| ----------------- | ---------- | ------------------------------------------------------------------ |
| Source fixture    | 15,195,525 | `25df21c52678e7c4c785674809793601a2eeb5363cce76f7508416bc8efc71de` |
| FFMS2 output      | 15,332,291 | `eefb999bba027af5905df06384cb2bbeb27e196750e54f405fa2738674d31aa2` |
| BestSource output | 15,710,801 | `52b28434924da496f500490249878eb15f537ef91879228cf3e2b2d442f0f309` |

Local evidence is retained under
`target/windows-external-readers-e43102c-20260920/`: native UI captures,
identity-pinned process snapshots, pause measurements, live chunk hash checks,
cache inventories, package receipts, and independent media reports. Evidence
matrices distinguish required checks from retained observer diagnostics.
`validation/qualification-evidence-matrix.md` records 39 required checks passed;
`validation/strict-lifecycle-facts.json` records the cross-snapshot comparisons.

The first FFMS2 resume observer incorrectly treated the initial stopped state
as its terminal result. The corrected observer waits until it sees running.
Separate snapshots establish that first resume's progress and cancellation.
The two later FFMS2 observers verified unchanged live chunks and process
cleanup, but their stricter aggregate result remained false because the
periodic saved-frame counter did not advance before exit or final publication.
The BestSource forced-exit observer had the same counter-observation limit;
its final resume observer passed all checks, including counter advancement.
Those raw results remain intact; final completion is established by terminal
state and independent decoded-output verification. Initial mis-targeted UI
observations and the refused attempt to force-exit during preparation are also
retained as diagnostics, not lifecycle passes.

AV1AN emitted an empty internal-audio warning with its own audio handling
disabled. Jesses subsequently muxed the selected source tracks; the independent
payload and packet checks above passed.

Validation also passed workspace Rust tests, Clippy with warnings denied,
formatting, IPC contract checks, Svelte checks, frontend build, 19 focused
frontend tests, and the real FFMS2/BestSource stop/reopen/resume runtime gate.

## Scope

Qualification is limited to the current Windows host, the recorded executable
and external-runtime identities, and unchanged sources/settings/tools. It does
not qualify fresh Windows images, arbitrary reader/plugin upgrades, interruption
before the first durable reader checkpoint, OS reboot or power loss,
target-quality scoring, Linux, macOS, or a published release. External readers
were explicitly configured on this host; this is not a bundled-reader claim.
