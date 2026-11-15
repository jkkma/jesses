# Quick Convert encoding window

> **Execution policy:** One bounded task, implemented and reviewed directly under AGENTS.md.

**Goal:** Open a separate processing window when Quick Convert starts encoding, following the user's request and a reference encoder's processing window.

**Architecture:** Use a dedicated Svelte processing view and one reusable Tauri window. Reuse the existing progress estimator, job state helpers and native control commands. Keep job execution in the existing native manager. Give each window an independent job subscription. Open after command confirmation or when queued work starts; offer manual reopening. Closing the monitor only closes the monitor; closing the main app still shuts jobs down safely.

**Tech stack:** Existing Svelte 5, TypeScript and Tauri 2/Rust; no added dependencies.

**Scope:** Live progress, encoder statistics, log, existing pause/cancel/recovery controls, and final results. Use Jesses styling. AV1AN and image operations do not automatically open this window. Keep the window open on completion. Do not publish a release or replace the installed app.

**Approach:** A native app window can reuse authoritative job data and controls. An external console would require process changes and would not provide the existing controls; a modal would not provide the separate window requested.

**User refinements:** Make the window responsive and follow the reference processing layout while keeping Jesses' colors. The final view gives most space to a scrollable monospace log, with compact progress and controls below. It uses the existing paper-and-ink color variables and resizes down to 360 logical pixels. Log following pauses when the user scrolls back.

**Review focus:** Concurrent subscriptions; canceled command review; queued/short jobs; window-open failure must not fail or repeat an encode; closing/reopening the monitor; main-app shutdown with the monitor still open.

- [x] Add behavioral regressions for automatic opening and the dedicated monitor, and demonstrate failure.
- [x] Implement native window lifecycle and independent subscriptions, with native regression coverage.
- [x] Connect Quick Convert starts and queued transitions; add monitor and reopen control.
- [x] Run frontend, native, registration and build checks, inspect the window, and review the final diff.

Validation: 335 frontend tests and all 29 desktop Rust tests passed. The final monitor suite passed all 11 tests, including 360x640, 600x400, 900x760 and 1440x900 layouts. Svelte checking reported zero errors/warnings. Native build, Clippy with warnings denied, formatting and four IPC registration checks passed. Vite built successfully with its advisory about the existing large application chunk.

Native verification used an isolated portable debug build, its own profile, and a generated 15-second 1280x720 source. Two real x264 encodes succeeded. Both main and processing windows received pause and completion updates. Closing/reopening the processing window preserved a paused job; resizing the native window to 360 logical pixels produced no horizontal overflow. Abort reached both windows. Closing the main window with another paused job terminated the app and persisted the job as canceled. Native import used the Recent menu; source and output artifacts are confined to the evidence directory.

Evidence: `target/test-artifacts/quick-convert-window-20261114/`, including final screenshots, test logs, output videos and isolated job history. Reviewed directly for opening races, independent subscriptions, action-response races, window permissions and shutdown behavior. No release publication or installed-app replacement was performed.

Syntax-highlighting follow-up: The processing log highlights command options, quoted values, encoder prefixes, numeric statistics, phases, warnings, errors and successful results using Jesses' existing palette. Rendering uses escaped text spans and preserves the exact log contents and scrolling behavior. Tests cover live additions, literal HTML-like output, encoder diagnostic prefixes, and command/path false positives. All 338 frontend tests pass; Svelte checking reports zero errors/warnings, the production build succeeds, and formatting and diff checks pass. Evidence includes `highlighting-tests.log` and `highlighted-log.png` in the same artifact directory.
