<img src="assets/branding/jesses-icon-master.png" width="128" alt="jesses app icon">

# jesses

A desktop app for video encoding, muxing, and media analysis, created by **jkkma**.

Rust, Tauri, Svelte, and shadcn-svelte.

The development build provides a desktop media workspace with native file
selection and drag/drop, FFprobe metadata and stream inspection, and detection of
FFmpeg, FFprobe, standalone SVT-AV1, its 5fish and HDR builds, x264, and av1an. The interface uses a fixed
parchment-and-rust light theme. Remux and Combined mux copy selected streams from
one or several sources, with track ordering, metadata ownership, progress,
cancellation, and output validation. Matroska, MP4, MOV and WebM destinations
have explicit codec and track compatibility checks.

SVT-AV1 is the primary encoding workflow. **SVT-AV1-HDR is the default**,
**5fish is the anime option**, and mainline SVT-AV1 is also available. Quick Convert drives
standalone SVT-AV1 and x264 executables, plus FFmpeg libx265 (HEVC) and libvpx (VP9). The separate av1an
tab handles scene detection and parallel SVT-AV1 chunks. Both workflows copy
selected audio, subtitles, and attachments by default. Quick Convert and standalone
batch jobs can trim, crop, resize, add black borders, process frame rates and
convert individual audio tracks. SVT-AV1 supports validated HDR10 output
and optional film grain synthesis; x264 currently supports SDR H.264 output.
Folder import and Batch encode prepare
multiple files with individual track selections and common quality settings.
Jobs run sequentially, and their settings and history survive restart. av1an
supports configurable chunk readers and quality targets, live pause/continue,
and durable stop/resume. Encoding forms offer source previews and automatic crop
proposals; Files offers bitrate charts and matched-interval quality analysis. Audio controls offer measured
loudness and explicit flat gain. General preferences and recent media persist.
Unsigned packages and bundled media tools are being qualified. This is a
development build; final combined artifacts and cross-platform release checks
remain in progress.

## Media utilities and source inspection

The **Utilities** tab provides lossless keyframe cuts and concat, color metadata
transfer, subtitle OCR, AV1 grain tools and sampled CRF ladders. Dependencies are
checked before execution and outputs use new destinations. See
[media utilities](docs/media-utilities.md) for supported formats, optional tools
and validation limits.

[Images and sequences](docs/images.md) supports explicitly ordered still-image
imports and PNG/JPEG/GIF exports. The Files inspector adds cancellable thumbnails
with display rotation and pixel aspect ratio. Quick Convert and av1an can keep
an [encoding source selected independently](docs/source-selection-and-previews.md)
while another file is inspected.

[Notifications and finish actions](docs/completion-actions.md) are session-only.
An explicitly armed close/shutdown action requires a successful queue and an
abortable 60-second countdown that remains visible across tabs. Saved requests
can be exported and inspected without executing foreign command strings.

## Encode a file

Add a local file in Files, open Quick Convert, choose the video and copied tracks,
and select a new destination and compatible container. The default SVT-AV1-HDR build starts at CRF 30,
preset 2, and Film grain retention tune. Mainline SVT-AV1 starts at CRF 30 and preset 4.
The ordinary SVT range remains CRF 1–63 and presets 0–13. Quarter-step CRF through
70 and research presets down to -3 are accepted only when the selected installed
build advertises them. The output video is 10-bit AV1.
FFmpeg, FFprobe and the selected encoder must be available through the configured
tool paths, a verified bundle or PATH. See the setup instructions below and
[package documentation](docs/packaging.md). The selected tool and its version
appear in the job log.

Choose **SVT-AV1 5fish** for anime or **SVT-AV1-HDR** for HDR movies in Quick
Convert, av1an, or Batch encode. Each build has its own executable, draft settings,
output suffix, capability result, and saved job identity. Jesses verifies the
binary's version signature before encoding and fails explicitly if another build
occupies its configured path. It never substitutes a different SVT build.

5fish starts at CRF 18, preset 2, line-art bias 5, and texture bias 4. Both bias
controls accept 0–7 and are only sent to 5fish. These defaults follow the
[5fish maintainer's high-quality anime guidance](https://github.com/5fish/SVT-AV1).
SVT-AV1-HDR starts at CRF 30, preset 2, and **Film grain retention** (`--tune 5`);
**Visual quality** (`--tune 0`) is also available, following the
[HDR project's tuning guidance](https://github.com/juliobbv-p/svt-av1-hdr).
Grain retention tunes the encoding of source texture; optional grain synthesis is
a separate control and remains off by default. Selecting either fork does not
grant permission to discard dynamic HDR metadata. Extended and fractional SVT
controls are gated by the exact build's help; unsupported values fail before the
source is encoded.

See [SVT fork setup and validation](docs/svt-forks.md) for pinned tool installation,
custom executable paths, and the checks covering both workflows.
SVT-AV1 remains the primary workflow. Existing x265 HEVC and VP9 choices keep their
FFmpeg library routes. Separate **AOM AV1**, **VPX VP9**, and **x265 HEVC** choices
run their installed encoder executables directly. The VPX and x265 routes use
mkvmerge to assign the exact planned rational cadence before the selected-track
mux; persisted FFmpeg jobs are never redirected to these tools. See
[standalone encoder drivers](docs/standalone-encoders.md) and the
[feature-extension validation record](docs/feature-extension-validation.md).

Choose **x264 · H.264** in Quick Convert for direct x264 encoding, or choose x264
with the standalone workflow in Batch encode. It defaults to CRF 23 and the
**medium** preset; CRF 0–51 and the ten named x264 presets are supported. The
installed executable must advertise Y4M input, Matroska output, and the source's
8-bit or 10-bit depth. Source depth, range, dimensions, cadence, and SDR color are
retained; x264 currently requires explicit left, center, or top-left chroma
placement. HDR input needs the explicit HDR/HLG to SDR option. Film grain synthesis
and HDR10 fallback are rejected for x264. **Lossless** is a separate mode and uses
the encoder's explicit lossless setting rather than treating CRF 0 as a promise.
Before publication, Jesses decodes and compares every output pixel with the exact
post-filter frames supplied to the encoder. Existing SVT-AV1 settings and saved
jobs remain readable.

x264 writes a timed Matroska intermediate through the supervised pipeline before
selected tracks are muxed into the chosen compatible output container. This preserves B-frame
presentation order without disabling B-frames or reconstructing timestamps from a
raw H.264 stream. The completed output must pass the same complete decoded-frame,
track, and metadata checks before publication. See the [x264 validation record](tests/fixtures/x264-validation.md).

Choose **x265 · HEVC** or **VP9 · libvpx** in Quick Convert or standalone Batch
encode. x265 defaults to CRF 28 / medium, with CRF 0–51 and ten named presets.
VP9 defaults to CRF 32 / speed 2, with CRF 0–63 and speeds 0–5 using constant
quality and the good deadline. Both preserve tagged SDR 8-bit or 10-bit 4:2:0,
full/limited range, and explicit left/center/top-left chroma. HDR input needs the
explicit HDR/HLG to SDR option; HDR output, SVT grain synthesis, and av1an are
unavailable for these two encoders. Dedicated lossless mode is available when the
installed library advertises it and passes complete decoded-pixel comparison. FFmpeg must advertise
the selected library and pixel format; missing support never triggers automatic
depth conversion. The supervised pipeline writes a timed Matroska intermediate,
then applies framing/audio settings and validates the complete decoded output.
See [x265 and VP9 qualification](tests/fixtures/ffmpeg-video-validation.md),
including a verified 480-frame excerpt from real 720p media.

Standalone Quick Convert and Batch encode offer **Constant quality**, **Lossless**,
**Video bitrate**, or **Target file size** where the selected encoder supports that
mode. Lossless disables quality and bitrate controls and must pass complete decoded
pixel equality after all selected processing. Video bitrate is a whole
number from 1 to 100000 decimal kb/s, with optional two-pass allocation. Target
size is a whole number from 1 to 1048576 MiB per file and always uses two passes.
It measures the selected audio, subtitles, attachments, and their container
overhead using the actual conversion and trim settings before calculating the
video budget. It reserves another 1% plus 64 KiB for video/container overhead.
Targets are approximate: content and encoder decisions can undershoot or exceed
them, and final container conversion can change overhead. Actual bytes and the
difference are recorded in job history. Every pass starts a fresh decoder and
encoder, and cancellation removes owned pass outputs and statistics. Omitted
rate settings retain CRF behavior in old jobs. av1an supports CRF and per-chunk perceptual quality targets.
See [rate-control qualification](tests/fixtures/rate-control-validation.md).

**H.264 NVENC** and **HEVC NVENC** use FFmpeg's NVIDIA encoders. Admission executes
a bounded one-frame initialization with the selected depth and mode, so a missing
or incompatible NVIDIA GPU or driver produces an actionable capability error
before source encoding. The Windows qualification host has an AMD GPU; its negative
hardware rejection is evidence for the gate, not a positive NVENC output claim.

Encoding supports validated constant-frame-rate SDR video with
explicit color metadata and 4:2:0 8-bit or 10-bit input. Square pixels remain the
default; an explicit output SAR or DAR converts non-square source geometry without
resampling it. SVT-AV1 HDR10 uses
limited-range 10-bit BT.2020/PQ with validated mastering metadata. It rejects
unsupported HDR formats, variable frame rates, rotation, unsupported chroma placement, and nonzero
video/container start times. Dimensions must be even, from 64 through 8192 pixels,
and frame rates must be between 1 and 120 fps. Source and encoded frames are decoded for timing,
frame count, geometry, and color validation before the output is published. Frame
metadata is parsed and checked incrementally, so memory does not grow with video
length. Each frame's metadata is limited to 1 MiB, and each complete scan has a
24-hour execution limit. Scans use up to eight decoder threads, capped by the
available logical processors. Preparation and finalization show separate scan
progress. After observing progress for five seconds, the app estimates the current
phase's speed and remaining time; estimates reset between phases and disappear
when progress stops arriving. Cancellation stops and awaits the scanner's process tree.
Progressive video keeps its cadence unless frame processing is selected;
uniformly interlaced video requires explicit BWDIF deinterlacing. Selected
subtitles and attachments are copied by default. Audio remains copied unless
its per-track conversion setting is explicitly changed.

**Crop, resize, and borders:** Quick Convert and standalone Batch encode provide per-file
crop edges and an optional output width. Crop counts must be nonnegative even
pixels. Cropping happens before resizing (Lanczos by default); output height follows the
cropped aspect ratio and rounds to the nearest even pixel, with halfway values
rounded upward. The form displays source, cropped, and output dimensions.
Cropped and output dimensions must stay between 64 and 8192 pixels. A larger
explicit width enlarges the picture. Pixels remain square unless an output SAR or
DAR is selected after framing. These controls keep
the original duration and selected tracks, and retain supported SDR or HDR10
color metadata. Choosing the HDR encoder leaves dynamic-HDR fallback off.

Enable **Add black borders** to set each border in nonnegative even pixels.
Borders are added after crop and resize, keeping the picture at its planned
size. The resize width controls the picture before borders; the dimension
summary shows the final output including them. Final width and height must
remain within 8192 pixels. Borders use the source's supported color range and
bit depth, including static HDR10 with SVT-AV1. Turning borders off keeps the
entered draft values for later use but submits no borders.

Framing survives source/build/workflow draft changes and saved jobs; editing
batch framing requires a fresh preview. Old jobs without framing retain their
original dimensions, and older saved framing without borders adds none. Every source frame is checked at its original size and
every encoded frame at the planned output size before publication. The av1an
workflow also supports the same manual crop, resize, and black borders.
Nearest neighbor, bilinear and bicubic kernels are also available.
See the [crop and resize validation record](tests/fixtures/framing-validation.md).
See the [black border validation record](tests/fixtures/borders-validation.md).

**Frame processing:** BWDIF offers single-rate or bob deinterlacing for known
top-field-first or bottom-field-first sources. QTGMC offers single-rate or bob
reconstruction through a capability-checked private
VapourSynth, L-SMASH and havsfunc runtime. Inverse telecine uses field matching and
fixed 5:4 decimation, with an optional combed-frame BWDIF fallback. Progressive
padded captures can remove only byte-exact decoded duplicate frames after a
streaming SHA-256 scan reports repeated-run lengths and adjacent length transitions; the repair
is admitted only when its unique-frame count exactly preserves duration at the
selected lower constant rate. Lossy near-duplicates are not removed, while genuine
identical static pictures cannot be distinguished from padding, so this mode is for
known padded captures. An optional rational output rate duplicates or drops
pictures while retaining playback speed and the audio/subtitle timeline. Trim
precedes reconstruction and rate conversion. Every source frame must have
consistent timing and the selected field workflow must match decoded field flags;
mixed or unknown material fails explicitly. The selected BWDIF, telecine,
duplicate-removal and rate filters run on bounded synthetic frames before encoding.
Custom SAR or DAR metadata is applied after crop, resize and borders. See the
[frame processing validation record](tests/fixtures/temporal-validation.md).
For av1an, trim and any processing that changes frame count or timing first produce
one fully decoded, metadata-checked FFV1 source. Scene detection, all chunks and
quality references read that same source. Recovery binds its complete decoded-frame
identity, so an old chunk set cannot survive a changed QTGMC/plugin/filter result.

**Frame intervals:** Quick Convert, av1an and Batch encode can select either a
zero-based start frame/exclusive end frame or start/end times in milliseconds. Time
boundaries map to the exact constant-rate frame interval without accumulating
floating-point drift. The complete source still
passes the original frame and timing checks before the interval is applied.
Output starts at zero and retains exactly the selected video pictures. Every
selected audio track requires explicit conversion; boundaries follow its decoded
sample clock, including codec delay and final padding. Selected ASS, SubRip and
WebVTT cues and chapters are intersected with the interval and rebased; selected
text tracks remain present even when they contain no surviving cues. Fonts retain
their original bytes. A boundary that cuts an animated ASS cue is rejected,
as are timed WebVTT markup and bitmap subtitles. Saved jobs
without an interval continue to process the complete source. A gain calculated
from a loudness measurement uses the complete source track, including when the
job trims it. See the [trim validation record](tests/fixtures/trim-validation.md).

**Audio conversion:** Quick Convert, av1an, and Batch encode provide **Copy**, **Opus**, **AAC**,
**FLAC (24-bit)**, **MP3**, **Vorbis**, and **E-AC-3**
for each selected audio track. Copy is the initial setting and retains the source
audio. Lossy conversion starts at 128 kb/s and offers **Preserve source**,
**Mono**, or **Stereo** channel choices. The bitrate is the target for that track,
without hidden scaling by channel count. The upper limit adjusts to the codec,
sample rate, and channel count; mono Opus is limited to 256 kb/s. AAC, Opus, and
Vorbis offer 32–512 kb/s subject to those limits. MP3 uses standard bitrates,
up to 320 kb/s at 32–48 kHz or 160 kb/s at lower supported rates. E-AC-3 supports
32–6,144 kb/s at 48 kHz, with lower maxima at 32/44.1 kHz. FLAC has no bitrate
target and explicitly converts to 24-bit integer PCM. Deselecting a
track omits it from the output.

Opus uses FFmpeg's `libopus` encoder at 48 kHz. AAC uses FFmpeg's native AAC-LC
encoder at the supported source sample rate. Use a matching FFmpeg and FFprobe
pair from 8.1 or newer. A small synthetic preflight checks that both tools correctly
handle AAC priming before conversion starts. Older tools remain usable for copied
audio. Required encoders and source channel
layouts are checked before video encoding. MP3 requires mono or stereo. Preserve
rejects layouts that a codec would silently reinterpret or downmix; choose an
explicit channel conversion or another codec. Additional codecs exercise the
exact sample rate, bitrate, and channel plan on synthetic audio before processing
the source, including Matroska codec delay and final padding. Track order, titles, languages,
dispositions, chapters, subtitles, and attachments retain the selected mapping.
Obsolete encoder and bitrate statistics are removed from converted audio tracks.

The runtime decodes each converted source and output audio track, checks continuous
timestamps, and compares audible start times and sample counts after codec delay
has been applied. AAC may retain padding in its final 1024-sample frame; this does
not shift the beginning of the audio or any video timestamps. Copied audio retains
the existing copy validation. Conversion adds work to preparation and finalization,
and these checks remain cancellable. Bounded source timestamp rounding is measured
against its declared time base. Converted audio follows the validated sample clock
while preserving its first decoded timestamp; no samples are inserted or dropped
to hide a timing gap.

Each file in a batch owns its audio choices. Changing them requires a new batch
preview, and a queued job keeps its submitted settings. Older saved jobs without
audio settings still copy their selected audio. av1an applies audio conversion
during final muxing, after its video chunks finish, using the same codec-delay
and decoded-timeline checks.
See the [audio validation record](tests/fixtures/audio-validation.md),
[additional audio codec qualification](tests/fixtures/audio-codecs-validation.md),
and [av1an framing and audio validation](tests/fixtures/av1an-framing-audio-validation.md).

**Loudness and gain:** Measure a selected source track to see its integrated
loudness, true peak and loudness range. A target loudness proposes a flat gain;
Apply makes that value an explicit conversion setting. The source fingerprint
guards against applying a stale measurement, and gain must remain between -60
and +24 dB. This changes the level uniformly; it does not compress dynamics.
Trimming still uses the complete source-track measurement. See the
[loudness validation record](tests/fixtures/loudness-validation.md).

**Subtitles:** Standalone jobs can explicitly convert text to SubRip, ASS or
WebVTT, or burn a selected supported track into the picture. Bitmap graphics
use source coordinates before framing; text rendering follows crop/resize and
precedes borders. Font attachments are preserved when selected and supplied to
the renderer. Conversion and destination-container controls explain formatting
changes and reject content they cannot represent. See the
[subtitle validation record](tests/fixtures/subtitle-validation.md).

Open the separate **av1an** tab to use scene detection and parallel encoding with
1–32 workers (default2). Scene detection can use standard/fast analysis or fixed
chunks, configurable minimum/maximum lengths, analysis height and chunk order.
L-SMASH Works, FFMS2 and BestSource require their respective VapourSynth plugin.
FFmpeg select and hybrid readers require the corrected source-built av1an;
older generators fail exact frame-count checks. All paths require FFmpeg,
FFprobe and the selected SVT build. Jesses checks actual engine capabilities
and selected plugin availability. The manifest-verified Windows bundle currently
ships L-SMASH and reports FFMS2/BestSource unavailable; both optional readers have
passed a separate compatible external-runtime stop/reopen/resume gate, which is
not a claim that they ship in the bundle. av1an currently encodes the first video track only; standalone encoders
can encode another selected video track. Jobs remain sequential; workers run
chunks within the active job. More workers require more CPU and memory.

Quick Convert and av1an keep independent settings, copied-track selections, and
destinations for each source and encoder while the app remains open. Returning to a source
restores that workflow's draft; **Reset settings** resets only its current draft.
Default destinations identify the encoder/workflow, such as `_av1.mkv`,
`_av1_5fish.mkv`, `_av1_hdr.mkv`, `_x264.mkv`, or `_av1an_5fish.mkv`. Quick Convert
always starts the standalone encoder directly; the av1an tab always starts av1an.

av1an runs in a uniquely reserved workspace inside the output folder, with caches
kept there. Completed-chunk frame counts drive progress. The output passes the
same decoded-frame, track, and metadata checks as standalone encoding. Jesses
validates the concatenated IVF frame records and corrects its rate/count header
before muxing, covering av1an versions that write a fixed 30 fps header. Existing
destinations are never replaced. **Stop and keep progress** waits for the supervised
av1an worker tree to exit and retains its completed chunks. **Resume** in the job
history continues with the original saved settings, including the selected SVT
build. It works after restarting Jesses and never starts automatically. Source
preparation runs again to verify the source before completed work is reused.

Recovery checks source content, selected tool identities, encoder settings,
frame timing, saved commands and chunk fingerprints. Incompatible or damaged
work is rejected without deleting it. Chunks completed after the last durable
checkpoint are encoded again. The encoded video remains available if stopping
or crashing interrupts final muxing or output validation; resuming that phase
reuses the validated intermediate. A completed output is never overwritten.
Cancel and Stop queue also retain available av1an recovery work. Stopping before
recovery preparation finishes may leave no saved progress to resume.

Standalone executable encoders retain only complete, verified phase boundaries:
first-pass statistics, encoded video, an exact-cadence timing wrapper, or the final
validated mux stage. They never claim mid-frame continuation. Each durable receipt
binds the canonical job/output directory, source bytes, tool identities, settings,
processing plan, timing and artifact bytes. A stopped or interrupted job can be
resumed explicitly after restart; Jesses rechecks every binding and adopts a newer
complete manifest if a crash occurred between committing the phase and saving job
history. Unknown or changed workspace entries reject cleanup and preserve the tree
for review. Successful publication removes the owned recovery workspace.

Saved source commands and target probe parameters must match the immutable plan.
Qualified VapourSynth source templates are checked byte-for-byte after validated
path/cache/reader substitution. Hybrid source segments also undergo complete
pixel-sequence comparison with the original before reuse or publication. See the
[recovery validation record](tests/fixtures/av1an-recovery-validation.md) and
[scene/target validation](tests/fixtures/av1an-options-validation.md).

**Perceptual targets** choose per-chunk CRF using mean VMAF v0.6.1, SSIMULACRA2,
Butteraugli INF, or XPSNR minimum Y/U/V. Configure the ordered score range, CRF
bounds, evaluation resolution, probe count and frame sampling. Higher is better
for VMAF, SSIMULACRA2 and XPSNR; lower is better for Butteraugli. Changing the
metric resets its suggested score range. Unreachable ranges can finish outside
the target, and probe scores are retained in the engine detail log.

VMAF requires a working FFmpeg libvmaf model. SSIMULACRA2 needs vszip or Vship;
Butteraugli needs Julek with the corrected engine, or Vship. These two metrics
require a VapourSynth source reader. Every-frame XPSNR uses the selected FFmpeg;
sampled XPSNR needs vszip R7 or newer and a VapourSynth reader. Actual scorer
checks run in the same selected child environment before encoding. Windows
packaging builds CPU vszip and Julek from pinned sources alongside the portable
frameserver. A separate managed installer can activate the pinned Vulkan Vship
plugin only after real SSIMULACRA2 and Butteraugli checks; incompatible GPU,
Vulkan, or VapourSynth combinations keep the CPU scorer available.
L-SMASH-only scoring requires the corrected software-probe engine;
missing dependencies or older incompatible engines produce an explicit error.
VMAF and every-frame XPSNR also require the corrected FFmpeg metric engine, which
preserves the Y4M reference pixels during color-matrix negotiation. Probes
preserve the selected SVT build, preset and advanced parameters. Probe references
and final chunks use the same validated temporal, tone-map, crop, resize, border
and aspect-ratio filter chain.
Targeting requires SDR; preserved HDR keeps CRF control. Old saved targets with
no metric field retain VMAF.

Source files are never modified; caches stay in the
reserved output workspace, even when the output folder also contains the source.

**Film grain synthesis** accepts 0–50, default 0 (off). Nonzero values add AV1
grain synthesis; encoder denoising remains disabled. Synthesis does not reproduce
the source grain exactly. The same setting is available for SVT-AV1 in Quick
Convert, av1an, and batch encoding; no content preset silently enables grain.

Static HDR mastering and content light metadata are checked in the source and
decoded output, allowing only AV1's fixed-point precision difference. **Allow
HDR10 fallback** is off by default. Turning it on explicitly permits discarding
Dolby Vision enhancement data and HDR10+ dynamic metadata in favor of an HDR10
base layer. Supported Dolby Vision input is HEVC profile 7/compatibility 6 or
profile 8/compatibility 1; profile 5 and unrecognized profiles fail explicitly.
**HDR / HLG to SDR** is an explicit option in Quick Convert, av1an and each batch
file. It accepts tagged limited-range 10-bit 4:2:0 BT.2020 PQ or HLG, uses
Hable with a chosen signal peak (100–10000 nits), and produces 100-nit BT.709
limited-range 10-bit SDR. HLG uses the 1000-nit reference display transfer.
Tone mapping precedes subtitles and borders and removes source HDR metadata.
Compatible dynamic-HDR base layers require its separate opt-in; unselected
rendering retains the existing HDR10 workflow. All source and output frame
checks remain. av1an quality probes and final chunks receive the same validated
tone-map processing through either the shared filter chain or a verified prepared
source; recovery binds the corresponding filter or decoded identity before saved
work is reused. See the
[tone-map validation record](tests/fixtures/tone-map-validation.md).

Files shows reported pixel format, bit depth,
color tags, and HDR indicators; metadata absent from stream headers is labeled
as unreported because it may still exist on decoded frames.

A source declaring `24000/1001` fps may use timestamps authored at decimal
`23.976` (`2997/125`) fps. The encoder accepts that one alternative only when every
decoded frame fits the existing timestamp tolerance and all frame metadata checks
pass. The accepted cadence is used for decoding, encoding, and output validation;
the tolerance is not widened to accept gaps or variable frame rates.

Use **Start encode** for an idle workspace or **Add to queue** to submit an
immutable settings snapshot. Change the source or destination to add another job.
One job runs at a time, in submission order. **Cancel job** stops one job;
**Stop queue** cancels the active job and every waiting job. A failed job does not
prevent later queued jobs from running. Every input and destination is rechecked
when its job starts.

## Import a folder and prepare a batch

In Files, choose **Add folder** and optionally include subfolders. Discovery reads
regular files with supported media extensions, sorts the discovered paths, and
skips symbolic links and Windows reparse points. Errors and skipped entries remain
visible. A scan returns at most 500 media files after examining at most 10,000
entries; discovery times out after 30 seconds. A truncated scan should be retried
with a smaller folder. **Stop import** keeps completed imports and discards late
results; the current read-only scan or probe may finish in the background.

Open **Batch encode**, select up to 100 files, and review the video and copied
tracks for each file. Choose the workflow and encoder, then common workers,
CRF/preset and the encoder's supported grain/HDR10
fallback settings and an existing writable
output folder, then select **Preview batch**. The app proposes names such as
`episode_av1.mkv` and `episode_av1_2.mkv`, avoiding existing files and destinations
already reserved in the queue. x264, x265, and VP9 proposals use `_x264`, `_x265`, and `_vp9`. Unicode
names and spaces are retained where valid.
The preview creates no output files or folders.

The preview reports per-file selection and header compatibility errors. Each
FFprobe header inspection is limited to 30 seconds and 2 MiB of metadata. A ready
row still requires the full decoded-frame and output validation when its job runs.
Changing files, tracks, workflow, encoder, workers, quality, grain, HDR fallback, or the output folder requires a new
preview.

**Queue ready files** submits only the ready rows, with immutable per-file
settings, in the reviewed order. The entire submitted batch must pass admission
and fit the 100-record queue/history capacity before any new job is added. A
collision, invalid input, or history-write failure rejects the submission without
partially adding it. **Stop queue** also invalidates a batch submission whose
source checks are still in progress. Preview again before retrying a rejected
batch.

## Remux a file

Add a local file in Files, open Remux, choose the streams and their order, and
select a new destination. Keep attachments after video/audio/subtitle tracks;
containers without attachment support require deselecting them. At least one
video or audio track is required. FFmpeg and FFprobe must be available.
Media streams are copied; compatible text subtitles may require conversion for
the chosen container. Unsupported combinations fail explicitly. Combined mux
provides the same output checks with tracks from multiple imported sources and
explicit metadata/chapter ownership. See [mux qualification](tests/fixtures/mux-validation.md)
and [container qualification](tests/fixtures/container-validation.md).

Existing destinations are never replaced. The app writes a temporary sibling,
checks packet counts, selected stream properties, dispositions, metadata, chapters,
attachment hashes, and duration, then publishes the output without overwriting.
Publication requires a filesystem with hard-link support (for example NTFS);
unsupported filesystems fail and leave existing files unchanged. Verification scans
the source and output, so preparing/finalizing can take time for large files.

Cancel stops the owned process trees and removes temporary output. Closing the
app cancels and awaits active and queued jobs. The desktop app saves up to 100 job
records under its platform data directory. Jobs left unfinished after a crash are
shown as **Interrupted** on restart; they never resume automatically or signal old
process IDs. Eligible av1an and standalone executable jobs show **Resume** after a
complete verified recovery phase was created. Old jobs without recovery records
require a new encode. Recoverable jobs stay in history until successful completion rather than
being evicted when new jobs arrive. Job settings are retained for history; they
do not become defaults for new jobs.

History uses an exclusive instance lock and atomic file replacement. If history
cannot be read or saved, new jobs are blocked and the error remains visible;
corrupted history is preserved. Correct the problem and restart the app.
Job logs are stored under the platform app log directory; each supervised tool log retains
the newest two 4 MiB segments. Saved history contains only bounded log summaries.
av1an also writes its own detail log there. Cleanup failures are reported with
their paths; total disk-log retention remains
future work. Command-line examples use in-memory history.

## Inspect and analyze media

The encoding forms can decode a chosen source frame and propose crop edges from
sampled frames. A proposal changes the draft only after Apply. In Files, bitrate analysis counts every
selected video packet and charts bounded time windows, including explicit
accounting for packets without usable timestamps.

Quality analysis compares a matched frame interval from a reference and candidate
with SSIM, PSNR or VMAF. The selected source streams must pass complete timing,
geometry and SDR color checks. The chart retains every measured frame, including
infinite PSNR values for identical images. CSV and SVG exports preserve the
completed result, source fingerprints, interval and model settings; changing the
form does not change an export already being saved. Existing files are never
overwritten. See [quality qualification](tests/fixtures/quality-validation.md),
[bitrate qualification](tests/fixtures/bitrate-validation.md) and
[export qualification](tests/fixtures/analysis-export-validation.md).

Tools & settings saves a default output directory and up to 15 recent media
paths. Recent paths are checked when opened. A bounded compatibility import
previews supported general settings before Apply; it does not import executable
paths or encoder commands. See [preferences qualification](tests/fixtures/preferences-validation.md).

## Run locally

Install Node.js 24, pnpm 11.19.0, and the platform's
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/). Rust 1.98.1 is pinned
by `rust-toolchain.toml`. For media inspection, install FFmpeg with FFprobe and make
the executables available on PATH before starting jesses.

```sh
pnpm install --frozen-lockfile
pnpm desktop
```

`pnpm dev` starts a browser preview at `http://127.0.0.1:1420`. The browser has no
local media access; its optional sample is explicitly labeled synthetic data.

```sh
pnpm check
pnpm build
pnpm exec playwright install chromium
pnpm test
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
pnpm contracts:check
```

Run the real-tool integration tests separately with FFmpeg, FFprobe, and
standalone SvtAv1EncApp and x264 on PATH:

```sh
cargo test -p media-runtime --lib --locked -- --include-ignored
cargo test -p media-runtime --test real_tools --locked -- --include-ignored
cargo run -p media-runtime --example inspect
cargo run -p media-runtime --example inspect -- /path/to/video.mkv
cargo run -p media-runtime --example remux -- /path/to/video.mkv /path/to/new-output.mkv
cargo run -p media-runtime --example encode -- /path/to/video.mkv /path/to/encoded.mkv 30 4
cargo test -p media-runtime --test x264_jobs --locked -- --ignored --test-threads=1
cargo test -p media-runtime --test ffmpeg_video_jobs --locked -- --ignored --test-threads=1
cargo test -p media-runtime --test audio_jobs --locked -- --include-ignored
cargo run -p media-runtime --example encode -- /path/to/video.mkv /path/to/encoded.mkv 23 5 0 false standalone 2 x264
```

With av1an, VapourSynth and L-SMASH Works installed, run its integration gate:

```sh
cargo test -p media-runtime --test av1an_jobs --locked -- --ignored --test-threads=1
cargo test -p media-runtime --test av1an_recovery --locked -- --ignored --test-threads=1
cargo run -p media-runtime --example encode -- /path/to/video.mkv /path/to/encoded.mkv 30 4 0 false av1an 2
```

QTGMC gates intentionally fail when havsfunc or any required plugin is missing;
they are excluded from generic FFmpeg-only CI. With a compatible QTGMC runtime
configured beside av1an or with native VSPipe on PATH, run:

```sh
cargo test -p media-runtime --lib qtgmc_installed_runtime_executes_real_frames -- --ignored --nocapture
cargo test -p media-runtime --test temporal_jobs qtgmc -- --ignored --nocapture --test-threads=1
```

FFMS2 and BestSource are likewise optional. A runtime that reports either plugin
as found can qualify its actual recovery route with
`configured_source_readers_and_fixed_chunks_stop_reopen_and_resume`; an absent
plugin is an explicit dependency failure rather than a skipped success.

The encode example's optional arguments are CRF, preset, grain strength, explicit
HDR10 fallback (`true`/`false`), workflow (`standalone`/`av1an`), worker count, and
encoder (`svtAv1`/`svtAv1FiveFish`/`svtAv1Hdr`/`x264`/`x265`/`vp9`). The old `svtAv1` workflow name remains accepted by the
example and when loading older history. x264 integration tests require x264 on PATH.
For a complete immutable request including per-track audio settings, run
`cargo run -p media-runtime --example encode_request -- /path/to/request.json`.
See the upstream [av1an CLI reference](https://rust-av.github.io/Av1an/) and
[SVT-AV1 parameters](https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/v4.0.0/Docs/Parameters.md)
for the underlying tools.
See the [local av1an/HDR validation record](tests/fixtures/av1an-validation.md)
for tested media characteristics and remaining qualification limits.
See the [streaming validation record](tests/fixtures/streaming-validation.md)
for full-source scans, bounded-memory checks, and native UI qualification.

Build a native executable with embedded frontend assets:

```sh
pnpm tauri build --debug --no-bundle
```

Native CI targets Windows x64 and Linux x64. macOS support is deferred. Local
unsigned Windows artifact checks are recorded in the [packaging documentation](docs/packaging.md);
final artifact, signing, clean-machine and Linux runtime qualification remain separate gates.

## Development layout

- `src/`: Svelte interface and typed Tauri client.
- `src-tauri/`: desktop entry point and window command permissions.
- `crates/media-core/`: platform-independent metadata and error contracts.
- `crates/media-runtime/`: tool discovery, probing, process pipelines, validated
  output transactions, folder/batch preparation, and job history/queue.
- `tests/`: browser workflows and synthetic fixture recipes.

Rust owns media contracts. Run `pnpm contracts` after changing the DTOs and review
the generated TypeScript; CI rejects drift. Native commands launch known tools
directly with argument arrays, bounded output, and timeouts. Job supervision uses
atomic Job Object assignment on Windows 10+ and process groups on Unix. Unix tools
must not deliberately detach from their process group. Binary pipelines use
bounded buffers and owned file handles, require success from both stages, and
stop both trees on failure. Cross-platform native UI qualification remains a future
gate.

The activity panel retains at most 200 entries in memory. Only its open/closed
state persists; source lists and media metadata are session-only.

See [icon assets](assets/branding/README.md) for the master artwork and generated desktop formats.

See [third-party notices](THIRD_PARTY_NOTICES.md) and [license](LICENSE).
