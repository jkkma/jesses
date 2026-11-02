# Source command review UI

The source fork opens `EditCommandWindow` when Shift is held as Quick Convert or
AV1AN reaches command execution (`UI/Tasks/QuickConvert.cs:402–413`,
`Av1an.cs:734–750`, and `Views/EditCommandWindow.axaml`). It displays editable
multiline arguments, with Run and close/empty cancellation. Batch processing
checks Shift separately as each file reaches its command, not once for the queue.

The Jesses UI now forwards the live modifier through the existing Run control and
AV1AN recovery actions. A 900-pixel source-style modal presents a monospaced
multiline editor with the fork's title, explanatory sentence, and Run button.
The UI prepares each held command, checks Shift when preparation returns, and
either shows the editor or confirms the unchanged arguments. Closing, Escape,
empty text, or Stop cancels the held session; a cleared item in a batch is
reported failed and later items continue, while Stop cancels the current item
and skips the rest. The batch captures its files, settings, output-name template
and resolved destinations once; only the Shift state is read live. Each encode
waits for its terminal event before advancing. Existing unrelated jobs are not
canceled by Stop. Progress/row state and pause/resume remain connected to the
shared job stream. GIF/PNG/JPEG and saved/new AV1AN recovery use the same UI
contract.

Browser contract tests use a **mocked** Tauri broker; they cover an edited
pipeline, Shift pressed during preparation and between files, AV1AN saved recovery,
empty/Escape/close, Stop during preparation/review/terminal wait, per-file crop
failure, pause, and a single native-channel model where the application and
terminal waiter both receive updates. The argument-editor screenshot is at
`target/migration-validation/frontend/jesses-edit-arguments.png`.

Linux native execution is now separately qualified: the command-edit real-media
suite passed 11/11, including queued and pre-ID cancellation, actual edited
pipelines, AV1AN/recovery and unchanged paths
(`target/test-artifacts/source-command-edit/command-edit-final.log`). The fixed
image suite passed in its 34/34 scoped image regression; see
[native command review](source-command-editing.md) and
[fixed-image review](source-image-command-edit.md) for exact coverage. This is
not a Windows or macOS native execution claim, nor an overall migration claim.

The existing footer Pause/Resume now targets the held image session, including
ordinary, edited, implicit and per-file batch exports. Five mocked browser tests
cover cross-tab controls, errors, session changes and Stop while paused or while
a pause request is pending. Native image process-state qualification is still
in progress and must be assessed separately.
