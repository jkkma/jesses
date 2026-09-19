# Loudness measurement and flat gain

Windows development qualification on 2026-09-13 covers full-track integrated
loudness, true peak and loudness range. Measurement reads the selected original
audio stream and chosen channel mix. It does not alter an encode draft. Applying
the proposal is explicit and requires an audio conversion codec. The proposal
uses the smaller of target gain and peak headroom, rounded down to 0.1 dB.
Silence has no finite integrated measurement or gain proposal. Manual gain accepts
−60 to +24 dB. Dynamics are preserved; no compressor or limiter is inserted.
Lossy encoding can change true peak after gain.

Measured gain records a sampled source fingerprint; encoding refuses stale
measurements before starting media processing. Channel or codec changes clear
gain. Closing/canceling an analysis discards late results, and queued requests
retain their submitted values. A trim still uses a complete-track measurement;
the result is not represented as the loudness of the trimmed interval.

Two parser tests and two browser tests pass. The actual-tool gate in
`crates/media-runtime/tests/loudness_jobs.rs` verifies the selected audio stream
against an independent decoded sample peak, silence, lossless FLAC output with
−6 dB gain, exact sample count, less than 0.000001 normalized sample error,
the resulting integrated loudness shift, stale-source rejection, cancellation,
source integrity and closed output/source handles. All spawned tools use the
owned process-tree supervisor. The shared analysis limit is two concurrent tasks.

A complete real 720p episode measured −17.52 LUFS, +0.05 dBTP and 13.7 LU range,
proposing −5.5 dB for a −23 LUFS target and −1 dBTP peak limit. Original SHA256,
bytes and modification time remained unchanged. The local receipt is retained in
`Videos/Jesses-migration-native-20260913/real-source-loudness.json`.
The frozen Windows desktop build `49c84b2425f90efa7842acc5d297fd2dd4c23458550d575a5843e1e744fb6777`
also measured that original track and applied the proposed −5.5 dB explicitly.
A native x265 encode of frames [2880,3360) retained 882,883 stereo samples per
channel in 24-bit FLAC. Independently decoded source samples multiplied by the
gain and quantized through the documented s32-to-FLAC24 path match every output
integer sample exactly. Evidence and the verifier are retained under
`Videos/Jesses-complete-native-20260913/native-trim-*`. The final Windows release
artifact remains a separate qualification; Linux runtime qualification is deferred.
