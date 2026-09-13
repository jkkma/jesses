# Analysis CSV and SVG export

The bitrate and quality panels export the completed request/result snapshot.
CSV retains every measured value, original stream indices, frame offsets,
source fingerprints, window units and model identity. Compressed packet bytes
remain decimal integers. Infinite PSNR is explicit. SVG retains all measured
points, labelled axes and embedded source/interval metadata; infinite PSNR
points are marked separately with gaps in the finite curve.

Exports use a native save dialog and a selected CSV/SVG extension. Bounded
validation rejects malformed/nonfinite snapshots. A flushed, owned temporary
file is published atomically without replacing existing files. Sources are not
reopened or changed; export records the identities measured at analysis time.
CSV text is quoted and protected from formula interpretation, and SVG metadata
is escaped as XML text.

Local Windows validation on 2026-09-13 covers five backend cases for
offsets, infinite values, escaping, nonfinite/oversized rejection, existing-file
preservation, exact floating-point JSON round trips and temporary cleanup. Four browser cases pass, including export
of the submitted comparison snapshot and save-dialog cancellation that creates
no export request. Svelte reports zero errors/warnings.

The real episode's 120-frame VMAF result was exported by the runtime example.
An independent CSV/XML parser verified all120scores, offsets0/2880 and both
source fingerprints. The SVG was rendered at1200x720 and inspected for clipping,
legible units and the full interval. Files and receipt remain under
`Videos/Jesses-complete-native-20260913/real-quality-vmaf.*` and
`analysis-export-verification.json`. The native bitrate export controls are
qualified below; native quality export and Linux/final package qualification
remain separate checks.

An additional independent FFprobe packet scan agrees with every one of the
original episode's 1,421 bitrate windows: 34,047 packets and 707,309,398 payload
bytes. The CSV retains all windows and exact decimal byte counts. Both SVGs
retain every plotted point and their source/interval metadata. A real pooled
VMAF score exposed a least-significant-bit change in the default JSON parser;
enabling exact floating-point round trips preserves `97.09981280000007` through
saved requests and exports. The corrected artifacts are `real-bitrate-v2.csv`,
`real-bitrate-v2.svg`, `real-quality-vmaf-v2.csv` and `real-quality-vmaf-v3.svg`; the independent receipt is
`analysis-export-v2-verification.json` in that evidence directory. Rendered
curves and labels were inspected at 1200 by 720 pixels.

Native CSV and SVG save dialogs were subsequently exercised in the rebuilt
Windows portable application on the three-track real-episode fixture. An
independent FFprobe scan verifies all 480 selected video packets and 513,271
payload bytes against every one of the 20 one-second CSV windows. The first
window contains 77,604 bytes; average bitrate is `0.20511354213497177` Mb/s and
peak window bitrate is `0.667792` Mb/s. There are no untimed or DTS-fallback
packets. All byte counts and window rates match exactly.

The independent checker recomputes the analysis fingerprint from the canonical
source path, length, modification time and sampled byte regions. It matches both
exports, whose metadata also agrees exactly. All 20 SVG curve coordinates match
the expected values within the 0.0005-pixel decimal serialization bound. Full
source SHA-256, size and modification time remain unchanged. The native artifacts
are `native-real-bitrate.csv` and `native-real-bitrate.svg`; their checker and
`native-bitrate-export-validation.json` receipt remain under
`Videos/Jesses-final-native-20260913`. This qualifies actual Windows controls and
exported data, without claiming a Linux or final clean-machine package pass.
