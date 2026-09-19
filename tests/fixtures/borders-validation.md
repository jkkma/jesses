# Black border validation

This milestone extends standalone SVT-AV1 and x264 framing with per-file black
borders in Quick Convert and Batch encode. Those two encoder families are the
development priorities. Additional encoder drivers are deferred.

## Behavior

- Crop first, resize the picture with the existing aspect-preserving Lanczos
  path, then add black borders. All four border widths are independent even
  pixel counts. The final dimensions must remain even and within 64–8192 pixels.
- The width control sizes the picture before borders. The form shows cropped,
  picture, and final dimensions when borders are enabled. Borders do not stretch
  the picture or change its square pixels, frame count, or cadence.
- Border controls are off by default. Turning them off retains the entered draft
  values but submits zero borders. Reset clears the current draft only. Each
  source, encoder and workflow keeps its own draft; batch edits require a fresh
  preview and queued jobs retain the submitted settings.
- Older settings without borders deserialize as zero borders. Saved jobs and
  recovery records retain their existing encoder identities.
- Every decoded source frame is checked at its original dimensions; every
  encoded frame is checked at the final dimensions. Existing depth, color, HDR,
  timing, selected-track, publication and cancellation checks remain active.

The filter is built from validated numeric settings using the documented
[FFmpeg filtering interface](https://ffmpeg.org/ffmpeg-filters.html). Pixel checks
use independent planar references rather than trusting filter arguments or a
successful process exit.

## Qualification

Local Windows x64 qualification on 2026-09-13 used the working tree on
`dev/svt-x264-borders`, based on `e04126b`. The qualified executable's SHA-256,
request JSON, process logs, independent media reports and source-integrity checks
are retained in `Jesses-borders-validation-20260913` under the local Videos
directory. These initial results describe local validation before publication.
Remote CI status is recorded separately.

The published combined branch at `6d127ac` passed
[CI run 34738421008](https://github.com/jkkma/jesses/actions/runs/34738421008)
on Windows x64, Linux x64, and the frontend. The Linux job includes the opt-in
framing gate. The final commit changes only a supervisor test and this record;
the production code is unchanged from the local border qualification.

- All 163 frontend tests passed, including request values, invalid edges and
  final-size limits, disabled-border draft retention, reset, source/build/workflow
  isolation, old history, per-file batch settings and preview invalidation.
  Desktop-width and narrower browser screenshots were inspected.
- The Rust workspace passed: 8 core tests, 148 runtime library tests and one
  ordinary integration test. Opt-in gates remain ignored in the ordinary suite.
  Clippy with warnings denied, Svelte checks, generated contracts, formatting,
  the frontend build and the embedded-frontend Windows executable build passed.
- All four opt-in `framing_jobs` gates passed. The SDR matrix covers 13 encodes
  across x264 and the three SVT choices, 8/10-bit limited/full range, crop-only,
  combined crop/resize/borders and border-only framing. x264 8-bit CRF 0 output
  is compared byte-for-byte against independently constructed planar frames;
  those references do not use the production border filter.
- The remaining gates check different saved batch geometries, rejected invalid
  borders, existing-file preservation, HDR10 static metadata, and cancellation
  cleanup. Copied audio/subtitle packet fingerprints, attachments, chapters,
  complete decoded frames, timing, depth and color are checked through actual
  encoder jobs.
- Independent raw-filter probes cover seven input/depth/range cases and three
  frames each. All 201,600 samples matched, including range extremes, below-black
  and above-white values, neutral chroma and SVT's 8-to-10-bit conversion.
  The retained script, JSON results and notes include commands and tool versions.

The first publication run exposed an existing Linux supervisor-test race:
consumer pipe closure can be reported before process-exit polling completes.
The test now accepts only that specific `BrokenPipe` error alongside the expected
early/nonzero consumer-exit errors. Its timeout and process-death assertions stay
active. Production pipeline behavior is unchanged.

The local FFmpeg/FFprobe pair is 9.0.1. The managed 5fish and HDR tools use the
existing pinned installations. The separately selected generic SVT executable
reports `SVT-AV1 v4.2.0+71+88-17cd99550 [Mod by Patman]`; this result does not
qualify every upstream SVT version. The first generic-SVT attempt correctly
failed with `ENCODER_BUILD_MISMATCH` because the default PATH binary was an HDR
build. The successful test invocation prepended the separately identified
generic executable directory to its process PATH without changing the machine's
configuration or weakening identity checks.

### Real media

The runtime and a separate FFprobe/FFmpeg verifier checked these existing,
read-only excerpts. Each output retained the selected stream order, tags,
dispositions, copied-track packets, attachments and chapters. Video timestamp
differences were at most 1 ms due to container rounding.

| Encode                                      | Picture size | Final size  | Frames | Tracks | Output bytes |
| ------------------------------------------- | ------------ | ----------- | -----: | -----: | -----------: |
| Anime, x264 CRF 23/medium, copied audio     | 1280 × 716   | 1280 × 784  |    236 |     27 |   11,008,250 |
| Anime, 5fish CRF 30/preset 8, Opus 128 kb/s | 1280 × 716   | 1280 × 784  |    236 |     27 |   11,176,263 |
| HDR movie, HDR CRF 30/preset 8              | 1920 × 1082  | 1952 × 1124 |    114 |      5 |    3,361,354 |

The converted Opus track also passed decoded-sample and waveform-alignment
checks. The HDR job used the existing explicit HDR10 fallback setting for its
dynamic-HDR input and passed the runtime's static HDR10 validation. This test
setting does not change the application's default.

Independent border-interior samples were exactly black with neutral chroma for
x264 and the HDR output; the lossy 5fish output differed by at most one 10-bit
code value. Both source files retained their SHA-256, size and UTC modification
time. No media processes or partial outputs remained after qualification.

### Desktop boundary

#### Native continuation, 2026-09-13

The retained qualified executable (SHA-256
`85F6ABF5BD9763DAB087E77367B79FD8B2413A47A0330CA5BE30A9793D658BF4`)
launched successfully in a later native Windows session. Quick Convert rejected
an odd three-pixel border with disabled submission. Valid 32-pixel top/bottom
borders changed 1920x1080 to 1920x1144; disabling/re-enabling borders retained
the entered values and restored the expected dimensions.

A native x264 CRF 23/medium encode completed and passed the independent verifier:
236 frames, all 27 selected streams, source color/depth, copied payloads and
attachments, and at most 1 ms of container timestamp rounding. Border interiors
were exactly Y=16/U=128/V=128. Source SHA-256, byte size and modification time
were unchanged. Native request/history, screenshots and independent JSON are
retained in `Jesses-borders-native-20260913` under the local Videos directory.
The same session exercised native per-file Batch border controls and reviewed
the output preview before queueing. Its standalone SVT-AV1-HDR CRF 30/preset 2
output added 32-pixel left/right borders to produce 1984x1080. Independent
verification again passed all 236 frames, 27 streams, copied packets and fonts,
border samples and timestamp checks. Both outputs and native job snapshots are
retained in the same evidence directory. This completes native Quick Convert and
Batch border interaction/output qualification for this Windows executable;
packaged clean-machine checks remain a separate gate. Linux native desktop
qualification is deferred.

#### Earlier helper failures

The Windows executable built successfully and browser layouts were inspected.
Native UI verification could not be completed: the computer-use helper returned
`computer-use request timed out: launch_app` on the initial attempt and the
single recovery retry. No targetable Jesses window was returned. This milestone
therefore claims actual runtime encodes and browser interaction coverage, with
native desktop interaction still unverified.

A follow-up on 2026-09-13 confirmed that the desktop executable still matched
the recorded qualified binary. Fresh app and window discovery succeeded, but
two launch attempts again returned `computer-use request timed out: launch_app`
and exposed no targetable Jesses window. This follow-up does not qualify native
border interaction; that gate remains open after main integration.

## Boundaries

av1an framing was unavailable at this checkpoint; its subsequent
[framing and audio qualification](av1an-framing-audio-validation.md) records
implementation and real-tool recovery tests. Automatic crop, trim, frame-rate conversion,
non-square-pixel sources, arbitrary resize aspect ratios and colored borders
remain pending. x264 retains its existing SDR-only support. HDR10 tests use the
existing supported SVT path; selecting borders does not enable dynamic-HDR
fallback. The Linux CI lane at this checkpoint had the same opt-in framing gate;
local Windows checks did not establish a Linux runtime result or clean-machine
packaging qualification. Linux native qualification is now deferred.
