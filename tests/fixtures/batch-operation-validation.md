# Batch operation boundaries

Batch encode applies an encoding recipe to each selected source. Concatenation,
quality comparison, bitrate reports, and CRF ladders use separate request types
and their own source selections. Loading or selecting multiple files must not
turn those operations into per-file encode jobs.

## Automated coverage

- `cargo test -p media-core --test batch_operation_contract --locked` checks the
  four operation payloads, whole-queue rejection with a valid leading encode,
  mixed encode/operation fields, and compatibility with existing encode defaults.
- `pnpm exec playwright test tests/frontend/batch.spec.ts` checks the collapsed
  operation guidance, navigation without utility or analysis dispatch, preservation
  of the reviewed selection, and queuing only selected encode sources.

An otherwise valid encode with unrelated top-level fields previously discarded
those fields. It now fails deserialization before the command can admit jobs.
The four utilities were already outside the batch encoder; this change does not
introduce a general-purpose utility queue.

## Windows x64 check, 2026-09-24

A local portable debug candidate over `ec1d3f9` passed the real Tauri/WebView2
checks with a fresh portable data directory, bundled tools, and a sanitized PATH:

- Four operations, each sent as both its own payload and mixed with valid encode
  fields, were rejected by both preview and queue commands: 16 rejections. Every
  queue request contained a valid first item to check that rejection was atomic.
  The queue and output directory remained empty after each rejection.
- The native Batch guidance started collapsed. Reviewing two sources, visiting
  Utilities and Files, and returning to Batch retained the reviewed request.
- Queuing through the UI produced exactly two successful standalone x264 jobs.
  Independent decoding and probing verified 24 H.264 frames per output at
  96 by 64 pixels and 24 fps. Source hashes stayed unchanged.
- All 920 packaged payloads remained byte-identical. New package-local files were
  confined to portable application data. Graceful close left no owned process.

The candidate executable SHA-256 is
`e16db4652e1743848b3ea34febf708ecdfe275852f97ccc3bcd268ddbd93c598`.
Local receipts are under `target/windows-batch-eligibility-20260924/`.
The workspace suite passed 315 tests with 131 optional tests ignored; the Batch
browser suite passed 66 cases. Clippy, formatting, Svelte checks, contract generation,
and the embedded-frontend Windows build passed.

This records batch eligibility on that local debug candidate. It does not qualify
the full behavior of the four utilities, release packaging, or other platforms.
