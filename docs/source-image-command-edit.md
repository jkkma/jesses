# Shift+Run for fixed-image exports

The native `CommandEditRequest::Image` route now prepares the fork's Shift+Run
review for Quick Convert GIF, PNG sequence and JPEG sequence exports. It uses
the same source-style argument dialog and the shared single-use native review
broker. The source reference is `Nmkoder/UI/Tasks/QuickConvert.cs:402–413`.
Legacy single-PNG/JPEG exports also retain their one-frame output requirement;
image-folder import is a separate operation and is not given an extra editor.

Preparation holds the original picture/caption guards and generated subtitle,
font, palette, QTGMC script, workspace and spool objects. It does not execute
the requested conversion while the dialog is open. Probe/clock reads and the
existing small QTGMC dependency probe remain preparation. The editable program
contains the QTGMC producer/consumer when selected, decoded reference command,
GIF palette pass and final image encoder. Clearing the box, closing it, native
cancellation or manager shutdown releases the held task/assets without a false
successful result. A completion watch lets shutdown wait even when a confirm
caller already owns the result receiver.

Confirming unchanged text continues through the existing strict typed image
pipeline: complete reference validation, exact PNG decoded samples, requested
format/depth/count/geometry, source guards and no-clobber publication. This is
covered separately for all three source formats.

A changed program writes its selected encoded image format into one reserved,
identity-guarded spool. Its actual decoded frames determine the result count,
dimensions and layout. Typed pre-edit picture expectations are replaced by the
explicitly confirmed program; valid changes to trim, geometry, palette or pixel
layout are therefore accepted. The selected output family must still be GIF,
PNG or JPEG. Format signatures, complete strict decoding, nonempty/constant
frame geometry and layout, existing image dimension/DTO limits, and source
identity remain enforced. This does not promise acceptance of every possible
shell program or another container under an image extension.

For sequences, a complete streaming packet scan covers every spool byte. The
native publisher then reserves each destination frame before FFmpeg stream-
copies the compressed packets with `-c:v copy`; a second streaming packet hash
must match the original spool in order, byte count and packet count. No image
re-encoding occurs during expansion. Complete decoding is checked again before
the owned directory is atomically published without replacement. Single images
and GIF publish their guarded spool through the existing no-clobber writer.

Edited sequences temporarily keep both their encoded spool and final sequence
stage. They can need approximately one additional encoded output's disk space,
plus the bounded reference/metadata work. The publisher checks available space
against both actual packet bytes and the existing conservative image budget.
Frame/packet metadata scans use bounded streaming parsers; there is no new
arbitrary frame-count cap. Source DTO/resource/file-descriptor limits still
produce explicit operational errors. On Unix, edited spool/reference/palette/
log files and their sequence stage use private permissions.

`tests/image_command_edit_jobs.rs` exercises actual prepare/confirm/cancel APIs,
zero-length spools before confirmation, changed PNG/JPEG count and geometry,
an explicit grayscale PNG edit, independent compressed packet/file hashes,
GIF palette edits, all three unchanged paths, close/empty/shutdown, mutation and
late destination collision, cancellation during both spooling and packet
extraction, active shutdown, and QTGMC caption/script lifetime. These are Linux
native proofs against the pinned FFmpeg 9.0.1/R79 provider. Windows/macOS shell,
console and native filesystem execution remain separate qualification.

Reproduce the focused suite with:

```sh
. target/toolchains/qtgmc-linux-20261101/env.sh
cargo test -p media-runtime --test image_command_edit_jobs --locked -- \
  --ignored --nocapture --test-threads=1
```

Provider reconstruction and normal image/caption evidence are documented in
`linux-qtgmc-provider.md` and `source-image-caption-validation.md`.

The no-checked-video fixed-output route also uses the held image broker; see
`source-image-implicit-selection-gap.md` for its typed snapshot and deliberate
source-corruption correction. Its unchanged program keeps FFmpeg automatic
video selection across all loaded inputs, with source metadata/trim and explicit
nonvideo suppression. Its encoded spool receives the same complete image and
packet validation. The additional real-media suite is `image_implicit_jobs`.
The explicit image suite now also proves navigation cleanup of a held image
review while a previously confirmed ordinary native job completes successfully.

Source full-sequence filenames use the fork's `%8d` pattern and default start
number 1: `00000001.png` or `00000001.jpg`. Both unchanged and edited source
exports use that naming; legacy ranged API requests preserve `frame-%06d`.

`ImageProcessingSettings.metadata` carries the existing metadata controls as
`{inputs, options}`. Native preparation guards every original loaded input;
the final encoder remaps metadata/chapter donor indices after picture, bitmap,
palette or QTGMC inputs. Donor None is explicit `-map_metadata -1` /
`-map_chapters -1`. Apply-grid title/language retain explicit blanks; omission
retains source donor behavior. Output-ordinal track tags/dispositions retain the
source semantics. This metadata is applied only to the final user image output,
not the private decoded reference/palette intermediate. Source changes while a
review is held fail without publication.

The source-equivalent FFmpeg PNG, JPEG and GIF fixtures accept these title/lang
arguments but do not persist those fields in the output. Native tests prove
that same format outcome; they do not claim unsupported image tags are stored.
`explicit_fixed_images_apply_guarded_source_metadata_and_source_sequence_names`
checks all three formats with donor/None, grid-off and explicit blank/title/lang,
normal and unchanged-confirm execution, actual source outcomes, exact names and
mutation of a metadata donor. Evidence is included in
`image-final-scoped-regression.log` under the image-native validation directory.

Native fixed-image Pause/Resume is session-owned across prepare/confirm and all
rendering/validation stages. The existing footer uses the stable review ID;
Stop/navigation/shutdown join suspended work without requiring Resume. Exact
API, real process-state evidence and platform limits are in
`source-image-pause.md`.
