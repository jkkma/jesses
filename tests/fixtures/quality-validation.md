# Quality comparison validation

The quality inspector compares explicit corresponding reference and candidate video
intervals with any selected combination of SSIM, PSNR, and VMAF. Every metric has
its own request, result, per-frame graph, and export. A failed or canceled metric
cannot attach a later result to an earlier request. The file selected in the Files
view is the reference; the chosen second file is the candidate. Both video stream
indices, zero-based start frames, and frame count are explicit.

Decoded inspection confirms that both intervals exist, timestamps increase within
each input, video is progressive 8/10-bit planar 4:2:0 SDR, and color metadata and
pixel format match. By default, relative frame timing must agree within 2.1 ms.
The fixed-rate option pairs corresponding frames by ordinal position when source
timestamps differ. It never creates, drops, or repeats decoded frames. Users must
still verify that both intervals show corresponding content.

Reference alignment offers no change, automatic crop, resize to the candidate,
and crop then resize. Crop detection samples the selected reference interval and
chooses a common rectangle when more than 80% of samples agree; otherwise it keeps
the union of sampled visible areas. The selected crop and measured dimensions are
included in the result message. If storage dimensions or sample aspect ratios
differ and either input is anamorphic, comparison uses even-width square-pixel
display frames. An unchanged pair with matching anamorphic storage stays untouched.
The reference alone is cropped or resized; the candidate is only de-squeezed when
needed. Incompatible final dimensions fail before scoring.

Sampling scores frames 0, N, 2N, and so on in **both** inputs, up to N=1000.
The result counts scored frames and retains the selected interval's original frame
numbers in points and exports. VMAF uses one of three built-in versioned models:
`vmaf_v0.6.1`, `vmaf_v0.6.1neg`, or `vmaf_4k_v0.6.1`. No model file is modified.
PSNR uses FFmpeg's pooled-error summary rather than averaging frame decibels;
infinite PSNR represents identical decoded pixels.

Quality analysis is read-only and cancellable. It shares the two-analysis limit,
uses supervised FFmpeg/FFprobe processes, caps source scans at one million frames,
limits requests to 60,000 frames, and bounds diagnostic output and runtime. The
selected source fingerprints are verified again after scoring. HDR tone mapping
and arbitrary metric filter graphs remain outside this inspector.

Validation: `cargo test -p media-core quality:: --lib`,
`cargo test -p media-runtime quality:: --lib`,
`cargo test -p media-runtime --test quality_jobs -- --ignored`, and
`pnpm test -- tests/frontend/quality.spec.ts`. The opt-in real-tool test uses
FFmpeg with libvmaf and FFprobe. It checks independent pixel-error arithmetic,
model-dependent VMAF scores, crop/resize/anamorphic geometry, sampling, fixed-rate
pairing, cancellation, and unchanged source hashes and timestamps.
