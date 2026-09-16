# Media utilities

The Utilities screen runs each operation as an owned, cancellable process tree. Sources stay open and are checked again before a result is published. Every output must be a new path; an existing file, directory, or dangling link is never replaced.

## Dependencies

Use **Check utility tools** in the Utilities screen after installing a tool or language model. Discovery runs each tool's version command and reports the resolved executable and any failure.

| Feature                                 | Required tools                                                                                 |
| --------------------------------------- | ---------------------------------------------------------------------------------------------- |
| Lossless keyframe cut and concatenation | FFmpeg and FFprobe                                                                             |
| Matroska color metadata transfer        | FFmpeg, FFprobe, and `mkvmerge`                                                                |
| Bitmap subtitle OCR                     | FFmpeg, FFprobe, Subtitle Edit `seconv`, Tesseract, and the selected Tesseract language models |
| AV1 grain operations                    | FFprobe and `grav1synth`                                                                       |
| CRF ladder                              | FFmpeg and FFprobe; the selected FFmpeg encoder wrapper; the FFmpeg `libvmaf` filter for VMAF  |

FFmpeg, FFprobe, and `mkvmerge` can use the absolute executable overrides `JESSES_FFMPEG`, `JESSES_FFPROBE`, and `JESSES_MKVMERGE`. Otherwise utilities use verified packaged tools when present, then native executables found through absolute `PATH` entries. `grav1synth` uses absolute `PATH` entries.

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

- The input must contain a video stream readable by FFmpeg. Output is Matroska (`.mkv`).
- All input streams, global metadata, and chapters are mapped. An unsupported stream fails the operation; it is not silently dropped.
- The requested start is a presentation time. Stream copy begins at the usable demuxer seek keyframe at or before that time, so the output can contain pre-roll. For formats with coarse seek indexes this can be earlier than the nearest displayed keyframe.
- The requested end is capped at the source duration. Stream-copy timestamp rounding and decoded frame duration can move the audio/video end by a small amount. A retained subtitle event that starts inside the interval can extend the Matroska container to that event's declared end; validation therefore compares the container duration with the last retained packet or decoded frame.
- Validation compares stream count, codec/layout details, stable stream tags, dispositions, decoded frame counts, per-stream spans, and relative A/V start offsets. The accepted timing difference is derived from observed decoded frame durations. A separate error-on-decode pass must also succeed.

This is a stream-copy operation. It does not offer frame-exact cutting between independently decodable points.

## Lossless concatenation

- Choose 2 to 512 distinct inputs in playback order. Output is Matroska (`.mkv`).
- Every input must have the same ordered stream layout: codec/profile, geometry, time base, frame rate, pixel format, audio sample rate/channel layout, stable color fields, dispositions, and stable stream tags.
- FFmpeg's concat demuxer appends the streams without re-encoding. Source paths are written only to a private generated manifest and are escaped as data.
- The result must decode successfully and contain the sum of each input stream's decoded frames. Relative A/V start offsets and the summed duration are checked with frame-duration-derived tolerance.

See the [FFmpeg concat demuxer documentation](https://ffmpeg.org/ffmpeg-formats.html#concat).

## Color metadata transfer

The metadata source and target must be readable by `mkvmerge`, and the output must be `.mkv`. Choose the video stream that supplies the declaration and the target video stream whose encoded packets should remain unchanged.

The operation transfers these Matroska video-track properties when the source declares them:

- matrix coefficients, transfer characteristics, primaries, and range;
- maximum content and frame-average light levels;
- chromaticity and white-point coordinates;
- minimum and maximum mastering luminance.

It rewrites Matroska track headers. It does not rewrite codec bitstream VUI, SEI, Dolby Vision metadata, or pixels. The target video packet stream is hashed with SHA-256 before and after the remux, other stream layouts and stable metadata are checked, and every transferred property is read back from the result. See the [`mkvmerge` video-track options](https://mkvtoolnix.download/doc/mkvmerge.html).

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

`grav1synth` provides five routes:

- **Measure** compares a grain-bearing source with a denoised copy and writes a text grain table. The pair must have identical dimensions, pixel format, average frame rate, and decoded frame count. The denoised input must be a frame-for-frame derivative of that source after the same crop and resize; it is not a separate restoration or quality reference.
- **Extract** reads an existing grain table from AV1 headers.
- **Apply** writes table, advertised preset, or photon-noise grain headers to AV1 that has no grain headers.
- **Rewrite headers** deliberately replaces existing AV1 grain headers.
- **Remove** removes existing AV1 grain headers.

Apply, rewrite, and remove publish `.mkv`. They edit AV1 grain signaling without re-encoding the video. Jesses validates table syntax and ordering, reads the written headers back, and checks every stream's codec, layout, dispositions, stable tags, chapters, and stable container metadata. A full error-on-decode pass and decoded timeline comparison require the same video/audio frame counts, coverage, and relative starts. For every audio, subtitle, attachment, or data stream, per-packet SHA-256 payloads, order, timestamps, flags, sizes, and duration coverage must remain unchanged within a two-time-base-tick duration tolerance. A non-video stream with no inspectable packets is rejected before the rewrite. A table file is treated as a read-only source. See the [`grav1synth` command documentation](https://github.com/rust-av/grav1synth).

## CRF ladder

The ladder accepts progressive, square-pixel video and 1 to 8 distinct CRFs. It creates 1 to 12 centered samples of 1 to 120 seconds each. For longer sources the plan avoids the first and last five percent, capped at 60 seconds, when enough material remains.

Supported FFmpeg wrappers are `libx264`, `libx265`, `libsvtav1`, and `libvpx-vp9`. Pixel format is `yuv420p` or `yuv420p10le`. Each sample is decoded once into a frame-counted lossless FFV1 reference, and every rung encodes the same references. Candidate duration and decoded frame count must match before scoring.

Size-only ladders do not require a score. PSNR, SSIM, and VMAF scoring require explicitly tagged, supported SDR range/matrix/primaries/transfer/chroma location. Default recommendation thresholds are 45 dB PSNR, 0.98 SSIM, and 95 VMAF. The recommendation is the highest tested CRF that meets the threshold. PSNR is pooled through error energy; SSIM and VMAF are weighted by decoded frame count.

The reported bitrate is sampled video bitrate. Whole-file size is a projection across source duration and can move with unsampled scene complexity. Audio, subtitles, attachments, chapters, container overhead, production filters, and standalone-encoder-only settings do not transfer into the ladder.

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
