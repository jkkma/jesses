# Native source command review

The fork's `QuickConvert.cs:402–413` and `Av1an.cs:734–750` open
`Views/EditCommandWindow.axaml/.cs` when Shift is held immediately before that
file runs. The editor contains the actual complete command text. Empty text or
close cancels; AV1AN saves only the confirmed command. Jesses uses the same
source-facing dialog and checks live Shift after native preparation for every
file, including later files in a batch. See `source-command-review-ui.md` for
frontend evidence and `source-image-command-edit.md` for fixed-image validation.

`CommandEditRequest` covers Quick encode/copy/nonvideo conversion, fixed images,
and saved/new-settings AV1AN recovery. `prepare_command_edit` admits behind
existing queue work and returns only when the specific job reaches its held
execution point. Native source identities, output reservations, scripts,
caption/font/grain assets and recovery state stay owned while the editor is
open. The returned program includes real decoder/encoder pipelines, all
multipass commands and the final selected-track mux. It uses the held paths,
not disposable preview paths. Image preparation similarly holds QTGMC,
reference/palette and final encoding as one program.

`confirm_command_edit` consumes a review ID once. Exactly unchanged text uses
the original native argv route and its original typed validation. Changed text
runs the deliberately edited shell program with process-tree cancellation,
logs and progress. Shell execution is not sandboxed. Changed output is fully
decoded and checked as actual media; pre-edit codec/geometry/trim assumptions
are not reapplied. The original known-source/tool identities and owned-resource
checks remain, and atomic publication still cannot replace a later destination
owner. A malformed, failed, missing-output, canceled or cleared program cannot
report success or publish an unchecked output. Image edits retain the selected
GIF/PNG/JPEG output-family contract and packet-preserving sequence publication.

`cancel_command_edit`, preparation cancellation, Stop and app shutdown cancel
owned work and wait for cleanup. Cancellation reaches a queued job or a running
probe before preparation has returned a review ID. A late frontend preparation
reply is canceled explicitly before any dialog can open. Review IDs have no
editing timeout: background timer throttling and system sleep cannot expire a
source editor. Unmount/abort, main-window navigation/reload and application
shutdown trigger cleanup. Navigation cancels pending reviews and image tasks,
while previously confirmed ordinary queue jobs survive. A renderer crash
without a navigation, unmount or close event is not independently detected;
application shutdown remains the cleanup boundary in that case.

Confirmed AV1AN text is stored with the actual encoder parameters and owned
recovery identities only after confirmation. A saved resume replays that text,
adds the actual AV1AN resume flag, and rebinds exact generated owned-path argv
roles when a launch location changes. It does not replace arbitrary matching
text or user-added paths. Source/tool stamps and chunk/concat-control hashes
remain checked. Resume with new settings discards the old recipe and regenerates
it; backing out before confirmation restores the prior confirmed recipe and
settings. Clearing a fresh review removes its unused owned cache. The real
saved-resume fixture verifies completed chunk hashes and modification times do
not change during replay.

The job client has one native job channel shared among application listeners
and terminal waiters. Adding a per-file waiter cannot supersede application
progress, and disposing a waiter does not dispose another listener.

## Platform shell qualification

Unix uses Bash with `pipefail`. Windows owns batch scripts for the entire
program and each pipe stage, so the whole program is not embedded in a `/C`
argument and original stage arguments avoid the additional CMD pipe-side parse.
Individual CMD/batch command lines still have platform length limits. Native
child environment values carry Unicode literals; delayed expansion is disabled
for original text, literal percent signs are escaped in generated arguments,
and every started pipe stage records its exit status. A generated checker makes
compound operators see aggregate pipeline failure immediately: a successful
consumer cannot hide a failed producer, while an explicit `||` fallback can
handle it. Original unedited execution remains native argv.
The generator follows Microsoft's documented
[quoted argument rules](https://learn.microsoft.com/en-us/cpp/c-language/parsing-c-command-line-arguments?view=vs-2019)
and [CMD variable expansion](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/set_1).

The Windows test in `jobs/command_edit/shell/windows.rs` is runnable on a native
Windows Rust runner. It builds an argv/binary-pipe helper and checks spaces,
empty arguments, `&`, parentheses, `%PATH%`, `^`, `!`, embedded quotes, trailing
backslashes, Unicode paths/text, grouped pipelines, `&&`, redirects, multiline
programs, upstream failure and explicit `||` recovery. Linux helper tests and Windows target type
checks pass. Actual Windows shell/console/filesystem execution has **not** been
qualified on this host; the fixture is evidence to collect on the platform,
not a claim of a Windows pass. macOS native execution is also separate.

## Reproduction and evidence

The Linux real-media suite `tests/command_edit_jobs.rs` covers unchanged copy,
complete edited pipelines/compound programs with changed geometry and trim,
single/two-pass x264, late collision, upstream failure, single-use review IDs,
empty/close, queued cancellation, cancellation of a real slow FFprobe child,
subtitle-only and external-only mapping, real AV1AN unchanged/edited execution,
saved replay/new settings and cache cleanup. The image suite additionally
covers all three unchanged families, actual edited frames/palette and QTGMC
caption ownership, canceled extraction, active shutdown and mixed navigation
cleanup with an ordinary confirmed job.

```sh
. target/toolchains/qtgmc-linux-20261101/env.sh
export JESSES_AV1AN="$PWD/target/toolchains/av1an-linux-20261101/target/release/av1an"
cargo test -p media-runtime --test command_edit_jobs -- \
  --ignored --nocapture --test-threads=1
cargo test -p media-runtime --lib jobs::command_edit -- --nocapture
cargo test -p media-runtime --test image_command_edit_jobs -- \
  --ignored --nocapture --test-threads=1
cargo check -p media-runtime --tests --target x86_64-pc-windows-msvc
```

Native logs are under `target/test-artifacts/source-command-edit/` and
`target/migration-validation/image-native-20261101/`. The restored AV1AN build
uses source revision `805dad69143fa0a81cfe2fb89c0b9e90a828ea72` and the existing
exact package compatibility patch; its source archive, build inputs and build
logs are durable under `target/tool-source-cache/` and
`target/toolchains/av1an-linux-20261101/`. R79/QTGMC and FFmpeg provider recipes
and qualifications are recorded separately in `linux-qtgmc-provider.md` and
`linux-ffmpeg-package.md`. Positive NVIDIA execution remains a separately
deferred hardware qualification.
