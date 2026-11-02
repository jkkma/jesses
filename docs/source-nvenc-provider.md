# Historical NVENC provider investigation

**Removed capability.** The user subsequently required CPU video encoders only.
Current recipes exclude hardware encoders, omit nv-codec-headers and enforce
`scripts/cpu-encoding-policy.json` through an actual compiled inventory gate.
The old delivery and receipts below are preserved local historical artifacts;
they must not be staged as the current application provider. Vulkan processing,
libplacebo Spline tone mapping and GPU metrics remain explicitly supported.
The retired qualification/restore scripts are archived under
`docs/historical-tests/`; the commands below describe the earlier investigation.

## Evidence before removal

At this earlier checkpoint, both production FFmpeg recipes explicitly enabled `ffnvcodec` and `nvenc` even
with autodetection disabled. Linux, Windows and the Linux CI dependency builder
use the same complete `nv-codec-headers` 13.0.19.1 source at commit
`88fee5c37318c991a8762d423530f91681e32e3a`, SHA-256
`a5f838f82e6208505102c2ce257673288846b18092f1f66368340b1338cd93f4`.
The locks retain the source URL, commit, exact archive and all five original
NVIDIA header notices plus README; the source archive and readable notices are
part of the delivered manifest. The upstream headers use MIT permission notices.

The [pinned upstream README](https://github.com/FFmpeg/nv-codec-headers/blob/88fee5c37318c991a8762d423530f91681e32e3a/README)
specifies Video Codec SDK 13.0.19 and NVIDIA driver 570.0 or newer for Linux and
Windows. [NVIDIA's FFmpeg guide](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/ffmpeg-with-nvidia-gpu/index.html)
explains the separate codec-header dependency and version/driver compatibility.
The build enables the dynamic codec API; the proprietary driver is a host
runtime dependency. No CUDA toolkit or driver is added to the packaged payload.

`scripts/qualify-nvenc.py` requires actual encoder help and all source pixel
formats: H.264 420/444, HEVC 420/444/P010, and quality/Advanced options. Its
optional `--expect-no-device` runs all five real initializations and accepts only
a diagnosed NVIDIA driver/device absence. Invalid encoder options, an absent
compiled encoder, or successful hardware encoding cannot pass that negative
gate. It never labels these checks positive GPU qualification.

The complete Linux build succeeded at
`target/toolchains/ffmpeg-nvenc-20261101/delivery`, using the unchanged existing
codec prefix and FFmpeg source/object tree. The previous source-complete
`ffmpeg-full-20261101/delivery` bytes and receipt were verified and preserved.
The bounded restore driver verifies every original delivery record, checks that
only the header lock input changed, rehashes every preexisting prefix file, and
writes a distinct new delivery with baseline build receipts and reuse hashes.
The normal Linux source builder remains the clean rebuild recipe.

Actual Linux gates passed all prior codec/depth, HDR display, three embedded
VMAF models and original libass-font smoke tests. Both NVENC help gates and all
five initializations passed the required negative classification with
`Cannot load libcuda.so.1`. Receipts are `build/nvenc-capabilities.json`,
`build/nvenc-no-device.json` and `build-provenance.json` in the new delivery;
log is `target/migration-validation/ffmpeg-nvenc-20261101.log`.
The shared QTGMC `env.sh` remains pointed at the previous pair; use explicit
per-command `JESSES_FFMPEG` and `JESSES_FFPROBE` paths to the new delivery when
qualifying NVENC.

```sh
python3 scripts/test-nvenc-build.py
python3 scripts/test-package-ffmpeg-linux.py
python3 scripts/qualify-nvenc.py \
  --ffmpeg target/toolchains/ffmpeg-nvenc-20261101/delivery/ffmpeg \
  --expect-no-device --output target/test-artifacts/source-command-edit/nvenc-local.json
```

Windows source staging, explicit flags, notices and portable help validation
are implemented and covered by cross-platform recipe tests. An actual Windows
build/run remains a runner gate. Positive NVIDIA execution was never established; hardware encoding was later
removed from product scope. The NVIDIA skill catalog was checked; its Jetson codec skills
do not cover this desktop FFmpeg build, so no skill was installed.
