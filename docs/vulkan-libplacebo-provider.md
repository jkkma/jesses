# CPU encoding with Vulkan/libplacebo processing

The user's CPU-only encoding instruction excludes hardware encoders. It preserves Vulkan processing, libplacebo Spline tone mapping and GPU quality scorers. These are separate capabilities.

Both production FFmpeg 9.0.1 recipes now explicitly enable `--enable-vulkan --enable-libplacebo` under `--disable-autodetect`. They retain the 46 hardware-encoder exclusions in `scripts/cpu-encoding-policy.json`, including Vulkan encoders. No Vulkan processing API or filter is disabled to enforce that policy.

## Reproducible dependency closure

`scripts/package-libplacebo-lock.json`, hash-bound by both platform FFmpeg locks, retains these complete upstream archives and their notices:

| Dependency     | Pin                                                      | Purpose                                                                                                            |
| -------------- | -------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| libplacebo     | v7.351.0                                                 | FFmpeg 9's minimum supported libplacebo; Vulkan renderer, Spline, peak detection and Dolby Vision reshaping        |
| Vulkan-Headers | cacef3039d277c448c89336290ec3937270b0996, header 1.4.310 | Exact libplacebo submodule; public headers and matched XML registry                                                |
| glslang        | 15.1.0                                                   | Statically linked runtime shader compiler; optimizer/tests disabled, so no external SPIRV-Tools/shaderc dependency |
| fast_float     | 1bf70101536d37fa9954cc4f03fd0903d045a9f3                 | Exact libplacebo submodule                                                                                         |
| Jinja          | 15206881c006c79667fe5154fe80c01c65410679                 | Exact source generator submodule                                                                                   |
| MarkupSafe     | 28ace20b140d15c083e1cbc163ee6b7778ba098c                 | Exact source generator submodule                                                                                   |

`build-package-libplacebo.py` is the shared static-library recipe. Linux uses its existing isolated builder; `build-package-libplacebo-windows.py` adapts the same source verification/build commands to the pinned UCRT64 compiler. Meson wraps and CMake source downloads are disabled. GLSL libraries are explicitly resolved from the owned prefix. The libplacebo `.pc` repair preserves static library order using `-l` entries rather than absolute archives that FFmpeg misclassifies as compiler flags; its C++ runtime is explicit.

libplacebo enables Vulkan, glslang and Dolby reshaping, and disables unused OpenGL/D3D11, libdovi, ICC, unwind, xxhash, demos and tests. FFmpeg supplies the dynamically loaded platform Vulkan entry point; the operating system must supply its Vulkan loader and device driver. No Vulkan loader or driver is represented as a statically bundled codec library.

The exact upstream archive remains unchanged. `libplacebo-python314.patch` changes only `VkXML(ET.parse(xmlfile))` to `VkXML(ET.parse(xmlfile).getroot())`, required by Python 3.14's ElementTree constructor. Its SHA is in the processing lock and its bytes are shipped with the recipe. Pristine upstream Python 3.12 and patched Python 3.14 generate byte-identical 382,797-byte C source, SHA256 `5f82d2d56a541f2442477fae4a7964c01d2f04f62f3d2014314778c6da97f618`.

Fresh builds use the normal platform build recipes. The checked incremental Linux restoration driver preserves the prior delivery, records every reused static-prefix file, rechecks original source archive bytes before resume, and reuses the explicitly named existing FFmpeg object tree:

```sh
~/.local/bin/python3.14 scripts/restore-libplacebo-ffmpeg-linux.py \
  --previous-build target/toolchains/ffmpeg-full-20261101 \
  --build-directory target/toolchains/ffmpeg-vulkan-20261101 \
  --cache target/tool-source-cache --jobs 2
# If this owned build was interrupted, use the same command with --resume.
```

The source-complete Linux delivery is `target/toolchains/ffmpeg-vulkan-20261101/delivery`. Original CPU and historical NVENC delivery bytes were preserved. Final new FFmpeg SHA256 is `f18d560789ae2ba38aaaf665e49730a5c99a44e847e78e6e5c02bb28441fd3a1`; FFprobe SHA256 is `84a4a7934a52d0646517254a6c74a2c6ceab2860c2f116b428b910be03aa19cc`.

## Admission and execution

`qualify-libplacebo.py` checks actual compiled Vulkan/libplacebo/Spline/peak/Dolby options. Package staging requires its SHA-bound `libplacebo-capabilities.json` together with the independent CPU-only encoder inventory. Package creation does not require a physical GPU. `--exercise` renders a real frame and records the selected device; `--require-physical` additionally rejects software Vulkan devices.

The source Quick Convert selector maps Spline (GPU) to `algorithm:spline,backend:auto,peakMode:measured`. There is no source-visible Auto algorithm. An explicitly selected Spline or GPU route retains its literal Vulkan graph and fails `TONE_MAP_GPU_UNAVAILABLE` when no physical device works; it cannot silently become Hable. Other curves retain their existing Auto CPU fallback. This deliberately honors the user's explicit Spline requirement over the source fork's historical Spline-to-Hable fallback.

Held command review retains the explicit Vulkan graph even without a device. Ordinary unchanged confirmation probes it before execution; edited confirmation can run the user's valid replacement program. Image planning similarly defers the probe until unchanged confirmation. The completed source-filter phase separately proves ordinary held QTGMC defers arbitrary filter rendering until confirmation while retaining prepared script/assets.

Sparse decoded source color/static-HDR enrichment is used during actual encode preparation when header declarations are incomplete. It does not make File List probing decode a whole clip. Explicit header/frame disagreement and malformed/inconsistent HDR declarations still reject. It decodes only the first 16 packets with automatic decoder threads and requests only the static-HDR side-data fields, never the per-frame RPU or HDR10+ payloads; the complete source check below already compares every frame's color and mastering fields against the result. A tone-mapped (SDR) job does not run it merely because the header lacks mastering metadata, since the render uses per-frame metadata and the first frame's static declarations are still absorbed; missing color fields still trigger it. On a 10-minute 3832×1600 Dolby Vision 8.1 clip it went from about 8.5 minutes to 0.3 seconds.

### Complete source verification beside the encoder

Every source frame is still decoded and checked before anything is published: constant-rate timestamps, progressive fields, dimensions and pixel format, color declarations, static HDR metadata and, for Dolby Vision rendering, an intact RPU with limited-range 10-bit/no-residual metadata. Decoding a 4K HEVC source runs at roughly 200 frames per second even with every core, so that scan used to delay the encoder by about 11 minutes for a feature. The encode plan needs only the frame count, the frame clock and the first frame's declarations. Those now come from every packet's presentation time (demuxed, not decoded), checked with the unchanged cadence validator including the 24000/1001 versus 2997/125 reconciliation, plus a complete check of the first decoded frames. The unchanged per-frame validator then decodes the complete source concurrently with the encoder. A rejected frame, or a decoded frame count, clock or HDR plan that differs from the packet plan, stops the encoder and fails the job, and publication waits for the scan to pass. Variable timestamps are rejected from packets before encoding, so canonical-source normalization still starts immediately. Sources whose packets cannot stand in for decoded frames keep the previous blocking scan: a packet without a presentation time, a discarded or corrupt packet, a first packet that is not a keyframe, a first decoded frame whose time is not the first packet time, an interlaced field order in the stream header, or selected deinterlacing or cadence repair (field-coded video can store each field as its own packet). On the 10-minute clip the packet plan takes about 2 seconds instead of the 1.5-minute decode.

## Real Dolby Vision 8.1 footage

The unchanged official Dolby `P81_GlassBlowing2` asset is retained at `~/Downloads/jesses-hdr-samples/Dolby-GlassBlowing2-P81-1080p-5994-original-20261101.mp4`, with a provenance sidecar. It is 344,509,829 bytes, SHA256 `8e86ed6bb71cc71ec0394dad274c335fcfeaaa6801aed3abcea5ee98b6cd1168`. Stream and decoded-frame inspection confirms Main10 HEVC, 1920×1080, 60000/1001 fps, limited 10-bit 4:2:0 BT.2020/PQ, profile 8.1 with an HDR10-compatible base layer, 4000-nit/0.005-nit mastering metadata and per-frame RPU. Content-light values are 0/0, meaning unspecified, not a measured zero-nit picture. The sidecar records the official download URL and Chromium's primary-source reference to that same original.

Supported single-layer profile 8.1 rendering now emits `apply_dolbyvision=1`, as profile 5 already did. It requires a valid no-enhancement-layer configuration and intact RPU plus parsed limited-range 10-bit/no-residual metadata on every decoded frame. Explicit HDR10-base-layer selection remains separate and emits `apply_dolbyvision=0` after removing dynamic metadata. Neither path claims dual-layer/FEL rendering.

The historical explicit API retains its narrow strict-CFR/nonzero-origin route: it offsets only the newly encoded video back to the original origin during mux. Those legacy requests preserve their existing behavior. The source UI now freezes `ToneMapSettings.sourceWorkflow:true`; its standalone route retains the original compressed picture/RPU through tone mapping, honors output-side Exact/Frame trimming and input-side InputSeek, applies right-angle display rotation, and admits fully validated strictly increasing VFR picture clocks without pre-rendering or resampling the HDR source. The actual source Y4M producer performs the final CFR scheduling. It does not use the copied-cut ordinal-clock allowance.

`real-footage/qualification.json` under the evidence root records the earlier four legacy/render results: separate HDR10-base-layer and full-RPU Spline video outputs, plus full-RPU output with copied EAC3 and converted FLAC. Each has 240 strictly decoded 1920×1080, 10-bit BT.709 frames and no stale HDR/Dolby metadata. They preserve the input video origin (0.050 seconds for video-only, 0.076 for AV). These remain valid legacy/render evidence, **not current source-UI clock parity**. The source producer pads the positive initial video gap to 245 frames beginning at zero instead of retaining a second .076-second output offset. Copied EAC3 and converted FLAC retain their separate source audio clocks.

An independently invoked literal `libplacebo=tonemapping=spline:peak_detect=1:apply_dolbyvision=1` graph processes the preceding 120 frames before comparing frame 120 with the decoded CPU x264 result. Its 10-bit reconstruction RMSE is 8.785. The unchanged original's SHA is rechecked afterward. Commands, decoded declarations, packet and pixel receipts are retained in `real-footage/`; a review image is `real-footage/sdr-preview.png`. The retained SDR video with original sound is `real-footage/GlassBlowing2-CPU-x264-Vulkan-Spline-RPU-audio-copy-SDR.mkv`.

The official Netflix asset advertised as `Meridian_UHD4k5994_HDR_P3PQ.mp4` was also downloaded unchanged, with its CC BY 4.0 license and SHA sidecar. Actual probing finds 8-bit untagged H.264, so it is explicitly rejected as a verified 10-bit HDR qualification input despite its filename. The small Jellyfin original is a verified 10-bit PQ graphics signal fixture, not live-action footage. No downloaded media is added to Git.

Current 13-case source-UI real-footage qualification is `target/migration-validation/dolby-combinations-20261102/qualification.json`, detailed in [source-dolby-combinations.md](source-dolby-combinations.md). It proves compatible P8.1 CPU/Auto base-layer fallback, strict selected-Spline failure without Vulkan, all three native trim modes with copied EAC3, trimmed text/bitmap burn after tone mapping, a metadata90° derivative, an original-payload/RPU VFR derivative with a 250 ms presentation gap, and two compatible-base source AV1AN CPU ordinal-reader cases. CPU source requests use the fork's literal 266.667-nit anchor/clamp; the historical explicit 100-nit path remains separate. Source-compatible fallback requires the parsed no-enhancement-layer HDR10-compatible configuration; profile number alone does not authorize it. Legacy explicit APIs retain their base-layer opt-in. No dual-layer/FEL reconstruction or native Windows render is claimed.

## Native evidence

Evidence root: `target/migration-validation/libplacebo-vulkan-20261101/`.

- Physical Vulkan: AMD Radeon 890M Graphics, RADV GFX1150, integrated GPU, Mesa 25.2.8. A separate forced llvmpipe exercise records `physicalGpu:false`; it is not counted as physical qualification.
- `native-physical-spline.log`: JobManager passes non-black HDR picture through literal Vulkan Spline and standalone CPU x264, yielding 8 strictly decoded 128×96, 10-bit limited BT709 frames, no stale mastering metadata, source unchanged. Independent literal Spline reference versus decoded lossy output has 10-bit RMSE 8.192. Command, output and pixel receipts are retained in the reported fixture directory.
- `native-held-no-vulkan.log`: missing-ICD preparation returns an editable Spline/Vulkan program; unchanged execution rejects and publishes nothing; explicit CPU replacement succeeds and removes private stages.
- `native-image-held-no-vulkan.log`: missing-ICD image review remains editable for GIF/PNG/JPEG; edited valid image programs complete, unchanged PNG rejects and cleans up.
- `dolby-profile8-units.log`: 2/2 profile 8 RPU/base-layer selection, malformed/missing RPU and enhancement-layer rejection regressions. `dolby-origin-units.log`: 1/1 strict-CFR source origin, missing-RPU rejection, no cut-only ordinal allowance and no native-trim admission regression.
- `generator-regression.json`: exact generator equality described above.
- `qualified-physical-capabilities.json`, `qualified-software-capabilities.json`, `physical-vulkan-render.json`: device identities and complete rendered-frame hashes.
- `ffmpeg-recipe-tests.log`: 10/10 source/extraction/environment checks. `libplacebo-recipe-tests.log`: 4/4 lock, capability and rejection checks.
- `final-provider-build.log`: all existing multidepth codec, audio, VMAF embedded-model and libass/font native smokes pass; CPU-only inventory retained.
- `staging-proof.json`: complete source/notices/input inventory admits successfully, 32 source archives, 52 notices and 22 build inputs at that checkpoint.

Windows source recipe/admission wiring is implemented and syntax-checked. No native Windows build, Vulkan device execution or Windows media result is claimed by this Linux evidence. macOS delivery is not qualified here.

Downloaded HDR sources and subsequent real-footage results are recorded separately under `~/Downloads/jesses-hdr-samples`; copyrighted media is not checked into Git.
