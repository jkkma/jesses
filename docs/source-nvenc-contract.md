# Historical source NVENC quality investigation

**Removed capability.** The user subsequently required CPU video encoders only.
NVENC is absent from active menus/catalogs and rejected at encoding admission.
This document retains the earlier source investigation and completed receipts;
it is neither the current implementation contract nor deferred work. The
compatibility-only `sourceNvencQuality` field can still deserialize old records.
Vulkan/libplacebo Spline processing and GPU metrics remain in scope.

## Investigation before removal

`VideoEncodersLib.cs:129–140,166–177` emits the Quick quality defaults before
Advanced. Quality zero emits `-tune lossless` alone; positive CRF emits
`-b:v 0 -cq N`. Advanced can subsequently override tune/RC. In particular,
Advanced `rc=constqp` does not turn the source CRF value into an explicit QP.
Bitrate/file-size defaults likewise precede Advanced.

The optional internal `sourceNvencQuality` marker snapshots this source adapter
behavior for every Quick NVENC quality-dropdown mode. It adds no UI control.
The source UI keeps `lossless:false`, omits `constantQuantizer`, and preserves
the Advanced rows. Legacy requests without the marker retain explicit typed
lossless/constant-QP behavior, including `-rc constqp -qp 0` when requested.

The native builder and hardware initialization probe retain source tune/RC
ordering. Source-mode validation accepts the fork's Advanced overrides without
requiring a stale typed QP/lossless mode. A source `rc=constqp` override also
supersedes target-size budget expectations; it does not claim to meet an ignored
bitrate or file size. Source frame/format/input ownership and actual media
validation remain required. A probe uses a small representative bitrate for
physical initialization and is not a file-size measurement.

Focused unit fixtures compare source zero/default/nonzero and Advanced RC/tune,
legacy QP/lossless, and source bitrate/file-size defaults plus overrides. They
exercise argument construction and typed admission without a GPU. No positive NVIDIA encoding was established. The user then removed hardware
encoding from the product scope.

Log: `target/test-artifacts/source-command-edit/nvenc-source-quality.log`.
