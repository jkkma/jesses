# Media utilities

The Utilities screen runs each operation as an owned, cancellable process tree. Sources stay open and are checked again before a result is published. Every output must be a new path; an existing file, directory, or dangling link is never replaced.

## Choosing sources and batch behavior

**Batch encode** applies an encoding recipe to each selected source. Utilities also offers **Run this utility for all files**, which prepares explicit requests in file order with individual noncolliding destinations. A utility batch has at most 100 rows, continues after an individual source fails, and reports each result or error. Cancel stops the active process trees and marks remaining rows skipped; completed outputs remain available. Concatenation still consumes one ordered input list. Other source relationships remain explicit:

| Operation          | Where to run it | Source selection                                                        |
| ------------------ | --------------- | ----------------------------------------------------------------------- |
| Concatenation      | Utilities       | One ordered list becomes one output.                                    |
| Quality comparison | Files           | One explicitly aligned reference/candidate pair.                        |
| Bitrate chart      | Files           | One selected file and stream per report.                                |
| CRF ladder         | Utilities       | One source per run; review its results before choosing encode settings. |

Selecting files in Batch does not launch utilities. Their source controls and the Utilities batch choice remain independent of Batch encode. Encode preview/queue commands still accept encode requests only; utility batching uses its own bounded request list. All batch destinations are checked for collisions before execution and each operation rechecks its sources and output before publication. **Pause** and **Continue** apply only to that active utility operation, including both stages of a pipeline; cancellation also terminates paused trees.

The active utility shows its current file and phase during a batch, with elapsed time and available media progress. Paused time is excluded. Grain measurement ETA uses the fork's approximate 7.2 megapixels per second reference rate applied to the source dimensions and frame count; decoding, denoising and hardware change the actual time. Final per-file results remain available even when another row fails.

## Dependencies

Use **Check utility tools** in the Utilities screen after installing a tool or language model. Discovery runs each tool's version command and reports the resolved executable and any failure.

| Feature                                 | Required tools                                                                                                                                                                                                                                                           |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Lossless keyframe cut and concatenation | FFmpeg and FFprobe                                                                                                                                                                                                                                                       |
| Matroska color metadata transfer        | FFmpeg, FFprobe, and `mkvmerge`                                                                                                                                                                                                                                          |
| Bitmap subtitle OCR                     | FFmpeg, FFprobe, Subtitle Edit `seconv`, Tesseract, and the selected Tesseract language models                                                                                                                                                                           |
| AV1 grain operations                    | FFprobe and `grav1synth`                                                                                                                                                                                                                                                 |
| CRF ladder                              | FFmpeg and FFprobe; the selected FFmpeg encoder wrapper; `libvmaf` or `xpsnr` FFmpeg filter when selected; for SSIMULACRA2 or Butteraugli, a VapourSynth Python runtime with L-SMASH and the CPU `vszip` or `julek` plugin (bundled on Windows; external on Linux/macOS) |
| Deinterlace to new file                 | FFmpeg and FFprobe with libx264; QTGMC additionally needs a working VSPipe, havsfunc, L-SMASH and its filter plugins                                                                                                                                                     |
| Repair padded cadence                   | FFmpeg and FFprobe with libx264, VSPipe and BestSource                                                                                                                                                                                                                   |

FFmpeg, FFprobe, `mkvmerge` and `grav1synth` accept absolute executable overrides `JESSES_FFMPEG`, `JESSES_FFPROBE`, `JESSES_MKVMERGE` and `JESSES_GRAV1SYNTH`. Otherwise utilities use verified packaged tools when present, then native executables found through absolute `PATH` entries.

The Windows package includes `mkvmerge`, the CPU quality scorers, Vship GPU scoring
and QTGMC dependencies. GPU scoring is checked for each job and uses the CPU when
the GPU probe fails. BestSource remains an optional external reader for
padded-cadence repair. OCR tools remain external. Windows/Linux package recipes now include the [source-pinned grain provider](grav1synth-provider.md), with private compatible media libraries and corresponding sources.

On Windows, Subtitle Edit `seconv` and Tesseract are discovered first in these stable per-user locations:

```text
%LOCALAPPDATA%\jesses\tools\subtitle-ocr\seconv\seconv.exe
%LOCALAPPDATA%\jesses\tools\subtitle-ocr\tesseract\tesseract.exe
%LOCALAPPDATA%\jesses\tools\subtitle-ocr\tesseract\tessdata\*.traineddata
```

The runtime rejects a managed executable that is a link, reparse point, directory, or a path that resolves outside the managed root. It supplies the managed Tesseract directory and model directory only to the owned OCR child process. The desktop therefore needs no persistent `PATH` or `TESSDATA_PREFIX` change. If a managed executable is absent, discovery falls back to absolute `PATH` entries.

The following PowerShell recipe installs the Windows x64 builds used by the real fixture. It requires `7z`, refuses to merge with an existing managed directory, checks each downloaded artifact, and does not run an installer or change machine settings:

```powershell
$ErrorActionPreference = 'Stop'
$root = Join-Path $env:LOCALAPPDATA 'jesses\tools\subtitle-ocr'
if (Test-Path -LiteralPath $root) { throw "Managed OCR directory already exists: $root" }

$download = New-Item -ItemType Directory -Path (Join-Path $env:TEMP "jesses-ocr-download-$PID")
$seconvZip = Join-Path $download 'SeConv-Windows-x64.zip'
$tesseractInstaller = Join-Path $download 'tesseract-ocr-w64-setup-5.5.3.20260724.exe'

Invoke-WebRequest 'https://github.com/SubtitleEdit/subtitleedit/releases/download/v5.2.0/SeConv-Windows-x64.zip' -OutFile $seconvZip
if ((Get-FileHash -Algorithm SHA256 -LiteralPath $seconvZip).Hash -ne '14C8B467815DD04847E0E7B1E633D222D9C14C6A307ACADC03E21112803780B7') { throw 'SeConv download hash mismatch' }

Invoke-WebRequest 'https://digi.bib.uni-mannheim.de/tesseract/tesseract-ocr-w64-setup-5.5.3.20260724.exe' -OutFile $tesseractInstaller
if ((Get-FileHash -Algorithm SHA256 -LiteralPath $tesseractInstaller).Hash -ne 'BEE9E3434BD94FD65387D9BE28CD467A41F61B1275383B55B0F59A1331270AE4') { throw 'Tesseract download hash mismatch' }

[System.IO.Directory]::CreateDirectory($root) | Out-Null
$seconv = New-Item -ItemType Directory -Path (Join-Path $root 'seconv')
$tesseract = New-Item -ItemType Directory -Path (Join-Path $root 'tesseract')
Expand-Archive -LiteralPath $seconvZip -DestinationPath $seconv
& 7z x $tesseractInstaller "-o$tesseract" -y | Out-Null
if ($LASTEXITCODE -ne 0) { throw "7z failed with exit code $LASTEXITCODE" }

$tessdata = New-Item -ItemType Directory -Path (Join-Path $tesseract 'tessdata')
$english = Join-Path $tessdata 'eng.traineddata'
Invoke-WebRequest 'https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/87416418657359cb625c412a48b6e1d6d41c29bd/eng.traineddata' -OutFile $english
if ((Get-FileHash -Algorithm SHA256 -LiteralPath $english).Hash -ne '7D4322BD2A7749724879683FC3912CB542F19906C83BCC1A52132556427170B2') { throw 'English model download hash mismatch' }

& (Join-Path $seconv 'seconv.exe') --version
& (Join-Path $tesseract 'tesseract.exe') --list-langs
```

Additional language models use their Tesseract codes as filenames, for example `spa.traineddata`. Download a compatible model family from the official [`tessdata_fast`](https://github.com/tesseract-ocr/tessdata_fast), [`tessdata`](https://github.com/tesseract-ocr/tessdata), or [`tessdata_best`](https://github.com/tesseract-ocr/tessdata_best) repository into the managed `tessdata` directory, then run **Check utility tools** again. The verified English model above is pinned to the displayed `tessdata_fast` commit.

## Lossless keyframe cut

- The input must contain a video stream readable by FFmpeg. Output preserves a compatible MKV, MP4, MOV, WebM, AVI, MPEG-TS or IVF filename/container.
- All input streams, global metadata, and chapters are mapped. An unsupported stream fails the operation; it is not silently dropped.
- The requested start is a presentation time. Stream copy begins at the usable demuxer seek keyframe at or before that time, so the output can contain pre-roll. For formats with coarse seek indexes this can be earlier than the nearest displayed keyframe.
- The requested end is capped at the source duration. Stream-copy timestamp rounding and decoded frame duration can move the audio/video end by a small amount. A retained subtitle event that starts inside the interval can extend the output container to that event's declared end; validation therefore compares the container duration with the last retained packet or decoded frame.
- Validation compares stream count, codec/layout details, stable stream tags, dispositions, decoded frame counts, per-stream spans, and relative A/V start offsets. The accepted timing difference is derived from observed decoded frame durations. A separate error-on-decode pass must also succeed.

This is a stream-copy operation. It does not offer frame-exact cutting between independently decodable points.

## Lossless concatenation

- Choose 2 to 512 distinct inputs in playback order. Output is Matroska (`.mkv`).
- Every input must have the same ordered stream layout: codec/profile, geometry, time base, frame rate, pixel format, audio sample rate/channel layout, stable color fields, dispositions, and stable stream tags.
- FFmpeg's concat demuxer appends the streams without re-encoding. Source paths are written only to a private generated manifest and are escaped as data.
- The result must decode successfully and contain the sum of each input stream's decoded frames. Relative A/V start offsets and the summed duration are checked with frame-duration-derived tolerance.

See the [FFmpeg concat demuxer documentation](https://ffmpeg.org/ffmpeg-formats.html#concat).

## Color metadata transfer

The metadata source and target must be readable by `mkvmerge`. The source-shaped ColorData dialog edits its explicitly selected target in place; legacy requests can still copy to a new `.mkv` destination. Choose the video stream that supplies the declaration and the target video stream whose encoded packets should remain unchanged. The independent color-space and HDR checkboxes default to enabled. A read-only query reports all tags, mastering/light metadata and available Dolby Vision profile information when no target is selected.

The operation transfers these Matroska video-track properties when the source declares them:

- matrix coefficients, transfer characteristics, primaries, and range;
- maximum content and frame-average light levels;
- chromaticity and white-point coordinates;
- minimum and maximum mastering luminance.

It rewrites declarations through a guarded Matroska stage and restores MP4, MOV or WebM targets to their actual container family. It does not recolor pixels or replace codec SEI/Dolby Vision payloads. Every effective property is read back, stable metadata/chapters and complete packet bytes/timing are verified, and only then does an atomic selected-target replacement commit. An incompatible declaration/codec/container combination rejects with the original target preserved; the tested H264 MP4/MOV static-HDR replacement is one such case; MKV/WebM static-HDR replacement has positive real-media coverage. Selecting neither checkbox or transferring absent HDR alone leaves the target unchanged. Existing explicit new-output requests retain no-clobber publication. See the [`mkvmerge` video-track options](https://mkvtoolnix.download/doc/mkvmerge.html) and [implementation evidence](encoding-parity.md#colordata-declaration-editing).

## Bitmap subtitle OCR

The selected stream must be one of FFmpeg's bitmap subtitle codecs:

- Blu-ray PGS (`hdmv_pgs_subtitle`);
- DVD VobSub (`dvd_subtitle`);
- DVB bitmap subtitles (`dvb_subtitle`);
- XSUB (`xsub`).

The selected track is copied into a private single-track Matroska subtitle file, then Subtitle Edit `seconv` performs timed OCR through Tesseract. Output is UTF-8 SubRip (`.srt`). Text subtitle streams do not need OCR and are rejected by this route.

Language values are Tesseract codes reported by `tesseract --list-langs`, such as `eng`, or `+`-joined installed codes such as `eng+spa`. Every requested component must be installed. Missing `seconv`, Tesseract, or `.traineddata` files produce an actionable dependency diagnostic. Jesses checks that the SRT contains ordered, positive cue intervals and recognized text, but names, punctuation, line breaks, and accessibility text still need review.

PGS-to-SRT is covered by the real-tool fixture. VobSub, DVB, and XSUB are accepted by the runtime and supported by the [Subtitle Edit `seconv` interface](https://github.com/SubtitleEdit/subtitleedit/blob/main/docs/features/seconv.md), but they were not part of the current real fixture and remain dependent on the installed FFmpeg and `seconv` builds.

## AV1 film grain

`grav1synth` provides these routes:

- **Denoise and measure source** creates a private lossless FFV1 reference with the fork's spatial hqdn3d settings, measures its difference from the original and writes a reusable table. Denoise strength is 1–16, default 4. An optional new `.mkv` destination keeps the verified denoised copy; otherwise the private reference is cleaned up after the operation. The original is never modified.
- **Measure against denoised video** compares a grain-bearing source with an explicitly supplied denoised copy and writes a text grain table. The pair must have identical dimensions, pixel format, average frame rate, and decoded frame count. The denoised input must be a frame-for-frame derivative of that source after the same crop and resize; it is not a separate restoration or quality reference.
- **Extract** reads an existing grain table from AV1 headers.
- **Apply** writes table, advertised preset, or photon-noise grain headers to AV1 that has no grain headers.
- **Rewrite headers** deliberately replaces existing AV1 grain headers.
- **Remove** removes existing AV1 grain headers.

Apply, rewrite, and remove preserve compatible MKV, MP4, WebM and IVF containers. The source-pinned provider passed all 14 stock writes and exact integer/fractional IVF clocks; AV1-in-MOV is explicitly refused. They edit AV1 grain signaling without re-encoding the video. Jesses validates table syntax and ordering, reads the written headers back, and checks every stream's codec, layout, dispositions, stable tags, chapters, and stable container metadata. A full error-on-decode pass and decoded timeline comparison require the same video/audio frame counts, coverage, and relative starts. For every audio, subtitle, attachment, or data stream, per-packet SHA-256 payloads, order, timestamps, flags, sizes, and duration coverage must remain unchanged within a two-time-base-tick duration tolerance. Header-carried attachments and ISO chapter data are checked by complete extradata SHA256 and chapter timing/tags. Other non-video streams with no inspectable packets remain rejected before the rewrite. A table file is treated as a read-only source. See the [`grav1synth` command documentation](https://github.com/rust-av/grav1synth).

## CRF ladder

The ladder accepts progressive, square-pixel video and 1 to 8 distinct CRFs. It creates 1 to 12 centered samples of 1 to 120 seconds each. For longer sources the plan avoids the first and last five percent, capped at 60 seconds, when enough material remains.

Supported CPU FFmpeg wrappers are `libx264`, `libx265`, `libsvtav1`, `libaom-av1` and `libvpx-vp9`. Each must be advertised by the selected installed FFmpeg. Hardware encoder choices and execution are excluded by the latest user request; Vulkan/libplacebo Spline processing and GPU metrics remain supported. Pixel format is `yuv420p` or `yuv420p10le`. Each sample is decoded once into a frame-counted lossless FFV1 reference, and every rung encodes the same references. Candidate duration and decoded frame count must match before scoring.

The fork's content presets for x264, x265 and SVT share one definition with regular encoding. The backend probes their actual parameter values against the selected FFmpeg wrapper and reports every unsupported control it omits. The result records the applied values and preset identity. SVT sample jobs use parallelism level 1 (`lp=1`) and probe 24 textured frames. The pinned SVT 4.2.0 library and standalone encoder both crashed on the combination `hbd-mds=1` plus `enable-overlays=1` at higher parallelism on short 10-bit clips; level 1 keeps both controls enabled. Ordinary SVT and AV1AN jobs cap that exact combination to level 1 and state the effective allocation in the job log. Explicit 8-bit ordinary output rejects `hbd-mds=1` before queueing; an 8-bit ladder reports that unsupported preset row as omitted. AOM currently offers their encoder controls without a built-in content preset. VMAF offers standard, NEG and 4K models; the selected model is retained with the result.

**Keep sample encodes** requires a new directory. After every sample has passed decoded frame-count and timing checks, the runtime publishes the retained FFV1 references and encoded candidate clips without replacing an existing directory. Cancel removes only owned staging files.

Size-only ladders do not require a score. PSNR, SSIM, VMAF, SSIMULACRA2, Butteraugli INF, and weighted XPSNR scoring require explicitly tagged, supported SDR range/matrix/primaries/transfer/chroma location. SSIMULACRA2 and Butteraugli use the selected Windows AV1AN installation's CPU VapourSynth plugins, or an external Python/VapourSynth provider on Linux/macOS. A bounded real scorer probe runs before sample encoding and reports missing readers/plugins. On Unix, `JESSES_VAPOURSYNTH_PYTHON` can select an absolute interpreter path; `JESSES_VAPOURSYNTH_PLUGIN_PATH` selects its extra plugin directory. Existing provider module/configuration paths remain available to that child. An optional `JESSES_VSHIP_PLUGIN` must pass the actual GPU metric probe before use; failed GPU scoring restarts all rungs consistently on CPU. The reproducible pinned Linux provider is documented in [Linux VapourSynth scorers](linux-vapoursynth-scorers.md). Butteraugli uses the 203-nit scale; lower scores mean less distortion. Weighted XPSNR combines the Y/U/V error energies in a 4:1:1 ratio for 4:2:0 video. Every metric checks the expected frame count and rejects missing or nonfinite frame scores.

Default recommendation thresholds are 45 dB PSNR, 0.98 SSIM, 95 VMAF, 80 SSIMULACRA2, and 4 Butteraugli. Weighted XPSNR has no automatic threshold; enter one to request a recommendation. The recommendation is the highest tested CRF meeting the threshold. Butteraugli qualifies at or below its threshold; other scores qualify at or above it. PSNR and weighted XPSNR pool sample error energy; SSIM, VMAF, SSIMULACRA2, and Butteraugli average by decoded frame count.

The reported bitrate is sampled video bitrate. Whole-file size is a projection across source duration and can move with unsampled scene complexity. Audio, subtitles, attachments, chapters, container overhead and production filters do not transfer into the ladder. Standalone-only preset controls are reported as omitted when the FFmpeg wrapper cannot accept them.

## Temporal exports

**Deinterlace to new file** exports progressive H.264 in Matroska at CRF 12. QTGMC is the default method and defaults to its Very Slow preset; BWDIF and YADIF are also available. Choose one frame per field to double the source's frame rate, or one frame per source frame to retain it. Select the field order explicitly when the source declaration is wrong. The original source stays intact and the output is offered for optional import after validation.

**Repair padded cadence** is for an interlaced capture whose decoded frame count exceeds its duration at its declared frame rate by more than 2%. It uses decoded presentation timestamps to place each output frame and frame-content differences to prefer real pictures over repeats near that time. The result remains interlaced at the declared rate, so it can be deinterlaced afterward. A file whose count already fits its duration is refused before writing a redundant copy. The input must have a declared field order, a measurable duration and rate, and a working BestSource reader.

Both exports copy audio and supported subtitle and attachment streams, preserve source color and aspect declarations, and reject tagged HDR because this CRF 12 x264 path cannot establish static HDR metadata preservation. Jesses checks every expected decoded frame, output rate and field order, full decode, copied packet payload order, stable tags, chapters, and container duration before publishing to a new path.

## Real-tool validation

The opt-in Windows fixture builds its own media and exercises concat, an ordinary cut, a B-frame cut with an intentional audio offset, color transfer, an SSIM ladder, every grain route on AV1 with offset FLAC audio and SubRip subtitles, and PGS-to-English SRT OCR. It independently checks grain-route video frame coverage, stream count, audio/subtitle packet hashes, and packet starts, then compares source bytes after the operations. On 2026-09-15 it passed with:

- FFmpeg/FFprobe `N-126122-gca821e458a-20260813`;
- MKVToolNix `mkvmerge` 93.0;
- `grav1synth` 0.2.0;
- Subtitle Edit `seconv` 5.2.0;
- Tesseract 5.5.3 with `eng.traineddata`.

The downloaded fixture archives and extracted copies live below ignored `target/utility-tools`; they are not application runtime dependencies or user-managed installations. The validation host also has copies in the managed per-user paths documented above. This command deliberately omits `seconv` and Tesseract from `PATH` and clears `TESSDATA_PREFIX`, proving normal desktop discovery and the child-only OCR environment:

```powershell
$toolBin = 'C:\path\to\ffmpeg-mkvmerge-grav1synth'
$env:PATH = "$env:USERPROFILE\.cargo\bin;$toolBin;$env:SystemRoot\System32;$env:SystemRoot;$env:SystemRoot\System32\Wbem"
Remove-Item Env:TESSDATA_PREFIX -ErrorAction SilentlyContinue
cargo test -p media-runtime --test utility_jobs -- --ignored --nocapture
```

The test is meaningful runtime evidence for those tool builds and generated fixtures. It is not packaged clean-machine or cross-platform qualification.

On 2026-09-26 a separate native temporal fixture passed BWDIF and QTGMC bob exports of an eight-frame interlaced source, each producing 16 progressive frames with copied PCM audio and unchanged source bytes. A second BWDIF export retained a non-square 4:3 sample aspect ratio. A padded capture with 36 coded pictures at 24 fps produced a 24-frame interlaced cadence-repair output, retaining audio and source bytes. An unpadded capture and tagged HDR input were refused without publishing an output; active cancellation removed its temporary output and left the source unchanged.

On 2026-11-01 the actual Jesses AOM/SVT ladder matrix passed against the verified source-built FFmpeg/FFprobe 9.0.1 pair: both AV1 encoders used standard, NEG and 4K VMAF models (six workflows). Every workflow published and independently decoded a 24-frame reference plus a 24-frame candidate, preserved original bytes, and refused a repeated samples-directory destination. SVT retained all eight Game Capture controls at `lp=1`; a separate 8-bit run reported the high-bit-depth-only row as omitted and completed successfully. The standalone parameter suite also encoded all eight Game controls with the same cap.

The Linux external CPU provider passed the actual portable ladder fixture for SSIMULACRA2 and Butteraugli with L-SMASH source decoding, complete retained frame counts and source preservation. This is Linux execution evidence; macOS and Windows qualification remains attached to their native checks. Hardware video encoders were subsequently excluded by explicit user request.

On 2026-11-01 the exact source ladder color-format matrix passed all 27 software
choices through actual Jesses workflows: 4:2:0/4:2:2/4:4:4 8/10-bit and VP9 alpha,
complete retained24-frame decoding, PSNR, 4:4:4/10-bit VMAF and alpha SSIM, original
bytes and no-clobber. Historical NVENC P010 investigation is archived; those encoder choices are no
longer exposed or runnable under the explicit CPU-only scope.
The preserved-container fixture passed six cut container families and MKV/MP4/
WebM grain operations. See [native source UI handoff](nmkoder-native-ui-handoff.md)
for exact container evidence. [Source-pinned provider qualification](grav1synth-provider.md) now covers all 14 film stocks, IVF clocks and actual grain measurement; native Windows/macOS qualification remains separate.
