# Source preview and automatic crop validation

Native Windows interaction was additionally exercised on 2026-09-13 with the
interim portable development build. A two-video source showed the correct red
and blue original streams, updated after seeking, proposed exact known borders,
and changed framing only after Apply. The full real 720p episode also displayed
the expected source frame and produced a zero-crop proposal after sampling;
the picture reaches its frame edges. Source SHA256, length and modification time
were unchanged. Screenshots and integrity receipts are retained locally under
`Videos/Jesses-migration-native-20260913`. This does not establish packaged
clean-machine or Linux desktop behavior.

The 2026-09-13 development slice provides a source frame preview and a reviewable
automatic crop proposal alongside framing controls. Source images are generated
by the media backend, before encode crop, resize or borders. A crop outline uses
source coordinates. Applying a detected crop changes the ordinary per-file draft;
it never changes a submitted job or skips batch preview invalidation.

## Behavior and bounds

- Opening the disclosure starts the first preview. Closed batch rows generate no
  images. Seek changes, source/video changes, closure and disposal cancel owned
  requests and discard stale completions. Cancellation is registered before the
  media command starts, including cancellation during registration.
- Preview images fit within 960 by 540 pixels, do not upscale, and are returned
  as bounded PNG data. FFprobe output and each FFmpeg capture are limited to
  2 MiB. Source inspection has a 15-second limit; a preview has 30 seconds.
  There are at most two active backend analyses.
- Automatic crop examines up to six decoded frames at each of ten positions,
  with a 12-second limit per position. It proposes the common rectangle when
  more than 80% of usable detections agree. Otherwise, the proposal encloses
  all detected content, preserving differently positioned pictures. Invalid,
  all-black and unusably small rectangles do not become crop proposals.
- Detected edges align outward to the even-pixel grid. Applying a proposal
  remains explicit; sampling cannot establish that every scene has the same
  borders. The user can inspect several positions before applying it.
- The source stays under the existing read-only file guard throughout each
  operation. Metadata and a bounded sampled-content fingerprint are checked
  again before returning. The fingerprint identifies this inspection; it is
  not a complete-file content hash or an encode validation substitute.
- The supported source geometry is unrotated and square pixel. HDR with known
  BT.2020/PQ or HLG metadata is converted to SDR for the display image only,
  using FFmpeg's CPU zscale/Hable path. Saved HDR encode options are unchanged.
  Required filters must be present; an unsuccessful media command is reported.

The filters follow the [FFmpeg filter documentation](https://ffmpeg.org/ffmpeg-filters.html).

## Automated qualification

Local Windows x64 qualification used FFmpeg/FFprobe 9.0.1 and the pinned Rust
1.98.1 toolchain. Three opt-in actual-tool tests passed:

1. A synthetic file with two differently colored/bordered videos verifies the
   selected source stream, independently decoded PNG pixels, exact detected
   edges, invalid video selection, Unicode/apostrophe/dollar paths, and unchanged
   source bytes and modification time.
2. A tagged 10-bit PQ source verifies SDR display conversion, black borders and
   a visible picture. A separate all-black source produces no crop proposal.
3. Canceling active analysis returns promptly and releases source handles; the
   fixture can be renamed immediately after cancellation.

Four ordinary tests cover conservative proposals for changing picture positions,
invalid detector output, bounded preview dimensions and cancellation before a
source is opened. Six browser tests cover lazy startup and explicit application,
stale image completion, late registration cancellation, changed-source rejection
and stale crop detection after selecting another video stream. The Batch regression
also verifies that detection alone preserves a ready preview, explicit application
invalidates it, and queued requests retain their earlier framing after subsequent
proposal application and submission.

```sh
cargo test -p media-runtime --test analysis_jobs --locked -- --ignored --test-threads=1
pnpm exec playwright test tests/frontend/analysis.spec.ts
```

Svelte checking, Rust formatting and runtime Clippy with warnings denied passed.
The Linux CI lane at this checkpoint ran the same actual-tool gate with its pinned
FFmpeg build; libzimg was included in that build and its dependency fingerprint.
Remote CI results were a separate qualification gate. Linux native qualification
is now deferred. The additional native Windows evidence is recorded at the
beginning of this document.

Frame previews are for inspecting framing, not color-critical mastering or
frame-accurate trim selection. Rotation/SAR handling, full rendered-filter
previews and trim remain outside this slice.
