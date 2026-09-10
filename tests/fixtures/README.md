# Synthetic media fixtures

These small test inputs are generated from FFmpeg test patterns and a sine wave. They contain no user media. Generated files and machine-specific probe reports stay outside version control.

Run from the repository root with FFmpeg and FFprobe on PATH:

```powershell
powershell -NoProfile -File tests/fixtures/generate.ps1
```

The script creates `tests/fixtures/generated/`, refuses an existing output directory, uses argument arrays, and checks tool exit codes. To retain multiple runs, choose a fresh `-OutputDirectory`. Use `-Ffmpeg` and `-Ffprobe` for explicit tool paths. Each run records the reported tool versions and full probe JSON beside the media.

| File                   | Expected probe result                                                                                                                                                                                                  |
| ---------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `progressive.mkv`      | Success; FFV1 video, 320 x 180, progressive, YUV 4:2:0 8-bit, BT.709 matrix, limited range, `24/1` rational frame rate, 24 decoded frames; PCM 16-bit audio, 48,000 Hz, two channels; 1.000-second container duration. |
| `fractional.mkv`       | Success; same stream properties with `24000/1001` rational frame rate, 24 decoded frames, and 1.001-second container duration.                                                                                         |
| `café 東京's clip.mkv` | Success; byte-for-byte copy of `fractional.mkv`; Unicode, spaces, and an apostrophe must survive path handling.                                                                                                        |
| `malformed.mkv`        | Nonzero probe exit and a structured `error` object; report an import error instead of fabricating streams or duration. Diagnostic wording may vary by tool build.                                                      |

Video is stream index 0 and audio index 1. The audio language is `eng` and the title is `Synthetic test pattern`. Compare rates as rational values and allow one container timebase tick when comparing duration. `nb_frames` can be absent even though `-count_frames` returns `nb_read_frames`; absence must remain distinct from zero. A two-channel PCM stream may omit `channel_layout`; do not invent a reported layout. File sizes and encoder tags vary by tool version and are not equality assertions.

For a focused manual probe:

```powershell
ffprobe -v error -show_error -show_format -show_streams -count_frames -of json "tests/fixtures/generated/fractional.mkv"
```

These recipes cover initial import and metadata checks. They do not establish encoding, HDR, variable-frame-rate, subtitle, attachment, cancellation, or packaged-app correctness.

References: [FFmpeg synthetic source filters](https://ffmpeg.org/ffmpeg-filters.html), [FFmpeg command options](https://ffmpeg.org/ffmpeg.html), and [FFprobe structured output](https://ffmpeg.org/ffprobe.html).
