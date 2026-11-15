# Streamline encoding UI

The user prioritizes waits before and after encoding and requested specific Quick Convert and AV1AN defaults and a visible Tune control.

## Changes

Show truthful single-file preparation stages and provide Stop before the native job appears. Yield to native job progress as soon as the matching job is published. Cancellation waits for any operation that cannot itself be interrupted to return before stopping the next stage.

Explain common Run blockers: missing source, job connection or tools, output path, and video quality; retain existing track, container and subtitle explanations. Let the quality message open the Video tab. This is a focused improvement, not a complete validation rewrite.

On a fresh workflow, select SVT-AV1-HDR, CRF, preset 4, 10-bit 4:2:0, source frame rate, no crop, resize, borders or grain synthesis, and VQ tune. Quick Convert uses MP4 and quality 20; AV1AN uses WEBM and quality 35. Interlaced Quick Convert inputs default to one output frame per input frame; the user can explicitly select one frame per field. Keep existing source-change reset preferences and explicit in-session choices. AV1AN storage does not persist these video defaults; preserve unrelated saved preferences.

Expose Tune on Video for SVT-AV1-HDR only. Show advanced tune overrides in that selector, and let choosing a Video tune clear the override. Leave non-HDR encoders and advanced values available. Normal Run remains one click and Shift still opens command review.

On initial track loading, use the existing “First Track Of Each Type” menu rule: select the first video, audio and subtitle row, leaving other types unchecked. Preserve manual selections on existing rows when adding tracks.

Keep AV1AN's existing Scene Detection default and preserve saved split-method preferences without migration. Fixed chunks remains an explicit option. Its source workflow derives the effective chunk length from the configured keyframe interval and source rate, rounded and clamped to 12–480 frames. The 12.2-second Desktop benchmark used explicit fixed chunks and does not describe the released default.

## Verification

Exercise both workflows after source import, VQ requests, advanced tune overrides, source changes, narrow layout, first-track selection and submitted indices, common blockers, held preparation and cancellation, and job-progress takeover. Verify Scene Detection remains the default and saved preferences survive reload and source changes. Run frontend regressions, type check and build. Native backend evidence separately verifies that VQ maps to `--tune 0`.
