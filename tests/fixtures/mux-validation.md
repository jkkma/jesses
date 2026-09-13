# Multi-source Matroska remux qualification

Local qualification: Windows, 2026-09-13. These checks use generated media and
the installed FFmpeg/FFprobe pair. They do not establish native desktop or remote
Linux CI qualification by themselves.

## Behavior

Remux offers a combined mode for choosing imported source files and ordering their
video, audio, subtitle, and attachment tracks. A track is identified by its stable
source ID and original stream index. Removing another source does not renumber or
reset surviving track edits. Attachments stay after media tracks. Container metadata
and chapters can come from independently chosen sources; chapters can also be omitted.
Track titles, three-letter languages, and default/forced flags can be preserved or
overridden. An empty title or language explicitly clears the tag.

The backend accepts at most 32 sources and 256 distinct tracks. It holds every source
guard until completion, rechecks every source before publishing, rejects repeated
canonical sources and existing destinations, and writes only a reserved temporary
Matroska output. Source guards deny writes and deletion on Windows; other platforms
recheck source identity and metadata. Sources remain read-only.

The output must pass stream, frame/packet count, codec, metadata, chapter and attachment
checks. Every copied video/audio/subtitle packet is then compared against its original
source for SHA-256 payload, byte count, packet order, PTS/DTS and known duration. The
timestamp tolerance is 2 ms for Matroska timebase rounding. The streaming comparator
retains at most 32 packet records, bounds each parsed record to 8 KiB, supervises both
FFprobe process trees, and honors job cancellation before publication. Incompatible
copy operations fail validation rather than silently changing packet contents or timing.

This mode copies tracks. It does not convert subtitle formats, burn subtitles,
retime sources, trim timelines, or convert audio. Job history persists the full
multi-source request alongside a compatibility summary. Existing single-source
receipts remain readable; inconsistent new receipts are retained and block admission.

## Executed checks

- `cargo test -p media-runtime --test mux_jobs --locked -- --ignored --test-threads=1`
  — **4 passed**. Independent FFV1 video and FLAC audio sources, colliding original
  stream indices, source order different from output order, SRT cues, font attachment,
  independent global metadata/chapter owners, Unicode/apostrophe/dollar-sign paths,
  per-track title/language/default/forced changes, explicit tag clears, no chapters,
  source SHA-256/mtime preservation, full request history roundtrip, source destination
  collision, cancellation during preparing and finalizing, lock release and partial cleanup.
  The external SRT case independently extracts and compares both complete cue records.
- `cargo test -p media-runtime --lib jobs::mux:: --locked -- --include-ignored`
  — **4 passed**, including an actual-tool comparator gate that accepts the original
  packet stream and rejects changed payloads, a missing final packet, and a 250 ms
  timing shift. Other cases cover stable identity, malformed settings, metadata flag
  preservation, bounded records, and missing hashes.
- `cargo test -p media-runtime --lib jobs::history:: --locked` — **3 passed** on Windows,
  including preservation/rejection of a mux receipt with a mismatched compatibility
  summary, mixed encode settings, or a removed source identity.
- `pnpm exec playwright test tests/frontend/mux.spec.ts tests/frontend/remux.spec.ts`
  — **16 passed**. Combined mode has three browser integration cases covering source
  identities, independent owners, output order, edit/removal stability, immutable saved
  requests, late IPC replies, invalid selection/language, and recoverable start errors.
  The existing 13 single-source remux cases still pass.
- `cargo clippy -p media-runtime --all-targets --locked -- -D warnings` and
  `cargo check -p jesses --locked` passed.

Linux CI runs the actual mux job and comparator gates after installing the verified
FFmpeg tool pair. A successful run of that workflow is separate evidence; adding the
gate does not establish it passed remotely.

Native Windows combined-source remux also passed on frozen executable
`49c84b2425f90efa7842acc5d297fd2dd4c23458550d575a5843e1e744fb6777`.
The UI selected a real 480-frame x265 excerpt and a separately imported FLAC
file, deselected the first source's audio, moved the second source's original
stream #0 before subtitles, and set its Unicode title and Japanese language.
The resulting 27-stream output preserves all copied packet payloads and PTS/DTS,
all 24 font hashes, and the requested title/language/order. The independent
output digest is `21fbb9e49d0af1b566dce0a613b5af1a3b533559a08d59cadb3063a38a1aad15`.
Source files remained stable during independent checking, and the separately
extracted audio hash still matches its recorded pre-import hash. Receipts and
screenshots are under `Videos/Jesses-complete-native-20260913`, including
`native-mux-output-independent.json` and `native-mux-track-review.png`.
