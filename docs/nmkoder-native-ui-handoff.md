# Exact source UI: remaining native handoff

Source root: `~/repos/nmkoder/Nmkoder`. User authority requires the
source controls/defaults and the requested paper-and-ink theme. These are confirmed runtime
gaps, not extra UI requests. The latest explicit CPU-only user scope excludes
hardware video encoders while preserving Vulkan/libplacebo Spline processing
and GPU-assisted metrics.

## Sample Encodes color formats

`UI/Tasks/UtilCrfLadder.cs:46–55` lists seven library encoders, in this order:
SVT, AOM, x265, x264, VP9, HEVC NVENC, H.264 NVENC. The latest user scope keeps
the first five CPU encoders and removes both NVENC choices. Its color menu directly uses
each encoder's `ColorFormats` and `ColorFormatDefault` (`:108–122`). The source
tables are `Data/Codecs/Video/VideoEncodersLib.cs`; exact FFmpeg names come from
`Data/PixelFormats/PixelFormats.cs:43–55`.

| Runtime encoder       | Exact ordered formats                                                          | Default     | Source table line |
| --------------------- | ------------------------------------------------------------------------------ | ----------- | ----------------- |
| `av1` / libsvtav1     | yuv420p, yuv420p10le                                                           | yuv420p10le | 235–236           |
| `aomAv1` / libaom-av1 | yuv420p, yuv422p, yuv444p, yuv420p10le, yuv422p10le, yuv444p10le               | yuv420p10le | 308–309           |
| `hevc` / libx265      | yuv420p, yuv422p, yuv444p, yuv420p10le, yuv422p10le, yuv444p10le               | yuv420p     | 58–59             |
| `h264` / libx264      | yuv420p, yuv422p, yuv444p, yuv420p10le, yuv422p10le, yuv444p10le               | yuv420p     | 22–23             |
| `vp9` / libvpx-vp9    | yuv420p, yuv422p, **yuva420p**, yuv444p, yuv420p10le, yuv422p10le, yuv444p10le | yuv420p     | 190–191           |

Implemented in `utilities/ladder_formats.rs`: all 27 CPU menu combinations have
encoder-specific admission and installed-help gates. Every retained reference
and candidate is fully decoded with exact dimensions, frame count and canonical
pixel format checked; VP9 alpha forces libvpx decoding. The 27 software choices
passed actual Jesses ladder workflows with PSNR, 4:4:4/10-bit VMAF and alpha SSIM,
24-frame retained references/candidates, varying-alpha preservation, original
bytes and no-clobber retry. Proof: `/tmp/jesses-toolchain/source-ladder-formats-real.log`.
Historical NVIDIA receipts are archived; they do not describe active menu
choices or pending qualification. `CrfLadderRequest.pixelFormat` retains each
CPU source menu value without a new user control. `nv12` is not a source choice.
VP9 yuva420p must retain alpha, with libvpx decoding and independent alpha proof;
metrics which compare visible color must not silently substitute for alpha
validation. Installed encoder format/help/probe gates should reject genuinely
unavailable formats rather than allow FFmpeg's implicit pixel-format fallback.

`quality.rs` retains complete paired frame counts/clocks and accepts matched
4:2:2/4:4:4 8/10-bit planes. Full-resolution 4:4:4 chroma has no siting offset;
HEVC's unsignaled 4:2:2 position uses nominal left, while explicitly declared
positions still have to match. See H.265 clauses6.2/E.2.1 in the
[official specification](https://www.itu.int/rec/dologin_pub.asp?id=T-REC-H.265-201911-S%21%21PDF-E&lang=e&type=items).

## Windows console presentation

`Views/MainWindow.Settings.cs:15–24` and `MainWindow.axaml:936–959` define general
console Hidden (default), Visible, Visible/stays open, plus separate AV1AN console
checked by default and disabled off Windows. `GeneralPreferences` persists them
as `consoleWindow` and `av1anConsoleVisible`. The native diagnostic-window
consumer is implemented; native Windows execution still needs its local
qualification receipt. See [Windows local validation](windows-local-validation.md).

Source general policy: `Media/AvProcess.cs:88–110` reads CmdDebugMode; visible
FFmpeg avoids output handlers. `OS/OSUtils.cs:45–51` switches UseShellExecute,
stdout/stderr redirection and CreateNoWindow together. `OS/Shell.cs:22–28` uses
Windows cmd /C or /K; Unix /bin/sh -c does not launch a new terminal or stay open.
No new Unix terminal-emulator feature is required.

Source AV1AN policy is explicitly Windows-only (`Media/AvProcess.cs:665–681`).
Its `WriteLaunchScript` (`:689–720`) titles the window av1an, keeps it visible
five seconds after finishing, and preserves the real encoder exit code. General
stay-open is separate from this fixed AV1AN delay.

The actual media process keeps CREATE_NO_WINDOW, captured pipes and atomic
JobObject containment. A separate owned native helper allocates the visible
console and displays bounded diagnostic records; binary media pipes do not
enter it. General `/K` keeps the current job/queue waiting until the diagnostic
window closes, while Stop/shutdown cancel and unblock it. Active-window close
cancels its corresponding tool, and AV1AN has an independent five-second tail.
The real tool exit status remains authoritative. Helper readiness uses its
inherited pipe; all helper processes and owned spool handles are joined/closed
before cleanup. Unix launches no extra terminal.

The standalone twelve-case Windows fixture and exact commands are in
[the Windows-local handoff](windows-local-validation.md). Linux supervisor and
policy gates and MSVC cross-target checks have passed; no native Windows
console pass is claimed yet.

## Preserved utility containers and provider qualification

Automatic `_cut`, `_grain`, `_nograin` names now preserve the compatible input
extension. There is no silent MKV substitution. Final publication remains atomic
and no-clobber, with concurrent admitted names reserved until completion/cancel.

- Actual cuts passed MKV, MP4, MOV, WebM, AVI and MPEG-TS. Every output's encoded
  packets are an ordered source slice; runtime checks retain track metadata,
  codec/timeline integrity, full decode and source identity. The TS proof fixed
  nonzero input-origin handling and demuxer audio preroll with `copypriorss=0`;
  timing tolerances were not widened.
- Grain apply/rewrite/remove passed MKV, MP4 and WebM. A source-controlled final remux
  restores stream tags/dispositions/chapters/attachments omitted by grav1synth.
  Actual proofs compare original nonvideo packet SHA256 values/order, every
  base decoded frame with synthesis disabled, chapter times/tags, source bytes
  and no-clobber behavior. Header-carried attachments and ISO chapter streams
  have complete extradata SHA256 validation instead of pretending they contain
  ordinary packets. Regenerated chapter clock denominators may differ; actual
  chapter times/tags and complete header bytes remain checked.
- MP4 generated `major_brand`, `minor_version`, `compatible_brands` entries must
  not also be duplicated into mdta: a plain FFmpeg copy otherwise reads back
  `isom;isom` and `512;512`. The mapper preserves the actual original brand and
  lets the muxer emit its real header fields once; strict metadata equality stays.
- AV1-in-MOV is an explicit unsupported codec/container combination: FFmpeg9
  refuses even the source fixture (`av1 only supported in MP4 and AVIF`). The
  grain output gate refuses it before publication; MOV H264 cuts are positive.
- The subsequent exact-provider phase qualified IVF at 24, 24000/1001 and
  30000/1001 fps: apply/rewrite/read/remove preserve the complete header clock,
  every packet timestamp, all base decoded pixels, source bytes and no-clobber.
  All 14 source stocks also pass actual IVF writes/read-back against their own
  embedded parameter hashes. This closes the previous stock 0.2 timing failure;
  no validation tolerance was weakened.

Positive compatible-container proof is now
`/tmp/jesses-toolchain/packaged-grain-provider-containers.log` (3 tests passed,
including all IVF cases), against the source-built static provider.

The fork pins `https://github.com/rust-av/grav1synth` at
`1044228cd411672b565e5762a9b3597f4dd163b0`
(`.github/scripts/bundle-tools.sh:1439–1440`). Its actual app helper uses unified
`apply --iso/--preset --replace -y` (`Media/Grav1synth.cs:138–148`); it also requires
`presets`. The previously used external binary was crates.io0.2.0: `apply --grain`, separate
`generate --iso`, no `--replace`, no `presets`. Adapter dispatch now inspects
installed `apply --help`, preserving either qualified command dialect rather
than unconditionally passing a released-tool-invalid flag. Existing-grain
rewrite keeps its explicit client authorization and validates the written output.
**Film-stock and IVF qualification passed on Linux with the exact source pin.**
The production source/locked-Cargo recipe, complete sources, static compatible
FFmpeg/dav1d, staging, discovery and native workflow gates are documented in
[grav1synth-provider.md](grav1synth-provider.md). Windows/macOS native provider
qualification and native execution of the Windows console fixture remain outstanding.
