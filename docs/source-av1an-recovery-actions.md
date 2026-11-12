# Source AV1AN recovery actions

This contract implements the five source operations in `Nmkoder/Views/Av1anResumeWindow.axaml` and its code-behind: list unfinished encodes, Delete Encode, Delete Chunks, Resume (New Settings), and Resume (Saved Settings). Clear Temp uses the reviewed set from `Views/MainWindow.Av1an.cs:422–449`. No UI filesystem deletion is required.

`list_av1an_recoveries` / `listAv1anRecoveries()` returns verified inactive entries:

```ts
{
  job: JobSnapshot;
  encodedBytes: string;
  chunkCount: number;
  createdAt: number | null;
  lastRunAt: number | null;
}
[];
```

The source row (`Data/Ui/Av1anFolderEntry.cs:77–90`) contains input filename, encoded-file count/bytes, creation age, and last-run age. `encodedBytes` includes completed and incomplete files in the verified encode directory, matching the source Clear Temp confirmation's `ChunkFiles.Sum(f => f.Length)` (`MainWindow.Av1an.cs:433`). It does not claim to count source-reader caches. Timestamps are native manifest Unix milliseconds recorded at workspace creation and actual attempts. Older manifests without timestamps remain unknown, matching the source's `???` display. Entries sort by last run, newest first. Invalid or unreadable entries cannot hide other valid entries; direct actions return the ownership error. Active jobs are excluded.

`av1an_recovery_action` / `av1anRecoveryAction(request)` accepts these exact actions and returns `{jobs: JobSnapshot[]}`:

```ts
{ action: 'deleteEncode'; id: string }
{ action: 'deleteChunks'; id: string }
{ action: 'resumeSaved'; id: string }
{ action: 'resumeNew'; id: string; settings: EncodeSettings }
{ action: 'clearAll'; ids: string[] }
```

Clear All uses the exact IDs shown in the confirmation; encodes discovered later are not implicitly included. Every selected job and workspace is preflighted and exclusively locked before any member is removed. Originals, external input files, neighboring files, and published outputs are never deletion targets. Cleanup does not require an original that moved or changed, while either resume checks the original and external source fingerprints. Active worker tasks, foreign files/directories, symlinks/reparse points, replaced roots/intermediates, and changed verified artifacts reject the operation. Unknown workspace layouts fail closed.

Delete Chunks removes encoded files, resets completion receipts and the durable final video, and retains verified queue, scenes, source-reader index, and load script. Saved Resume preserves the immutable saved request/settings and uses existing source/tool/chunk verification.

New Settings keeps the saved source/output paths and stream selection and freezes the submitted request. Its queue policy depends on the persisted internal compatibility marker:

- Source UI requests set `av1anOptions.sourceWorkflow=true`. Existing sealed scenes, queue, per-chunk encoder/source commands and completed bytes remain authoritative. New global FFmpeg filters execute only for unfinished chunks, as in the pinned source engine. Current concat arguments are tracked separately from the frozen chunk recipe. Choosing New Settings obtains decoded contracts for the retained chunks. Complete per-region decode and an independently reproduced concat packet/clock contract verify mixed results. A copied-cut queue uses an owned stable decoder slot plus immutable variants. Review or copy interruption restores the committed cut, settings and tool authority while retaining completed work. Saved Finalizing reuses the sealed joined video, with the detailed concat proof for mixed or otherwise exceptional queues and Thorough verification.
- Historical explicit API requests with the marker false/omitted retain the rebuild policy. New Settings invalidates encoded chunks and old queue/scene/script plans and requires fresh encoder preparation. A compatible lossless prepared source can retain its verified stamp; a changed trim or incompatible recipe removes it. A pending-reset journal admits only coherent old/new settings after an interrupted reset, with no old encoded chunks available for reuse.

Both policies verify original contents, exact owned resources and retained receipts before adoption; mutable `done.json` cannot introduce arbitrary files. The source-specific behavior, frozen count/FPS authority and measured engine outcomes are detailed in [source chunk/resume authority](source-av1an-chunks.md).

Image sequences remain excluded from AV1AN. Existing cancellation, source guards, disk checks, final-media verification, and no-clobber publication remain in the ordinary job path.

## Validation

Three opt-in real-tool cases in `crates/media-runtime/tests/av1an_jobs.rs` exercise historical explicit-API stopped x264 AV1AN workspaces using FFmpeg 9, VapourSynth/L-SMASH, and the pinned AV1AN build:

- Chunk deletion retains queue/scenes/loadscript; saved resume completes all 288 frames.
- New-settings resume changes CRF 23→17 and maximum chunk size 48→24, regenerates the scene queue, produces different encoded chunk hashes, then survives a second stop/saved resume.
- Active actions, foreign files, links, and changed-source resume reject; Clear All preflights the complete reviewed set and safely removes owned caches even after the original moves.

Two additional real normalized-source cases in `migration_recovery.rs` verify both saved resume and new-settings replan while retaining the exact canonical source bytes, then verify cleanup and original preservation.

The focused journal/preparation test verifies compatible prepared-source stamp preservation, trim invalidation, both coherent history sides, and rejection of a foreign artifact introduced after review. Existing recovery unit tests cover locked ownership, changed receipts/tools/sources, and immutable prepared identity. Tests preserve source/neighbor bytes and check failed actions leave them intact.

Source-profile native gates are in `source_av1an_chunks.rs`; the Finalizing Saved regression is in `source_av1an_finalizing.rs`. Their evidence under `target/migration-validation/source-av1an-trim-20261101/` distinguishes queue retention from the older rebuild receipts. `native-source-new-final.log` passes2/2 in225.07s, including retained queue/params/bytes, new global raw filters, shorter/equal-length copied cuts, changed authority/payload/identity negatives, canceled review and hard-killed review/startup restoration. `source-finalizing-green.log` passes1/1, `source-partial-cancel.log` passes1/1, and the frozen count-policy unit passes1/1. The direct cancellation gate performs a real partial slot copy and verifies same-process Saved retry after rollback. The finalizer gate checks mixed-region pixels, complete stage/final packet identity, no-clobber and no additional chunk encoder invocations. `atomic-write-failure.log` passes1/1 with an actual kernel partial-write failure, preserving the committed record and removing only that failed write's owned temporary receipt.

Test environment additionally requires the existing `vspipe` binary directory on PATH:

```sh
source target/toolchains/qtgmc-linux-20261101/env.sh
export JESSES_FFMPEG="$PWD/target/toolchains/ffmpeg-vulkan-20261101/delivery/ffmpeg"
export JESSES_FFPROBE="$PWD/target/toolchains/ffmpeg-vulkan-20261101/delivery/ffprobe"
export JESSES_AV1AN="$PWD/target/toolchains/av1an-linux-20261101/target/release/av1an"
export PATH="$PWD/target/toolchains/ffmpeg-vulkan-20261101/delivery:$PATH"
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 \
cargo test -p media-runtime --test av1an_jobs source_recovery -- --ignored --nocapture --test-threads=1
```
