# Source output and image export UI

The Nmkoder fork is authoritative for this phase: `UI/UiData.cs`,
`UI/BatchNaming.cs`, `UI/Tasks/QuickConvertUi.cs:700–779,1086–1110,1324–1344`
and `Views/MainWindow.{QuickConvert,Av1an}.cs` browse handlers at `a7e9731`.
This document records the current implementation and focused evidence; it is not
a claim of complete migration parity.

Quick Convert and AV1AN keep an extensionless output stem. A single run asks
`resolve_output_path` for the selected container or fixed-image target and uses
its returned path. It also asks for a suggestion on source load and checks again
on format change. Dotted names stay intact; Quick browse strips recognized media
extensions while AV1AN browse strips the final extension. Managed image folders
use their original folder name rather than the private sequence cache. A later
edit is protected from older lookup or picker replies. Run waits for a pending
default lookup, so a format change cannot accidentally discard a configured
folder. The actual writer still rejects a collision that occurs after the
read-only proposal.

Batch output ignores the single-output box, as in the fork. Encode preview uses
`sourceNaming`; GIF/PNG/JPEG use `resolve_batch_output_paths`. Both freeze the
source file list, naming values, template, target, configured default and date
at submission. They use each source's own folder when the configured default is
unusable, and the native resolver reserves numbered destinations across the
proposal. See [source batch output naming](source-batch-naming.md) for token,
reservation and native validation details. An image batch reports an individual
name failure and continues with other results.

GIF and image sequences can use the existing Burn In selector. A selected
subtitle from the video file is sent as a primary subtitle; when a different
loaded main file owns that selector it is sent as an external subtitle. Checked
and unchecked subtitle tracks both burn; the disabled selector sends none. For
batch processing, the source selector's subtitle ordinal is applied separately
to each input and missing ordinals are skipped with a log message. Encode batch
requests use the same per-file ordinal. Image outputs do not retain a separate
muxed subtitle track. Settings and source identities are captured before
asynchronous naming, crop or deinterlace work.

The source's full PNG/JPEG sequence output uses eight-digit names beginning at
`00000001`; the legacy ranged API retains its prior `frame-%06d` spelling.
Explicit image requests freeze all loaded processing paths and the source
metadata/donor/trim state. Batch requests freeze one source at a time, select
Current or None independently for metadata and chapters, and omit the editable
grid. The source image formats accept those FFmpeg metadata arguments but do
not retain title/language tags in the resulting images; native FFprobe checks
confirmed the same for these formats.

When nonvideo tracks are checked but no video track is checked, the fork leaves
video selection to FFmpeg across all loaded inputs. The `implicitExport` path
preserves that automatic selection and input order. Its intentional correction
suppresses nonvideo output streams: the fork can otherwise put subtitle bytes
into numbered image files or fail GIF output. Subtitle Strip and tested audio
modes were already valid in the fork. This is a source-bug correction rather
than exact packet behavior; it adds no setting. The bounded reproduction and
details are in [the encoding contract audit](source-encode-contract-gaps.md).

Linux browser evidence on 2026-11-01 includes 181 combined source/reconciled
tests after explicit-image metadata wiring. Tests use mocked Tauri responses for frontend behavior;
the native batch resolver's separate real-media qualification is in
`target/test-artifacts/source-batch-naming/`. Svelte check reports zero errors
and warnings, and Vite builds successfully with its existing large-chunk advisory.
The compact layout test confirms no basic-video scroll at 800 and 1360 pixels,
the numeric preset is 65 pixels, and the Deinterlace selector is 190 pixels so
its QTGMC label is readable. Screenshots are in
`target/migration-validation/frontend/`.

The later source batch UI follow-up refreshes known track selection for each
input and freezes per-file video drop for Quick audio/subtitle-only inputs. Its
27 focused mocked-browser cases include 101 files, mixed and all-nonvideo
inputs, container-specific data/attachment selection, and Stop/retry while
native preview or image naming is registering or awaiting a reply. Those
requests carry cancellation signals. Native 7/7 proof covers 101 inputs,
mixed outputs and cancellation during a real probe; see
`target/test-artifacts/source-command-edit/source-batch-final.log`.

The final Linux native image regression passed 34/34 integrations, covering
explicit and implicit command execution, image naming/metadata, historical
image/sequence cases and bitmap, QTGMC, shifted/VFR captions. Its log is
`target/migration-validation/image-native-20261101/image-final-scoped-regression.log`.
Positive NVIDIA hardware testing remains user-deferred; Windows/macOS runtime
qualification and overall migration parity remain open.
