# Quick Output File metadata and donor None

Source authority is `Nmkoder/UI/Tasks/QuickConvertUi.cs:1156–1190,1196–1292`.
The Output File row initializes from the current file's edited or probed title
and language. Saving trims both. Apply unchecked omits grid overrides; Apply
checked in a single-file task sends explicit strings, including empty strings
that clear inherited values. The fork suppresses the entire grid in a batch.

`EncodeSettings.outputMetadata?: {title?:string, language?:string}` snapshots
these global overrides. An omitted key inherits its selected donor; `""`
clears that tag. The consumer appends these overrides after donor mapping in
normal Quick copy, encode and final-mux commands. Held unchanged commands use
the same consumer; edited programs contain the same complete mapped arguments.
Normal validation checks the actual explicit values, including absence after
clearing. The setting is persisted with the immutable job request.

`copyMetadata?:boolean` and `copyChapters?:boolean` distinguish the source's
None selections from omitted legacy defaults. False emits `-map_metadata -1`
or `-map_chapters -1`; true/omitted uses the selected donor or current input.
A None selection cannot also name a donor path. Batch previews copy these two
flags into every per-file request and have no Output File grid field.

The actual qualified FFmpeg fixture confirms that the fork's
`-map_metadata -1` strips inherited ordinary stream tags as well as global
metadata. Jesses follows that behavior, then restores explicit grid track
overrides. The fork explicitly restores attachment metadata, and Jesses retains
attachment filename, MIME type and bytes. Chapter None is independent: metadata
None can retain selected chapters, and chapter None can retain ordinary global
and stream metadata. This is not implemented as an unconditional tag wipe.

`tests/source_output_metadata.rs` contains real copy/x264 and held unchanged/
edited compound cases. It checks Unicode/global language values, donor/default
inheritance, explicit blanks, independent None choices, stream title/language,
explicit track restoration, attachments, chapters, source bytes and persisted
snapshots. A separate native batch case checks Current Input per file and None,
without grid overrides. Frontend source request tests independently check grid
suppression, trimming and selector reset/current-input behavior.

```sh
. target/toolchains/qtgmc-linux-20261101/env.sh
cargo test -p media-runtime --test source_output_metadata -- \
  --ignored --nocapture
```

The log is `target/test-artifacts/source-command-edit/metadata-real.log`.
Fixed-image and implicit-image metadata behavior is documented by those route
owners; the normal encode consumer does not claim that every image format can
store global container metadata.

## Final container representation

A bounded actual audio-container matrix also covers OGG (Opus), M4A, MP4 and
MOV (AAC), with independent source-equivalent FFmpeg commands for comparison.
The existing Jesses OGG policy preserves global tags under `JESSES_CONTAINER_*`
comments because Vorbis comments belong to streams. This namespace is an
intentional existing representation difference, not source-identical metadata.
Standard title/language comments match the source's precedence: inherited or
explicit track values win over globals; with metadata None, explicit globals
become ordinary stream comments when no track override is present. The tests
check both title and language, including explicit track grid overrides, so the
new Output File row cannot unexpectedly overwrite a track title/language.

The existing ISO-container policy uses `use_metadata_tags`; it preserves a
global language that the source's default ISO muxer can omit. Actual M4A/MP4/MOV
titles match the source reference and None clears the inherited tags while
retaining valid audio. Verification follows these known container policies
rather than requiring final OGG tags to appear in `format.tags`. These checks
cover the new metadata consumer, not every codec/container combination.
