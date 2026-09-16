# Source selection and previews

Quick Convert and av1an normally follow the file selected in **Files**. Use
**Encoding source** to keep one loaded file selected while inspecting another.
Video stream selection, source preview, geometry, color, copied tracks and the
queued request all use that encoding source. Each workflow retains its own choice
and source drafts. Removing a chosen source blocks submission until it is
reselected or **Follow the Files selection** is restored.

This selection applies to one source and its tracks. Use Remux's multi-file track
selection to assemble tracks from separate sources into a new file.

## Inspector thumbnails

The inspector reports the original rational frame rate, pixel aspect ratio,
display aspect ratio and rotation reported by FFprobe. Expand **Video thumbnail
& scrubbing** to inspect a bounded frame. Choose the video stream and position;
changing either cancels the previous request and discards late results.

Inspector images honor positive pixel aspect ratios and rotations in 90-degree
steps. HDR images are shown as SDR without changing the source or encode settings.
Arbitrary display-matrix angles are reported but receive an explicit thumbnail
compatibility message. Crop previews use coded pixel coordinates before rotation
or display aspect correction, so the crop outline still targets source pixels.

The preview PNG has its source display matrix cleared after the explicit
transform, preventing image viewers from applying the rotation twice. Source
metadata, sizes and bytes stay unchanged.

## Per-file batch eligibility

Batch encode snapshots only encode requests and revalidates each source. Concat,
reference comparisons, bitrate charts and CRF ladders have separate workflows;
they are not offered as per-file batch operations. Concat has one ordered source
list, comparisons require an explicit reference, and a CRF ladder manages its own
sample/rung set. Their results do not silently become queued encodes.

## Validation

Browser tests cover independent source selection, removal, request identity,
late thumbnail results and stream selection. Real-tool source analysis tests
compare the oriented/anamorphic PNG's decoded RGB against FFmpeg's automatic
display-matrix reference, retain coded crop coordinates, verify source bytes,
and exercise HDR previews and cancellation. This is Windows development evidence;
it does not qualify packaged builds or other operating systems.

See [FFmpeg's display-rotation option](https://ffmpeg.org/ffmpeg.html#Video-Options)
for the counterclockwise metadata convention.
