# Images and image sequences

Open **Utilities → Images and sequences**.

## Import

Choose PNG, JPEG, BMP, TIFF or WebP files and review their explicit order. The
arrow buttons reorder entries without renaming or changing the originals.
Provide a rational frame rate, such as 24000 / 1001, and a new .mkv destination.
Each image must contain one frame and use matching dimensions and file format.
Each image is limited to 1 GiB and dimensions from 1 to 16,384 pixels on each
axis. Up to 10,000 images and frame rates up to 120 fps are accepted.

The app copies each source into private staging in cancellable 1 MiB chunks,
checks its identity and size again, produces a lossless FFV1 video, checks its
decoded frame count and dimensions, verifies every source, then publishes and
imports the video. Cancellation removes only the partial files and directory
identities created by that job. The resulting file can be used with Quick
Convert or Batch. It contains no audio.

## Export

Choose an imported video and its original stream index, then a zero-based first
frame. PNG and JPEG still outputs save one frame; sequence and GIF outputs take
an explicit count, up to 10,000. Width resizing is optional.

Sequence destinations must be new directories. Existing output files and
directories are rejected, so a shorter subsequent export cannot leave stale
frames mixed into the result. Cancellation waits for the owned process tree and
removes owned scratch files. Cleanup compares file and directory identities and
leaves any same-name replacement created by another process untouched. Completed
output is published after decoding checks.

PNG retains full-color image output. JPEG is lossy. GIF uses a 256-color palette
and centisecond timing, so its color and timing precision differ from video.
HDR input must first use the explicit HDR-to-SDR conversion in Quick Convert.

The implementation uses FFmpeg's [image and concat formats](https://ffmpeg.org/ffmpeg-formats.html)
and [palette filters](https://ffmpeg.org/ffmpeg-filters.html#palettegen).

## Validation

The opt-in image_jobs tests exercise real FFmpeg/FFprobe: explicit nonalphabetic
ordering, exact decoded RGB round trips through FFV1 and PNG, source-byte
preservation, rejected existing destinations, GIF/JPEG/still decoding and
pre-cancellation. Unit tests cancel a staged copy after its first chunk and
replace an owned staged file before cleanup, confirming that the partial is
removed and the foreign replacement survives. Browser tests exercise reviewed
order, rational FPS and output import. These tests do not establish packaged
Linux or clean-machine coverage.
