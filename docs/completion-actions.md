# Completion notifications and finish actions

Open **When the queue finishes** below the job list. Enable notifications or
choose to close jesses or shut down the computer, then select **Apply to this queue**.
The app requires an active or queued job before arming a finish action.

These choices exist only in the current process; restarting jesses resets them.
Historical successes never arm an action. Every job in the armed queue must
succeed. A failure, cancellation, stop or missing history entry disarms it.
Additional queued work joins the queue and resets any countdown.

After successful completion, a 60-second countdown appears. **Cancel finish
action** disarms it immediately. Active media inspections and utilities defer the
countdown. No force-close flag is sent to the operating system. Windows and Linux
may decline shutdown according to their permissions and session policy; the UI
shows the error.

The cancel control stays visible until the action is dispatched. The final check
shares a lock with new job admission and inspection reservations; a cancellation
or newly admitted job invalidates a pending action. No new work is accepted once
close or shutdown has been dispatched. If the operating system rejects shutdown,
the app shows the error and accepts work again.

Notifications use the native [Tauri notification plugin](https://v2.tauri.app/plugin/notification/).
Operating-system notification settings still apply. Tests validate the state
machine and browser controls without executing shutdown or closing the user's app.

## Saved requests

Utilities also exports a versioned encode request from job history. Inspecting
such a file displays the source and encoder before queueing it with a newly
chosen destination. Execution revalidates the original source and tools.

This starts a new encode. Durable av1an and standalone phase recovery remain
attached to verified job history and their saved workspaces. A recognized historical AV1AN
sidecar can import explicit supported encoder, worker, scene, pixel-format and audio
settings. Original media is probed so selected streams retain their real indices.
For an SVT sidecar, select the original mainline, 5fish or HDR build before inspection;
the historical format does not record this choice. Source color declarations must
match the current probe. Choose a new destination before queueing. Unknown arguments,
an unspecified SVT build, or a command naming a trimmed intermediate receive a specific explanation;
the import never silently discards those settings.

Historical sidecars do not contain verifiable source/tool fingerprints. Their
completed chunks cannot be adopted as Jesses recovery receipts. Source files,
sidecars and engine workspaces remain unchanged; saved command strings are parsed
as data and never executed or imported as cleanup authority. Historical temporary
folder, log-file and resume arguments are recognized but never used by the new job.
