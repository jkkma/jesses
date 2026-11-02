# Source batch output naming

The authority is Nmkoder `UI/BatchNaming.cs`, `UI/UiData.cs`,
`IO/IoUtils.cs:892–939`, and `UI/Tasks/QuickConvertUi.cs:700–779` at
`a7e9731`. Batch mode names each original source beside that source unless the
configured default output folder exists. Editing an encode tab's output box
does not change this batch destination policy.

`resolve_batch_output_paths` / `resolveBatchOutputPaths` is a read-only native
API shared by GIF/PNG/JPEG exports and ordinary encode batch naming. It receives
frozen input paths, the first source-video dimensions, the target format, the
template, a local calendar-date snapshot, and the source UI's displayed codec,
quality and preset strings. Native source guards recover the original identity
of a managed image folder; its dotted name is retained and its extension token
is empty. Source template expansion and collision handling never use the private
lossless cache filename.

Known placeholders are case insensitive, in source replacement order: `name`,
`ext`, `index`, `codec`, `crf`, `quality`, `preset`, `width`, `height`, `date`.
`crf` and `quality` are aliases for the displayed quality value, including
bitrate/filesize modes. Quick Convert supplies its integer-truncated display
value; AV1AN retains fractional target quality. Queue indices are padded to the
digit count of the complete frozen batch. Unknown or unfinished tokens remain
literal. Blank templates use `{name}`. Separators and the source's explicitly
invalid filename characters become underscores; surrounding whitespace and
trailing dots/spaces are removed. An empty result falls back to the source name.
Missing video dimensions expand to empty strings.

The returned items each contain an input path and either an output resolution
or an error. A failed source does not discard the other results or change their
original queue positions. The default folder is checked separately from the
template; missing/unusable configured locations fall back beside each source.
No output folder or file is created during resolution. Existing files,
directories, broken links and earlier destinations in the same proposal are
reserved with ` (1)`, ` (2)` numbering. Canonical parent directories and
platform path comparison keep aliases from reserving one destination twice.
Publication and queue admission still reject destinations occupied after the
read-only proposal; they never overwrite the later owner's bytes.

`BatchEncodeRequest.sourceNaming` opts an encode preview into this same source
renderer/resolver after actual source inspection. Its `outputDirectory` is the
configured default (possibly empty), and its dimensions come from the first
video in each inspected source, even when another video is selected for output.
Legacy API callers omitting `sourceNaming` retain their previous request
interpretation. This compatibility path adds no controls to the source UI.

Validation on Linux, 2026-11-01: the renderer unit test and three public API
tests pass. They cover all source placeholders, source replacement order,
unknown/literal tokens and sanitization, missing metadata, independent source
folders, a quoted configured directory, file/directory/broken-link collisions,
in-batch duplicates, container/GIF/PNG/JPEG naming, partial failures, real
FFmpeg/FFprobe two-video source inspection, original dotted image-folder
identity, and a proposal-to-queue race preserving the later destination.
Native desktop compilation, generated contracts and IPC registration checks
pass. Evidence is retained in `target/test-artifacts/source-batch-naming/`.
Windows/macOS runtime qualification remains separate from these Linux checks.

The source-compatible resolver and sourceNaming preview no longer impose the
legacy 100-file API cap. A real 101-item fixture probes the source and reserves
all 101 unique destinations; legacy non-source batch callers keep their earlier
contract. This does not create files during proposal generation.

`BatchEncodeInput.videoMode` optionally freezes each Quick file's mapped video
presence before native inspection. No-video items use drop and discard inactive
picture preparation, while selected audio/subtitles and trim remain. AV1AN keeps
its one-video-per-file rule. The actual mixed-batch fixture encodes video with
x264 and publishes audio-only and subtitle-only outputs without any unchecked
video or stale framing/pixel/filter work. Source batches rebuild their checked
tracks for each file, rather than reusing the prior mux selection.

Source preview and naming accept AbortSignal through the existing AnalysisTasks
ticket lifecycle. Cancel before registration prevents invocation, active preview
cancellation reaches the current FFprobe process and waits for its cleanup,
and the between-file/final checks reject the entire proposal. No jobs are queued
by either API. A late result is discarded after Stop. Native cancellation is
scoped to this preview and does not stop unrelated jobs. Legacy calls without a
signal preserve their direct API behavior. The isolated real slow-probe test
checks that its process is dead, no output/job was admitted, and a retry works.
Frontend contract tests separately cover delayed registration and late replies
for both encode preview and image naming.

Current focused evidence is
`target/test-artifacts/source-command-edit/source-batch-final.log` (7 tests,
including the isolated child), plus the source browser regression logs.
