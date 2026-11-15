# Lean encode validation and progress

Default Quick Convert and AV1AN jobs publish after successful tool exits and a read of the completed file's stream headers. The published container must be nonempty, have a readable video header, and have the planned stream types in order. A container-declared frame count is useful diagnostic information. A discrepancy with the producer or queue count produces a warning.

Keep preparation focused on feeding the selected encoder and muxer. Successful process exits and readable output metadata establish ordinary completion; reported frame-count discrepancies remain diagnostics. External reference-source research is retained with the local verification evidence.

## Publication

- Matroska and direct ISO output use the header check on the file that will be published. Repacked MP4, MOV, and WebM use it on the repacked file. The planned stream types come from the encode selection, including explicit attachments and MOV donor timecode. MP4 and MOV chapter tracks are handled as mux-generated streams.
- Repacking gets the staging metadata and subtitle cue or data timing evidence that the mux actually uses. The source ISO receipt, owned temporary files, source identity checks, and final cancellation gate protect publication.
- Thorough verification runs the existing packet, frame, timeline, pixel, and payload checks where applicable. Decode comparisons for alpha and lossless output run in this mode.

## Processing window

SVT, x264, and FFmpeg status records update live statistics. The log shows command lines, configuration, phase messages, warnings, errors, and final summaries. Save the latest progress in the existing structured fields and filter redraws before constructing the job log. The window also recognizes redraws in older saved snapshots. FFmpeg's terminal `Lsize=` summary and x264's `encoded N frames` summary remain visible.

Classify SVT-AV1-HDR redraws as progress and retain other output in the log.

## Preparation and recovery

The [runtime design](2026-11-15-streamline-encoding-runtime-design.md) covers bounded AAC priming, average-rate selection, and compressed timestamp correction. Header output validation follows either source path. Canonical AV1AN recovery uses guarded file identity, file size, and modification time by default. Thorough mode and saved full-content receipts use SHA-256. Declared HDR sources keep the source eligibility scan before any lossless render that can change frame correspondence.
