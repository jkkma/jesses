# Source audio, rate, and subtitle contracts

Implemented against the checked-out Nmkoder fork in `~/repos/nmkoder/Nmkoder`, not a different upstream release. These are backend contracts; frontend source menus must continue using the original Jesses theme and the source's exact choices.

## Rate control

`Views/MainWindow.QuickConvert.cs:124–157` defines bitrate 10–100000, fresh 1500, and filesize 0.0–8192.0, fresh 50.0. `Utils/BitrateCalculation.cs:27–49` calls the latter MB but multiplies by `8 * 1024 * 1024`: its unit is binary MiB. Zero is representable in that source UI; it is not a meaningful successful encode budget.

The additive request is `{ mode: "targetSizeDecimal", targetSizeTenthsMb: 501 }` for 50.1 source MB. It preserves the exact integer tenth through serialization, uses integer arithmetic when converting to whole output bytes, and never rounds 50.1 to an integer MiB. The old `{ mode: "targetSize", targetSizeMib: 50 }` remains 50 × 1048576 bytes with its historical whole-MiB range. Both use the existing measured selected nonvideo payload and container reserve when deriving an estimated video bitrate. Zero, out-of-range tenths, insufficient remaining budget, unknown/nonpositive duration, and unusable computed bitrate fail before encoding. The source itself can pass zero through until a zero/negative bitrate or encoder error; no successful meaning has been invented for it.

`UI/Tasks/QuickConvert.cs:244` enables two passes only when the encoder supports them. The NVENC implementations (`Data/Codecs/Video/VideoEncodersLib.cs:122–140,159–176`) support target size as one-pass VBR. Native target size therefore resolves to one pass for NVENC and two for supported software encoders. NVENC's nominal bitrate, quarter-rate minimum, double-rate maximum, and buffer follow that source builder. Explicit external two-pass NVENC statistics remain invalid. Hardware execution was not qualified on this host.

## Numeric per-track audio

The source's numeric grid is 1–24 channels and 8–8192 kb/s (`Views/AudioStreamsWindow.axaml.cs:42–88`). Primary and external audio accept those numeric bounds. Codec restrictions are checked separately; a value admitted by the source grid does not imply the selected encoder can encode it.

Legacy channel strings remain unchanged. The additive channel request is:

```json
{ "explicit": { "count": 5, "layout": "ffmpegDefault" } }
```

Layout policies are `source` (same count and known source layout), `standard` (named mono/stereo/5.1/7.1), and `ffmpegDefault` (the source's `-ac N` behavior, qualified with the exact installed encoder). The global source dropdown still maps to the legacy Keep/Mono/Stereo/5.1/7.1 values. No new layout widget is needed for the numeric grid.

Before processing user audio, a private one-second source-layout fixture exercises the exact codec, sample rate, output count, bitrate, decode, and Matroska sample timing. Probed output count/layout must match the qualified FFmpeg default. Incorrect-layout warnings or bitrate clamping fail even if FFmpeg exits zero. A missing input layout can use FFmpeg's standard `Nc` default for a known 1–8-channel input, including the source global Keep behavior, only when the exact encoder probe preserves the expected layout. Known authored layouts remain authoritative. The explicit `source` policy requires a known authored multichannel layout.

FFmpeg 9.0.1 qualification confirmed:

| Source and request                               | Actual output / result                                                                 |
| ------------------------------------------------ | -------------------------------------------------------------------------------------- |
| Layoutless 6-channel PCM → AAC, numeric 3        | 3 channels, `2.1`                                                                      |
| Layoutless 6-channel PCM → AAC, numeric 4        | 4 channels, `4.0`                                                                      |
| Layoutless 6-channel PCM → FLAC, numeric 5       | 5 channels, `5.0`                                                                      |
| Layoutless 6-channel PCM → FLAC, numeric 7       | 7 channels, `6.1`                                                                      |
| Layoutless 6-channel PCM → Opus, Keep, 1024 kb/s | 6 channels, `5.1`, qualified without the old 512 kb/s ceiling                          |
| 6 channels → FLAC, numeric 3 or 4                | Rejected before output: FFmpeg warns the result would have an incorrect channel layout |

The implementation does not relabel numeric 3 as `3.0` merely to make FLAC succeed: this would change the source's actual `-ac 3` default (`2.1`). Unsupported counts and codecs fail explicitly. FFmpeg's AAC default layouts for 10, 12, 14, 16, and 24 were also inspected and rejected by the installed AAC encoder; counts without a default layout cannot be safely invented.

## Measured normalization

`Data/LoudnessConfig.cs:115–155` supplies the source behavior: channel conversion before measurement and execution, targets −14/−16/−23 LUFS, −1 dBTP, and second-pass LRA `clamp(ceil(measured LRA)+1,1,50)`. `UI/Tasks/QuickConvert.cs:294–306` measures the run's selected section.

Primary and external audio accept `normalization: { target: "lufs16" }` (also `lufs14`, `lufs23`); omission is off. The native job owns its input guard/fingerprint, original stream index, exact decoded sample-trim prefix, output layout, target, and all five first-pass values. It measures only after the same sample trim and channel conversion used by execution. The second pass supplies measured integrated loudness, peak, range, threshold, and target offset with `linear=true`. Peak risk is reported as a necessary reason for dynamic limiting, not a claim that FFmpeg necessarily chose linear mode. Separate flat gain and normalization cannot be combined; copy audio cannot normalize. Silent/nonfinite measurements, changed inputs, cancellation, and time bounds fail without publication.

The existing read-only flat-gain measurement API remains available for its original separate task. It is not used as an approximation of source normalization.

## Subtitle output and independent burn-in

`Data/CodecUtils.cs:47` includes MOV_TEXT. `UI/TrackList.cs:425–481,531–553` retains checked subtitles independently of burn-in. `SubtitleMode` therefore adds `movText`, and each primary subtitle accepts optional `burnIn: true` while external tracks accept optional `subtitleBurnIn: true`. The selected output mode still applies to that track. Legacy `mode: "burnIn"` remains render-only and removes the track from output; this also represents a source burn selection whose checkbox is off. At most one track may render.

MOV_TEXT uses a verified SubRip staging asset because Matroska cannot store tx3g; the existing final MP4/MOV writer encodes MOV_TEXT and verifies cue text/timing. Source conversion modes also work with copied video/audio-only jobs; burns require video encoding. ASS remains a legacy backend format, not a source dropdown option. Primary/external input scope, subtitles, copied packets, source identity, and final-container verification remain enforced.

## Validation

`crates/media-runtime/tests/source_audio_contract_jobs.rs` contains four opt-in real-tool tests covering exact fractional size plus source zero/bounds in unit tests; converted numeric 3/4/5/7 and rejected incorrect layouts; layoutless PCM 5.1 over a six-second trimmed span downmixed before measured −16 LUFS normalization; external-track true-peak limiting (measured −1.00 dBTP, with this deliberately peak-heavy synthetic output at −17.04 LUFS despite its −14 target); retained burned MOV_TEXT tracks; and copied-video MOV_TEXT→SubRip conversion. Inputs are hash-checked and failed conversions leave no destination. Existing audio sample/timing, subtitles, loudness, rate-control, and recovery tests provide regression checks.
