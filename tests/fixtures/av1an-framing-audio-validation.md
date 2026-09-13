# av1an framing and audio validation

Windows x64 development qualification on 2026-09-13 extends the av1an workflow
with the same manual framing and per-track audio settings used by standalone
encoding. Quick Convert, av1an, and each batch workflow retain independent drafts.
The first-video restriction and existing progressive CFR, square-pixel,
4:2:0 SDR/HDR10 source requirements still apply.

## Implemented behavior

The planner validates every crop edge, resize dimension, and black border before
submission. av1an receives the generated FFmpeg filter through its `--ffmpeg`
argument: crop, aspect-preserving Lanczos resize, black borders, then square-pixel
restoration. Paths never enter the nested filter text. Complete source frame
validation uses source dimensions; IVF and decoded output checks use transformed
dimensions. Frame count, timestamps, supported color, and HDR10 metadata remain
strictly checked before publication.

Audio offers Copy, Opus, and AAC independently for each selected source track,
including bitrate and preserve/mono/stereo channels. Jesses disables av1an's
internal audio extraction, then applies these choices during its controlled final
mux. Existing codec capability, priming, decoded sample timeline, channel layout,
metadata, and subtitle/attachment checks are shared with standalone encoding.
Saved settings include framing and audio, and resumed jobs reject incompatible
settings while retaining completed video chunks.

## Actual tool gates

The four tests in `crates/media-runtime/tests/av1an_recovery.rs` passed with
FFmpeg/FFprobe 9.0.1, av1an 0.5.2-unstable `7df934d`, VapourSynth/L-SMASH,
5fish `v2.3.260827`, and SVT-AV1-HDR `v4.1.0-21-g00333404f`.
The installed fork executable hashes match `scripts/svt-forks.json`.

```powershell
cargo test -p media-runtime --test av1an_recovery --locked -- --ignored --test-threads=1
```

All fixtures contain 720 frames at 24 fps, selected audio before video,
two timed subtitle cues, and a hashed 4,096-byte attachment. Each test stops after
completed chunks exist, closes/reopens history, resumes the same job once, and
observes unchanged chunk bytes and modification times while encoding continues.
The fivefish cases also reject changed source/tool/settings evidence and unknown
chunk files without destroying recorded progress. Baseline cases retain copied
audio and 320 × 180 video.

| Additional case | Source and settings                                                                                       | Verified output                                                                                                                                   |
| --------------- | --------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| 5fish + Opus    | BT.709 SDR; crop top/right/bottom/left 8/8/4/8; picture width 160; borders 8/8/16/16; stereo Opus 96 kb/s | 720 AV1 frames, 184 × 112, 10-bit limited range, continuous decoded audio with preserved start, original subtitle text/timing and attachment hash |
| HDR + AAC       | BT.2020/PQ with static mastering and content-light metadata; identical framing; stereo AAC 96 kb/s        | 720 AV1 frames, 184 × 112, HDR10 metadata preserved, codec delay/timeline checked, original subtitle text/timing and attachment hash              |

Decoded corner samples in both transformed outputs remain limited-range 10-bit
black with neutral chroma, allowing four sample values of lossy reconstruction
error. Source bytes are unchanged after the negative test restores its intentional
padding edit. Successful workspaces and temporary outputs are cleaned up.
The complete four-case run passed in 167.43 seconds.

The 127 encode and batch browser checks passed, including independent av1an
framing/audio drafts, submitted settings, queue immutability, restored defaults,
selected-video restrictions, and invalid value guards. The ordinary jobs module
checks passed 96 tests, with nine opt-in cases excluded from that invocation.
Both original `av1an_jobs` real-tool gates also passed in 26.06 seconds using
SVT-AV1 `v4.2.0+71+88-17cd99550` (Patman), including active cancellation and
output conflict protection. Its directory was selected only for that invocation;
an earlier invocation correctly rejected an HDR executable resolving as mainline.

## Native full-episode qualification

The Windows desktop workflow also passed on a full 1280 × 720 episode on
2026-09-13. Native controls submitted 5fish CRF18/preset8, two workers,
line-art5/texture4, a 16-pixel top border, and stereo Opus128 kb/s. The output
contains all 34,047 frames at 1280 × 736, with zero per-frame timestamp difference
from the original. Independent decoded border samples are exactly Y64/U512/V512.
All 27 streams remain: 353 ASS subtitle packets have identical payload/timestamps,
and all 24 font hashes match. Decoded audio contains 68,165,242 samples per channel
at 48 kHz; its duration differs from the original by 0.002 ms. Three waveform
windows spanning the episode have correlations from 0.9927 to 0.9989 and alignment
errors below 0.3 ms. Original bytes, SHA256 and modification time are unchanged.

The frozen executable SHA256 is
`54666c86d0cc108af7facf4bbb185906d6ecfc66fe16edbe300ac0587d0bb1ff`.
The local receipt records its precise hash alongside the immutable request,
successful job history, independent output report and native screenshots in
`Videos/Jesses-migration-native-20260913`. This was an interim portable development
build with bundled SVT forks and other tools supplied by the development machine.

## Qualification boundary

These tests establish Windows runtime behavior, native framing/audio controls,
full-episode output, and durable stop/reopen/resume. Linux av1an runtime,
packaged clean-machine behavior, quality targets and operating-system pause remain
separate qualification items.
Quality targeting remains disabled; no claim is made that av1an's probing stage
would apply the final chunk filters.
