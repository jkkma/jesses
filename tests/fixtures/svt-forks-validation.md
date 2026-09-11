# SVT fork validation

Windows qualification on 2026-09-11 used FFmpeg/FFprobe 9.0.1, 5fish
`v2.3.260827`, and SVT-AV1-HDR `v4.1.0-21-g00333404f`. Downloads and extracted
Windows executables matched the SHA-256 hashes in
[the pinned manifest](../../scripts/svt-forks.json). Each fork was installed in
its own managed directory, with licenses and provenance retained.

## Actual media

- A 1920 × 1080 SDR anime excerpt completed through the public JobManager at
  CRF 18, preset 2, line-art bias 5, texture bias 4, and synthesis off. The result
  contained all 236 AV1 frames at 24000/1001 fps, 10-bit BT.709 limited range,
  plus its selected AAC track, ASS subtitles, and 24 font attachments (27 tracks).
- A 3840 × 2160 HDR movie excerpt completed through the public JobManager at
  CRF 30, preset 2, film grain retention tune 5, and synthesis off. All 114 frames
  passed decoded timing, geometry, color, and static HDR metadata checks; both
  audio and both subtitle tracks were copied. The test explicitly enabled the
  existing HDR10 fallback for this Dolby Vision profile 7 / HDR10+ source.
  The output is static HDR10, without the source's dynamic HDR metadata.
- Both complete source and output frame scans passed before publication.
  The anime and HDR excerpt source SHA-256 values remained unchanged.
- Separate direct HDR qualification covered 16 full-resolution movie frames
  at the same CRF/preset/tune, plus synthetic synthesis-off and synthesis-on
  samples. Static mastering and content-light metadata were preserved.

These short excerpts verify pipeline compatibility. They do not establish
full-film throughput, visual superiority, or clean-machine installer coverage.

## av1an compatibility

The installed av1an 0.5.2 development build names `SvtAv1EncApp` internally.
Jesses supplies a child-specific PATH, and on Windows stages an owned av1an
image so an encoder beside the original av1an executable cannot take precedence.
The original installation and application environment remain untouched.

Its `av-ivf` 0.5.0 concatenator panicked on a valid short 5fish AV1 packet.
The integration now uses av1an's FFmpeg concatenator with stream copy, retaining
the complete IVF record, decoded-frame, cadence, track, and HDR checks.
Both forks completed native av1an tests with 264 frames, at least two chunks,
two workers, fractional cadence, and persisted fork-specific settings. A running
480-frame 5fish job canceled after its worker queue started, published no output,
preserved its source, and released all chunk, source, and staged executable handles.

## Automated and interface checks

- Unit checks cover legacy history, exact fork identities, invalid overrides,
  settings isolation, HDR fallback consent, and fork-specific arguments.
- Native standalone and av1an integration checks encode SDR and static HDR
  fixtures, retain reordered copied audio, validate every frame, and reopen
  saved history with the exact encoder and tuning settings.
- Supervisor checks cover simultaneous child PATH overrides, descendant
  inheritance, unchanged parent environment, Windows hidden-drive variables,
  Unicode values, and ANSI-colored diagnostic/progress streams.
- All 116 browser checks passed, including fork defaults, source/workflow
  drafts, exact tool availability, stale asynchronous replies, batch previews,
  and queued settings. Svelte checking and the native debug build passed.
- Native computer-control verification was blocked by the helper error
  `foreground window did not report a process id` after mouse movement and a
  refreshed window handle. The browser checks and real runtime jobs are the
  interface and execution evidence for this milestone.

The CI workflow installs the manifest's pinned Linux binaries and runs the
standalone fork gate; av1an's external VapourSynth/L-SMASH dependency gate remains
an explicit local native check.
