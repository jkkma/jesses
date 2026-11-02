# CPU video encoder scope

The user's explicit request supersedes the earlier NVENC deferral: Jesses uses
CPU video encoders only. Vulkan processing, libplacebo Spline tone mapping and
GPU-assisted metrics/scoring remain in scope. Imported media and packet copy are
supported regardless of how another program originally encoded the source.

Quick Convert exposes x264, x265, VPX, SVT-AV1 and AOM alongside source Copy,
Strip and fixed-image outputs. Sample Encodes exposes SVT, AOM, x265, x264 and
VP9. A mutable old hardware draft resets to the visibly selected CPU default;
saved jobs keep their actual historical settings. Compatibility-only enum/field
tokens deserialize old records, while active hardware encode requests return
`HARDWARE_ENCODING_UNSUPPORTED` before tool discovery or queue mutation.
Ordinary/held admission, batch preview/admission, parameter catalogs, Sample
Encodes and Saved/New/held recovery use this policy. Copy/Drop may retain an
unused historical hardware selector without performing a CPU video re-encode.
Generic edited shell commands retain the existing deliberate user-edit contract.

Linux/Windows FFmpeg recipes and the CI build explicitly exclude every pinned
hardware encoder implementation in `scripts/cpu-encoding-policy.json`, including
Vulkan encoders. No GPU processing backend is disabled. The actual compiled
inventory gate requires software encoders and rejects hardware names even when
no device is available. The delivery retains that executable SHA-bound receipt;
staging rejects historical hardware-enabled deliveries. Retired code/scripts and
completed old receipts remain historical evidence under `docs/historical-tests`
and `target/`; they are not pending product capabilities.

Focused evidence (all passing at this phase):

- `tests/cpu_encoding_only.rs`: three public API/history rejection gates plus
  one actual-media compatibility gate. Normal/held/batch/catalog/utility and historical
  recovery rejection, plus real Copy/Drop and per-file batch with unused selector.
- `scripts/test-cpu-encoding-policy.py`: all46 pinned exclusions, future encoder
  names in known hardware families, required CPU inventory, preserved processing.
- `tests/frontend/source-encode-parity.spec.ts`, `source-zero-quality.spec.ts`,
  `batch.spec.ts`, `source-shell.spec.ts`: exact CPU menus, all CPU zero-quality
  and frozen batch choices, all27 CPU Sample Encodes color choices.

Native logs: `target/test-artifacts/source-command-edit/cpu-admission-tests.log`,
`cpu-copy-drop-real.log`, `cpu-provider-inventory.json`,
`cpu-rejects-hardware-provider.log`. Browser log: `cpu-menus-browser.log` in that
same directory. Provider GPU processing evidence is owned by the separate
Vulkan/libplacebo qualification phase.

The CPU parameter catalog units passed9 (one installed-tool gate remains separately
ignored), rate-control units4, source GOP/resize dispatch1, CPU policy4, source
Quick/AV1AN menu/zero/batch browser5, and Sample Encodes full CPU format/menu
browser1. These counts do not claim hosted Windows/macOS execution.
