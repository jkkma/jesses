# Additional audio codec qualification

Windows x64 development qualification on 2026-09-13 adds FLAC, MP3, Vorbis, and
E-AC-3 to per-track audio settings in Quick Convert, av1an, and batch encoding.
Existing Copy/AAC/Opus identities and saved settings remain readable.

## Codec contracts

All outputs retain the source sample rate except the existing Opus path, which
explicitly uses 48 kHz. Every conversion validates source channels, recognized
speaker layout, the selected encoder, and its actual settings before encoding
video. The chosen new codec/rate/layout/bitrate combination encodes a synthetic
one-second Matroska track, then both installed FFmpeg and FFprobe decode it.
Decoded start, sample count, final discard padding, rate, layout, and FLAC depth
must match. A successful encoder listing alone cannot pass this check.

| Choice        | Behavior and limits                                                                                                                                                                                                                                                          |
| ------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| FLAC (24-bit) | Native `flac`, explicit 24-bit integer PCM; no bitrate target. Integer PCM up to 24 bits is preserved when retaining channels. Floating-point and higher-depth sources are explicitly converted to 24 bits. Preserve rejects unsupported 4.0/quad(side) layouts.             |
| MP3           | `libmp3lame`, standard CBR settings; 32–320 kb/s at 32/44.1/48 kHz, or 32–160 kb/s at supported 8–24 kHz rates. Intermediate rates that LAME would round are rejected. Mono/stereo only, with an explicit downmix required for multichannel sources.                         |
| Vorbis        | `libvorbis`, target average bitrate 32–512 kb/s, source rate 8–192 kHz. The installed encoder's mode table must accept the exact bitrate/rate/channel combination. Preserve rejects 4.0, quad(side), 5.0(side), and 5.1(side), which would be reinterpreted.                 |
| E-AC-3        | Native `eac3`, source rate 32/44.1/48 kHz; target 32–6,144 kb/s at 48 kHz, scaled maximum at lower supported rates. Preserve rejects quad, 5.0, 5.1, 6.1, and 7.1 because the encoder would change layout or drop channels. Supported side-surround layouts remain explicit. |

The UI displays source speaker layout and concrete incompatibilities. It never
changes channel selection to resolve an unsupported Preserve choice. The runtime
re-probes source metadata and enforces the same boundary authoritatively.
FFmpeg's encoder options are documented in its
[codec reference](https://ffmpeg.org/ffmpeg-codecs.html); actual binaries determine
whether a requested combination is usable.

## Verification

The real-tool tests use FFmpeg/FFprobe 9.0.1 and standalone x264. Nineteen successful
additional-codec cases cover 44.1/48 kHz mono, stereo, multichannel preservation
where supported, explicit downmixes, MP3 at 8/22.05/32 kHz, Vorbis at 8 kHz,
E-AC-3 at 32 kHz and 48 kHz 5.1(side) at 640 kb/s, and FLAC at 192 kHz.

The 12 main cases independently compare decoded timelines and a non-silent signal
correlation above 0.98, check output codec/rate/channels, preserve subtitle packet
hashes, and confirm unchanged source bytes. Mono and 5.1 FLAC additionally compare
every decoded 24-bit PCM sample byte for exact preservation. The rate/layout
cases independently check identical sample counts, channel layout, start within
2 ms, and source bytes. Production validation also checks complete video timing,
selected copied streams, dispositions, chapters, and attachments.

A negative actual-tool gate exercises Vorbis 48 kHz mono at an unsupported
256 kb/s: the job fails in synthetic preflight before source video decoding,
does not publish an output, and releases temporary files. Unit guards reject
implicit layout changes, unsupported sample rates, nonstandard MP3 bitrates,
and E-AC-3 bitrates above the sample-rate-specific maximum.

The MP3 8 kHz case exposed a container-duration distinction: Matroska reports
LAME's 1,105-sample codec delay in its duration even when decoding trims it.
The runtime permits only that additional header duration for MP3, while decoded
start and sample count retain their strict existing limits.

The 137 encode/batch browser checks pass, including ten new cases for codec
choices in both workflows, explicit downmix errors, FLAC precision disclosure,
standard MP3 bitrate choices, and per-file FLAC/MP3 queue snapshots.

```powershell
cargo test -p media-runtime --test audio_jobs --locked -- --ignored --test-threads=1
```

All 11 actual-tool audio tests passed in 48.59 seconds, including the original
AAC/Opus conversion, timing, cancellation, and batch regression gates. All five
audio unit tests passed.

Linux CI's pinned FFmpeg build now explicitly enables libmp3lame and libvorbis;
its cache identity includes the compiler and codec development packages. Local
Windows results do not establish Linux runtime, native desktop interaction,
full-episode conversion, or clean-machine package qualification for these new
codecs. Loudness normalization is outside this change.
