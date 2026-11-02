# Nmkoder fork dialog inventory

This records the current UI in `~/repos/nmkoder` for dialogs and utility launch paths that sit outside the main tabs. The main-tab and global-setting controls are inventoried separately in `docs/nmkoder-ui-parity.md`. Current `.axaml`, code-behind, and task/config code are authoritative; screenshots under `nmkoder/docs/images` are illustrative and may show older controls.

## Coverage

`Nmkoder/Views` contains 19 XAML windows: `MainWindow` plus the 18 auxiliary windows below. The inventory includes all 18 auxiliary windows, all utility cards, their configuration entry points, and the task dispatch. It does not treat native OS file pickers as Avalonia windows.

| Screen                   | Role                                                    |
| ------------------------ | ------------------------------------------------------- |
| `AudioStreamsWindow`     | Quick Convert per-track audio configuration             |
| `Av1anResumeWindow`      | Resume or delete unfinished AV1AN work                  |
| `BitratePlotWindow`      | Bitrate chart result                                    |
| `ColorDataWindow`        | Color/HDR metadata utility configuration                |
| `CrfLadderResultsWindow` | Sample Encodes result table                             |
| `CrfLadderWindow`        | Sample Encodes configuration                            |
| `CropWindow`             | Crop configuration for either encode tab                |
| `CutWindow`              | Quick Convert trim, AV1AN trim, or standalone Cut Video |
| `DeinterlaceWindow`      | Deinterlace Video utility configuration                 |
| `EditCommandWindow`      | Optional command review/edit window                     |
| `EncoderArgInfoWindow`   | Extended encoder-argument help                          |
| `FileImportWindow`       | File-drop import selection                              |
| `FilmGrainWindow`        | Film Grain utility configuration                        |
| `MessageWindow`          | App-owned message/choice dialog                         |
| `MetricsWindow`          | Get Metrics configuration                               |
| `PromptWindow`           | Generic one-line prompt                                 |
| `ResetSettingsWindow`    | Settings subdialog for reset-on-new-file choices        |
| `ResizeWindow`           | Resize configuration for either encode tab              |

## Utilities tab and launch paths

The Utilities screen has 11 cards. Selecting a card chooses the task read by the main Run button. The Configure buttons select the same utility before opening its window; Get Metrics also opens its window when its card body is tapped. Cut Video opens its section editor on first card selection when no section has been configured. The task dispatcher maps the selected task to the corresponding `Util*.Run()` method.

| Card label                  | Configuration surface                            | Run task                |
| --------------------------- | ------------------------------------------------ | ----------------------- |
| Read Bitrates               | None                                             | `UtilReadBitrates.Run`  |
| Get Metrics                 | `MetricsWindow`                                  | `UtilGetMetrics.Run`    |
| Transfer Color Metadata     | `ColorDataWindow`                                | `UtilColorData.Run`     |
| Deinterlace Video           | `DeinterlaceWindow`                              | `UtilDeinterlace.Run`   |
| Repair Frame Cadence        | None                                             | `UtilRepairCadence.Run` |
| Film Grain (AV1)            | `FilmGrainWindow`                                | `UtilFilmGrain.Run`     |
| Sample Encodes              | `CrfLadderWindow`, then `CrfLadderResultsWindow` | `UtilCrfLadder.Run`     |
| Cut Video                   | `CutWindow`                                      | `UtilCut.Run`           |
| Concatenate Into Single MKV | None                                             | `UtilConcat.Run`        |
| Show Bitrate Chart          | None; chart opens after analysis                 | `UtilPlotBitrate.Run`   |
| OCR Bitmap Subtitles        | None                                             | `UtilOcr.Run`           |

Evidence: `Nmkoder/Views/MainWindow.axaml:814,823,833,843,854,864,875,886,897,906,915`; card selection/config routing in `Nmkoder/Views/MainWindow.Utils.cs:23,32,49,60,83,118,125,142,153,177,195`; Run-button dispatch in `Nmkoder/Views/MainWindow.axaml.cs:297` and `Nmkoder/Main/RunTask.cs:356,357,358,359,360,361,362,363,364,365,366`.

Among these utility dialogs, Film Grain is the only one with a file browser: it picks an existing grain-table input. Metrics and Color Metadata choose from files already in the file list; the other dialogs take no file path. None of these dialogs offers an output-path picker. Output paths are automatic or the utility only reports a result.

## Configuration dialogs

### Crop

**Title and layout:** `Configure Crop`, fixed 460 px width, height to content, centered on owner, not resizable. A no-video notice appears when no dimensions are available. The groups are `PIXELS TO CROP OFF EACH SIDE` and `RESULTING CROP AREA`; controls are Left, Right, Top, Bottom, a live result label, Reset, and default OK.

**Controls and defaults:** Four integer spinners start at 0 and step by 2. On open they use the supplied saved crop if present, otherwise all zero. With a video, left/right totals are constrained to leave at least 2 pixels of width and top/bottom totals leave at least 2 pixels of height; the dialog clamps an old saved crop proportionally if a smaller source is now loaded. With no video, each edge is allowed through 16,384 and the readout reports the four edge values instead of a resulting rectangle. A fitting crop readout reports output size/offset, even alignment, and whether saved values were reduced for this source. Reset returns all four values to zero.

**Apply/launch:** Quick Convert and AV1AN each pass their active source dimensions and current crop, then replace their saved crop only when OK returns a value. Dismissal returns null and leaves the caller's setting unchanged. No output picker.

Evidence: `Nmkoder/Views/CropWindow.axaml:4,5,11,16,20,22,25,27,30,31,34,35`; `Nmkoder/Views/CropWindow.axaml.cs:24,39,43,46,49,54,62,64,168,190,198,211`; `Nmkoder/Data/CropConfig.cs:33,37,39`; Quick Convert route `Nmkoder/Views/MainWindow.QuickConvert.cs:265,269,271,273`; AV1AN route `Nmkoder/Views/MainWindow.Av1an.cs:201,203,208,210`.

### Resize

**Title and layout:** `Configure Resize`, fixed 580 px width, height to content, centered on owner, not resizable. Groups are Source, Target, Options, Result. No-video notice appears when there is no source size. Reset and default OK are at the bottom.

**Mode options:** `Fit inside a box (keeps the aspect ratio)`, `Set the height (width follows)`, `Set the width (height follows)`, `Scale by a percentage`, `Exact size`. `Disabled` is not offered in the dialog; the main tab's resize dropdown turns resizing off. With no saved setting, mode is Fit and the target box is the source's display size rounded to an even width/height. Saved Disabled is presented as Fit.

**Mode-dependent rows:** Fit and Exact show two size spinners (W/H, min 16, max 16,384, step 2, XAML placeholder values 1920×1080). Height/Width show one spinner (min 16, max 16,384, step 2, placeholder 1080) and switch its label. Percent shows a spinner (1–800, step 5, placeholder 50). Exact alone shows `If it doesn't fit`: `Letterbox with black bars`, `Crop off the overflow`, `Stretch to fit` (default Pad/first choice for a new config).

**Other options/defaults:** `Round sizes to` offers `2 (default)`, `4`, `8`, `16` (default 2). `Resampler` offers `Bicubic (default)`, `Lanczos`, `Spline`, `Bilinear`, `Area`, `Nearest neighbor` (default bicubic/empty FFmpeg flag). `Allow upscaling past the source's own size` defaults off. `Correct non-square pixels` defaults on but is disabled for square-pixel sources. A new config otherwise inherits ResizeConfig defaults: target 1920×1080, 50%, Pad, modulus 2, upscaling off, aspect correction on, bicubic. In the dialog's first-use case, Fit target dimensions are replaced with the source display dimensions.

`Allow upscaling` is disabled for Percent mode and for Exact when fill is Crop or Stretch. It remains enabled for Fit, Height, Width, and Exact/Pad. The result is recalculated live. With no source it describes the target and says size is worked out per file at encode time. Reset reloads source-derived defaults; OK returns the config; dismissing returns null and leaves the caller's setting unchanged. There is no output picker.

Evidence: `Nmkoder/Views/ResizeWindow.axaml:4,5,11,16,19,22,23,26,29,31,35,38,43,46,51,53,54,55,56,60,63,65,67,68,69,70,72,76,78,81,82,83,87,88,90,91,92`; dynamic lists/default source in `Nmkoder/Views/ResizeWindow.axaml.cs:20,24,34,46,61,66,70,71,73,82,85,87,88,92,94,96,104,111,112`; row visibility and enabling in `Nmkoder/Views/ResizeWindow.axaml.cs:130,137,138,139,140,142,143,149,150`; defaults in `Nmkoder/Data/ResizeConfig.cs:43,46,47,49,50,53,60,63,66`; result/reset/confirm in `Nmkoder/Views/ResizeWindow.axaml.cs:207,223,228`.

### Cut Video and trim editor

The same `CutWindow` configures three purposes: Quick Convert trim, AV1AN trim, and the standalone lossless Cut Video utility. It is fixed 720 px wide, height to content, centered on owner, not resizable. The title is `Cut Video` only for standalone cut; otherwise `Configure Trim`.

**Groups and controls:** 300 px preview, position slider, kept-range bar and playhead; position readout; jump-to-start/end buttons; ±10 s, ±1 s, ±1 frame, +1 frame, +1 s, +10 s nudges; Start and End text boxes with `Set to playhead`; read-only Duration; optional keyframe note and optional `Snap start to keyframe`; Reset and default OK. The preview is unavailable for image-sequence folders and is replaced by a no-preview/no-file message where needed. Slider is enabled only for a positive duration; positions are clamped to the source duration.

Start/End default to the entire source (0 to duration) when no section is saved, otherwise the saved range is restored. In time mode, accepted input is `HH:MM:SS`, `HH:MM:SS.mmm`, or plain integer seconds; in frame mode, nonnegative frame-number strings are required. Duration is recomputed from the range. Reset restores the full source and puts the playhead at zero.

**Mode and keyframe visibility:** `Mode` is shown only for Quick Convert trim when the selected video codec re-encodes. Its exact choices are `Time (exact - seeks straight to the point)`, `Time (exact - decodes from the start, much slower)`, and `Frame numbers (exact - decodes from the start, much slower)`. Default is saved mode or `Time (exact - seeks straight to the point)`. AV1AN trim and standalone Cut always copy, and Quick Convert copy mode also copies; those cases hide the mode row and use time/keyframe mode. Copy cases automatically move the start back to the nearest keyframe at or before it, and hide the snap button. In re-encode mode the keyframe is irrelevant; the button remains visible but disabled, and the three seek modes remain exact.

Invalid times or a reversed range keep the dialog open and raise an error message. Confirming an empty range returns null. Dismissing returns the prior saved trim. The standalone utility writes an automatically named `_cut` file using the app's default output path; no output picker. AV1AN first extracts the section as a stream copy; Quick Convert uses it as an encode trim.

Evidence: `Nmkoder/Views/CutWindow.axaml:4,5,11,12,19,23,24,29,34,35,37,38,39,40,41,42,43,44,48,49,53,55,56,57,60,61,62,64,65,66,68,69,70,73,74,76,77,78`; purpose and mode setup in `Nmkoder/Views/CutWindow.axaml.cs:31,77,84,90,95,125,141,145,146,148,150,151,152,153,154,155,156,160,164,165,176,184,186,187`; validation/result in `Nmkoder/Views/CutWindow.axaml.cs:304,309,319,330,335,340,344,350,358`; keyframe state in `Nmkoder/Views/CutWindow.axaml.cs:553,562,580,585,653,679,693,716,732,748`; task output in `Nmkoder/UI/Tasks/UtilCut.cs:47,61,62,64`; Quick Convert launch in `Nmkoder/Views/MainWindow.QuickConvert.cs:280,284,285`; AV1AN launch in `Nmkoder/Views/MainWindow.Av1an.cs:263,265`; utility launch in `Nmkoder/Views/MainWindow.Utils.cs:142,147,149,153,155,159,161,167`.

### Film Grain (AV1)

**Title and groups:** `Configure Film Grain (AV1)`, fixed 660 px width, height to content, centered on owner, not resizable. A note explains the utility is separate from the AV1AN encode row. Groups are Source, Operation, conditional operation settings, Result, and Warning. Reset and default OK appear at the bottom.

**Operation menu:** `Measure a grain table from this file`, `Extract the grain table it already has`, `Apply grain to this file`, `Remove all grain from this file`. First-use/default operation is Measure. Measure shows `Denoise strength` (1–16, step 1, default 4) and `Keep the denoised video too` (off). Apply shows `Grain from`, with `Grain table file`, `Film stock preset`, `Photon noise (ISO)`; new/default source is Grain table file. Table mode shows a path field and `…` picker (`Pick a film grain table`, single file). Preset mode shows the preset dropdown. Photon mode shows ISO (1–409,600, step 100, default 400) and `Chroma` (off). Irrelevant operation/source groups are hidden rather than disabled.

Preset choices are loaded from the installed grav1synth `presets` command at startup when available. Fallback values, in order, are `Super8`, `MaxMid`, `16mm`, `16mm-1`, `16mm-2`, `16mm-3`, `Classic35`, `Classic35-1`, `Classic35-2`, `Classic35-3`, `Modern35`, `Modern35-1`, `Modern35-2`, `Modern35-3`. The exact live list is therefore runtime-dependent. Grain-table path, preset, ISO, chroma, denoise, and operation are persisted utility settings; first-run defaults are Measure, denoise 4, do not keep denoised video, Table source, empty table path, ISO 400, Chroma off. Reset restores those defaults. Controls update the active in-memory utility settings as they change; OK saves them for next launch. The X close is not a rollback. Output filenames are automatic beside the source/session; no output picker.

Evidence: `Nmkoder/Views/FilmGrainWindow.axaml:4,5,11,16,19,21,22,23,27,28,30,32,38,39,41,43,44,45,48,51,52,54,60,61,62,64,65,66`; operation labels and dynamic source labels in `Nmkoder/Views/FilmGrainWindow.axaml.cs:32,53,60,61,62,63,66,75,78,79,80,86,90,91,92,93,97,101,102,103`; visibility/browse/persist in `Nmkoder/Views/FilmGrainWindow.axaml.cs:114,116,127,148,149,150,151,152,229,234`; settings defaults and persistence in `Nmkoder/UI/Tasks/UtilFilmGrain.cs:44,48,58,65,67,71,74,78,80,91,100`; preset fallback and startup replacement in `Nmkoder/Data/GrainSynthConfig.cs:143,146,149,153,164,165,166,167,168,169,170,172` and `Nmkoder/Views/MainWindow.axaml.cs:97,103`; output behavior in `Nmkoder/UI/Tasks/UtilFilmGrain.cs:209,213,218,219`.

### Sample Encodes configuration

**Title and groups:** `Configure Sample Encodes`, fixed 730 px width, height to content, centered on owner, not resizable. Source and Encoder groups, then Ladder, then live Result. Reset and default OK. It can be configured without a loaded file; the Source/Result text says settings are saved and will apply to the file loaded when Run occurs.

**Encoder options:** seven FFmpeg library encoders, in dropdown order: `AV1 (SVT-AV1)`, `AV1 (AOM-AV1)`, `H.265 / HEVC (x265)`, `H.264 / AVC (x264)`, `VP9 (VPX)`, `H.265 / HEVC (NVIDIA NVENC)`, `H.264 / AVC (NVIDIA NVENC)`. Default is SVT-AV1. Selecting a different encoder resets speed preset, pixel format, and content preset to that encoder's own defaults. Preset and Color format are populated from that encoder's `IEncoder` lists. `Content preset` is shown only for SVT-AV1, x264, and x265; its options are `None` plus the encoder's content presets: SVT-AV1 has `Anime / Cel Animation`, `Game Capture / Gameplay`; x264 has `Grainy Film / Analogue Transfer`, `Anime / Cel Animation`; x265 has `Grainy Film / 35mm Scan`, `Anime / Cel Animation`. Content preset defaults to None. Preset, pixel-format labels, and their defaults come from the source encoder class; current defaults are `SVT-AV1` preset `4`, `yuv420p10le`; `AOM-AV1` preset `6`, `yuv420p10le`; x265 `medium`, `yuv420p`; x264 `slow`, `yuv420p`; VPX `3`, `yuv420p`; H.265 NVENC `p7`, `yuv420p`; H.264 NVENC `p5`, `yuv420p`.

**Ladder options/defaults:** `CRF values` is a free-text list, empty by default. Empty means three values around the selected encoder's default CRF. Input splits on comma, semicolon, spaces, tabs, or `/`; unparsable tokens are ignored, values clamp to the encoder's quality range, duplicates are removed, values sorted, and no more than 8 are run. Default ladders by encoder are x264 14/18/22, x265 18/22/26, H.264 NVENC 14/18/22, H.265 NVENC 18/22/26, VPX 19/24/29, SVT-AV1 25/30/35, AOM-AV1 15/20/25.

`Samples` is sample count (1–8, default 3) and seconds per sample (2–120, default 10). `Score with` choices, in order, are `VMAF`, `SSIMULACRA2`, `Butteraugli`, `XPSNR`, `Nothing (size only)`; default VMAF. `VMAF Model` choices are `vmaf_v0.6.1 (Default)`, `vmaf_v0.6.1neg (No Enhancement Gain)`, `vmaf_4k_v0.6.1 (4K)`; first is default, and the model control is disabled unless Score with is VMAF. `Keep the sample encodes beside the source` defaults off. No output picker; when keeping samples is enabled, clips/encodes are written beside the source.

These are utility-owned settings and do not read or modify either encode tab. Settings load from app config, controls update the in-memory values, and OK saves them. X close does not roll back current-session edits. Reset returns the defaults above. The result window appears after a run.

Evidence: controls in `Nmkoder/Views/CrfLadderWindow.axaml:4,5,11,16,19,22,23,26,28,32,33,36,38,39,42,45,47,48,52,54,57,61,63,65,66,67,68,73`; encoder order/defaults and dynamic lists in `Nmkoder/UI/Tasks/UtilCrfLadder.cs:46,48,49,50,51,52,53,54,57,62,66,71,75,77,78,79,80,89,91,98,105,108,115,139,143`; encoder/preset/pixel data in `Nmkoder/Data/Codecs/Video/VideoEncodersLib.cs:18,19,20,22,23,24,25,26,55,56,57,58,59,60,61,62,111,112,113,114,115,116,117,118,148,149,150,151,152,153,154,155,187,188,189,190,191,192,193,194,195,229,230,234,235,236,237,238,239,305,306,307,308,309,310,311,312`; content preset labels and mappings in `Nmkoder/Data/Codecs/EncoderArgPresets.cs:49,53,55,56,90,92,116,152,154,189,223,225,267`; ladder parse/default formula in `Nmkoder/Data/CrfLadder.cs:243,248,249,253,255,260,267,269,272,273,275`; UI mode, model enable and readout in `Nmkoder/Views/CrfLadderWindow.axaml.cs:48,55,60,63,67,70,76,78,81,93,127,134,139,140,141,168,175,176,181,191,192,193,194,195,196,200,222,227`; save/load in `Nmkoder/UI/Tasks/UtilCrfLadder.cs:188,190,191,192,193,194,195,196,197,198,199,201,202,205,216`; Run/result route `Nmkoder/UI/Tasks/UtilCrfLadder.cs:439`.

### Get Metrics

**Title and groups:** `Quality Metrics`, fixed 560 px width, height to content, centered on owner, not resizable. Groups: `Encoded (Low Quality)` file selector, `Reference (Source)` selector, METRICS checkboxes, then Alignment/Subsampling/VMAF Model, and OK. No Reset button.

Both selectors contain loaded file-list entries, not file-browser results. Previously selected file paths are restored if still present; if either side is absent/invalid, the smallest file is guessed as Encoded and the largest as Reference. The utility's initial defaults are VMAF on, SSIM off, PSNR off, no alignment (`None`), VMAF model first, and subsampling shown as `1 (Every frame)`. Subsampling choices are `1 (Every frame)` through `32 (Every 32th frame)` (the source labels each as `Every nth frame`); the legacy initial value 0 is clamped to display 1 and is saved as 1 on confirmation. Alignment choices are `None`, `Auto-Crop reference`, `Resize reference`, `Auto-Crop and resize reference`. VMAF models are the same three listed for Sample Encodes.

The Configure button and Get Metrics card body both select the utility and open the window. Fewer than two loaded files logs a requirement and does not open it. OK writes the selections to utility runtime settings; dismissing makes no change. There is no output picker or output file.

Evidence: `Nmkoder/Views/MetricsWindow.axaml:4,5,11,12,13,14,15,18,20,21,22,25,26,27,28,29,30,31,34,35,37,38,39,40,41,45,46`; file/subsample/default handling in `Nmkoder/Views/MetricsWindow.axaml.cs:47,49,55,57,58,59,60,61,62,64,65,67,70,74,75,95,97,98,99,100,101,102,111`; defaults in `Nmkoder/UI/Tasks/UtilGetMetrics.cs:19,20,21,22,24,25,26`; launch guard in `Nmkoder/Views/MainWindow.Utils.cs:83,88,89,125,127,128,131,133,135,139`.

### Deinterlace Video

**Title and groups:** `Configure Deinterlace Video`, fixed 620 px width, height to content, centered on owner, not resizable. A note explains this is utility-owned. Groups are Source, Deinterlacer, conditional QTGMC preset, Frame rate, Result, output note; Reset and default OK.

Mode options and order: `Automatic`, `Disabled`, `QTGMC (VapourSynth)`, `Bwdif (FFmpeg)`, `Yadif (FFmpeg)`. The utility default is QTGMC, not Automatic. `QTGMC preset` options are `Placebo`, `Very Slow`, `Slower`, `Slow`, `Medium`, `Fast`, `Faster`, `Very Fast`; default is `Very Slow`. `One frame per field` defaults checked. Preset row is hidden for Disabled/Bwdif/Yadif and shown for QTGMC/Automatic. The frame-rate checkbox is disabled only for Disabled mode.

Source readout says no input is loaded if absent; otherwise it shows file name and current scan verdict. Result text is computed from the selected mode/preset/rate, and updates when the QTGMC capability probe finishes. This setting is separate from both encode tabs, loads/saves in app config, and edits the live utility setting as changed; X close does not roll back current-session edits. Reset returns QTGMC / Very Slow / one frame per field. Output note is fixed: writes a near-lossless x264 CRF 12 MKV beside the source, copies audio/subtitles, and does not load the output back into the file list. No output picker.

Evidence: `Nmkoder/Views/DeinterlaceWindow.axaml:4,5,11,16,19,21,22,23,27,28,30,31,32,36,37,38,42,43,44,45,47,48,49`; mode labels/order in `Nmkoder/UI/Tasks/DeinterlaceUi.cs:58,59,311,318,319,320,321,322`; preset values/default in `Nmkoder/Media/Qtgmc.cs:39,45`; utility default and persisted settings in `Nmkoder/UI/Tasks/UtilDeinterlace.cs:48,50,53,56,58,60,61,62,71,75`; display/enable rules and probe in `Nmkoder/Views/DeinterlaceWindow.axaml.cs:50,55,56,60,62,65,68,77,80,94,95,96,99,100,104,105`; output in `Nmkoder/UI/Tasks/UtilDeinterlace.cs:125,127,140`.

### Quick Convert audio tracks

**Title and layout:** `Configure Audio Tracks`, initially 760×420, centered on owner. A one-line instruction sits above a data grid. Columns are read-only Track, Title, Lang; editable Channels and Bitrate (Kbps). OK is the only action. No output picker.

The grid has one row per audio stream in the current track list. Labels are `#1`, `#2`, etc.; blank title becomes `None`, otherwise title is truncated to 35 characters; language is uppercased. Existing per-file settings are restored when they match the current source path and size. Otherwise the initial channel count uses the audio tab's override if greater than zero, or each source track's own channel count; initial bitrate is the selected base bitrate multiplied for that channel count. On confirmation, channels clamp to 1–24 and bitrate to 8–8,192 Kbps. Closing without OK returns null and the caller returns the mode to its non-per-track setting when there is no saved configuration.

Selecting the audio tab's per-track configuration mode opens this dialog automatically. It is only accepted when a file is loaded, the file has at least one audio track, and the selected audio encoder supports custom bitrates; failed checks show a message and restore the previous mode. The caller passes the current file, base bitrate, and selected channel override.

Evidence: `Nmkoder/Views/AudioStreamsWindow.axaml:4,5,9,10,11,13,14,16,17,18,19,20,24,25`; row data and clamping in `Nmkoder/Views/AudioStreamsWindow.axaml.cs:42,57,59,64,66,72,73,74,76,77,78,80,84,86,87,88`; saved file association in `Nmkoder/Data/AudioConfiguration.cs:15,21,26,29,32,36,38,39,40`; entry, preconditions, and dismissal behavior in `Nmkoder/Views/MainWindow.QuickConvert.cs:308,310,313,314,316,319,322,328,333,335,341,344,346,348,350,358,360`.

### Transfer Color Metadata

**Title and groups:** `Transfer Color Metadata`, fixed 560 px width, height to content, centered on owner, not resizable. Controls: Source Video selector, Target Video selector, target behavior note, DATA TO TRANSFER, `Color space (transfer, primaries, matrix, range)`, `HDR metadata (mastering display, MaxCLL/MaxFALL)`, optional batch-mode note, and OK.

In normal Mux mode, source and target choices are loaded file-list entries. A non-file first target option is `(none - only read the source's metadata)`, which is the initial target if there is no valid saved target; source defaults to the largest loaded file if there is no valid saved source. Both copy checkboxes default on. Picking a target causes it to be rewritten in place; no target means read/log source metadata only. There is no file browser or output-path picker.

In Batch mode, only the current file is offered as source, Target Video is disabled, the normal target note is hidden, and the batch note says transfer requires Muxing Mode. The utility's Configure button requires at least one file in the list. Run silently reapplies stored values; with no target it only reports metadata. Current values are process-level static settings; this class does not save them to app config.

Evidence: controls in `Nmkoder/Views/ColorDataWindow.axaml:4,5,11,12,13,14,15,18,19,21,22,23,25,26,28,29`; defaults in `Nmkoder/UI/Tasks/UtilColorData.cs:16,17,18,19`; dynamic mode/list/default behavior in `Nmkoder/Views/ColorDataWindow.axaml.cs:17,30,35,37,40,52,54,55,59,60,62,65,67,72,74,76,78,81,84,88,92,93,96,100,101`; no-target write behavior in `Nmkoder/Views/ColorDataWindow.axaml.cs:119,121,122,124,127`; Run route in `Nmkoder/UI/Tasks/UtilColorData.cs:21,27,39,41,42,45,51,64,68,71,74`; launch guard in `Nmkoder/Views/MainWindow.Utils.cs:177,179,181,183,187`.

### Reset Settings On New File

This separate `Settings To Reset On New File` window is initially 420×400, centered on owner. It contains an instruction, a checkbox list, and OK. The eight names are generated from `Reset*` properties and displayed as `Trim`, `Frame Rate`, `Resize`, `Crop`, `Borders`, `Deinterlace`, `Tone Mapping`, `Custom Filters`. On first run, Trim, Crop, Deinterlace, and Tone Mapping are on; Frame Rate, Resize, Borders, and Custom Filters are off. Existing saved checkbox values override these defaults. OK writes all eight values; dismissing makes no change. Main Settings also has `Reset Now`, which calls the reset action directly outside this dialog.

Evidence: `Nmkoder/Views/ResetSettingsWindow.axaml:4,5,8,9,10,12,15,20,21`; list generation/save in `Nmkoder/Views/ResetSettingsWindow.axaml.cs:26,37,39,41,44,48,50,51,53`; names and order in `Nmkoder/UI/ResetSettingsOnNewFile.cs:10,11,12,13,14,15,16,17,19,24,25,26,27,28,29,30,31`; default-on list and load behavior in `Nmkoder/UI/ResetSettingsOnNewFile.cs:90,101,124,127`; Settings launch and Reset Now in `Nmkoder/Views/MainWindow.Settings.cs:84,86,95,97`.

## Other auxiliary windows

These do not configure utility settings, but are in the source's complete non-main window set and need counterparts where Jesses reaches the same action.

| Window                   | Controls and behavior                                                                                                                                                                                                                                                                                                                                                  | Evidence                                                                                                                                                                                                                         |
| ------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CrfLadderResultsWindow` | `CRF LADDER` header and dynamic subtitle; read-only, sortable-off table with CRF, Video bitrate, Per minute, Whole file, Of source, VMAF/dynamic score name, Encode time. The Of source column is hidden without source-byte data; score column is hidden for Nothing. Note/recommendation is dynamic. `Copy` exports a tab-separated text table; `Close` is default.  | `Nmkoder/Views/CrfLadderResultsWindow.axaml:5,6,11,12,16,17,20,21,22,23,24,25,26,27,31,34,36`; `Nmkoder/Views/CrfLadderResultsWindow.axaml.cs:38,59,65,70,71,75,77,78,83,106,174,186,195`                                        |
| `BitratePlotWindow`      | 1000×560 chart window; chart is the analyzed bitrate series. Drag to select a range; double-click resets. Info text changes with selection.                                                                                                                                                                                                                            | `Nmkoder/Views/BitratePlotWindow.axaml:5,6,9,13,15,16`; `Nmkoder/Views/BitratePlotWindow.axaml.cs:16,18,21,24`                                                                                                                   |
| `Av1anResumeWindow`      | 820×420 resumable-folder list; initially selects first entry or none. Buttons: `Delete Encode`, `Delete Chunks`, `Resume (New Settings)`, `Resume (Saved Settings)` (default). Deletes act immediately without a confirmation window; full delete removes folder and side JSON, chunk delete clears encode chunks and progress records.                                | `Nmkoder/Views/Av1anResumeWindow.axaml:4,5,8,10,12,14,15,16,17,18`; `Nmkoder/Views/Av1anResumeWindow.axaml.cs:50,54,57,66,73,80,87,88,98,105,107,115,117`                                                                        |
| `EditCommandWindow`      | 900×380; title becomes `Edit ffmpeg Arguments` or `Edit av1an Arguments`. Shows dynamic command in a multiline wrapping text box; `Run` returns trimmed text. It opens only when Shift is held during encode Run; an empty/closed result cancels the task.                                                                                                             | `Nmkoder/Views/EditCommandWindow.axaml:4,5,8,10,12,14,15`; `Nmkoder/Views/EditCommandWindow.axaml.cs:18,21,22,34,36`; callers `Nmkoder/UI/Tasks/QuickConvert.cs:402,404,406,408` and `Nmkoder/UI/Tasks/Av1an.cs:734,736,738,743` |
| `EncoderArgInfoWindow`   | 720×560; dynamic argument name, one-line summary, long-form details, and examples. Empty details show an explicit “not written yet” message; Examples header/list are hidden when there are no parsed examples. Close is default/cancel. Opened by right-clicking an encoder argument row.                                                                             | `Nmkoder/Views/EncoderArgInfoWindow.axaml:4,5,8,9,15,17,19,21,31,52,53`; `Nmkoder/Views/EncoderArgInfoWindow.axaml.cs:33,49,51,54,56,57,60,61`; caller `Nmkoder/Views/MainWindow.EncoderArgs.cs:315`                             |
| `FileImportWindow`       | 640×420; dynamic title includes file count; pre-supplied dropped file list with one checked row per file (checkboxes start checked). `Replace file list` appears only when `allowClear`; otherwise primary button label changes to `Import`. When both are available, buttons are `Replace file list` and default `Add to file list`. Escape closes without importing. | `Nmkoder/Views/FileImportWindow.axaml:4,5,8,9,11,14,19,20,21`; `Nmkoder/Views/FileImportWindow.axaml.cs:31,34,37,40,41,43,44,46,59,65,71,73,74`; checked default `Nmkoder/Data/Ui/ListEntryBase.cs:16,20`                        |
| `MessageWindow`          | Dynamic message/title and button family: OK-only, Yes/No, or Yes/No/Cancel. The rightmost/default button is OK or Yes. Escape returns OK for OK-only, Cancel otherwise.                                                                                                                                                                                                | `Nmkoder/Views/MessageWindow.axaml:4,5,9,11,14,15,16,17`; `Nmkoder/Views/MessageWindow.axaml.cs:26,28,29,30,31,43,50,54,59,67,69,72,74,77,79,82,84,85`                                                                           |
| `PromptWindow`           | 440 px wide, height to content; dynamic title/message and one-line text field. Initial value is caller-provided default; OK returns trimmed text. Closing without OK returns the original default. Used for image-sequence frame-rate input.                                                                                                                           | `Nmkoder/Views/PromptWindow.axaml:4,5,9,10,11,12`; `Nmkoder/Views/PromptWindow.axaml.cs:18,20,21,22,23,24,36,38`; caller `Nmkoder/Data/MediaFile.cs:87`                                                                          |

## Remaining runtime-dependent values

- Film stock preset dropdown is supplied by the installed grav1synth binary after startup; the source fallback is listed above. The live installed binary was not executed as part of this read-only extraction.
- Sample Encodes speed-preset and color-format menus are populated from the chosen encoder class, and preset/content lists can differ when source tables change. The code-defined current sets and default indexes are cited above; the dialog does not hard-code them in XAML.
- File names, titles, languages, sizes, source dimensions, scan verdicts, preview frames, result text, and selected output rows are populated from the current media list or task results, so they cannot be enumerated as fixed literals.
- The main Settings tab fields and main encode-tab conditionals are covered by the separate main-screen inventory.
