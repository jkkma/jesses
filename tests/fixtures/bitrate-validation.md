# Packet bitrate analysis qualification

Verified locally on Windows x64 on 2026-09-13 with FFmpeg/FFprobe 9.0.1.

The media inspector scans the selected original stream index on explicit request. It plots compressed packet payload in source-timestamp-aligned windows, excludes container overhead, reports untimed bytes separately and labels decode-timestamp fallbacks. Its average uses timed bytes and a measured timestamp span; unknown final packet duration leaves the average unknown. The graph uses full window width, including the final window.

`cargo test -p media-runtime --lib bitrate::tests --locked` passes three parser tests: reordered/negative timestamps and timeline gaps, missing terminal duration, and malformed/oversized/overflowing output. The parser limits records to 8 KiB, timeline windows to 7,200 and packets to 50 million. The owned FFprobe process has a five-minute timeout and shares the two-analysis concurrency limit with preview/crop. Cancellation and application exit use the same process-tree supervision and analysis tickets as previews.

`cargo test -p media-runtime --test bitrate_jobs --locked -- --ignored --nocapture` passes the real-media gate. A four-second synthetic Matroska fixture with FFV1 video and 24-bit PCM audio is analyzed by original stream identity. A separate FFprobe JSON scan independently sums every packet into windows; totals, per-window bytes and rates match exactly for both streams. Source SHA256 and modification time remain unchanged, cancellation returns the expected error and no analysis output is written alongside the source.

`pnpm exec playwright test tests/frontend/bitrate.spec.ts` passes two browser tests: lazy analysis with selected-stream/window propagation and keyboard graph inspection, and cancellation with late-result rejection when closing the panel. Browser tests use mocked IPC; native desktop and Linux runtime qualification remain separate gates.
