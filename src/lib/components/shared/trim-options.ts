import type { EncodeBackend, MediaStream, VideoTrim } from '$lib/ipc/generated';
import type { AudioTrackDraft } from './audio-options';

export type TrimDraft = {
  enabled: boolean;
  mode: 'frames' | 'time';
  startFrame: number | undefined;
  endFrameExclusive: number | undefined;
  startMilliseconds: number | undefined;
  endMilliseconds: number | undefined;
};
export const defaultTrim = (): TrimDraft => ({
  enabled: false,
  mode: 'frames',
  startFrame: 0,
  endFrameExclusive: undefined,
  startMilliseconds: 0,
  endMilliseconds: undefined,
});
export function selectedTrim(draft: TrimDraft): VideoTrim | undefined {
  if (!draft.enabled) return undefined;
  if (draft.mode === 'time') {
    return {
      startFrame: 0,
      endFrameExclusive: 0,
      time: {
        startMilliseconds: draft.startMilliseconds!,
        endMilliseconds: draft.endMilliseconds!,
      },
    };
  }
  return { startFrame: draft.startFrame!, endFrameExclusive: draft.endFrameExclusive! };
}
export function trimError(
  draft: TrimDraft,
  backend: EncodeBackend,
  audio: AudioTrackDraft[],
  included: number[],
  streams: MediaStream[],
): string | null {
  if (!draft.enabled) return null;
  if (draft.mode === 'time') {
    if (
      ![draft.startMilliseconds, draft.endMilliseconds].every(
        (value) =>
          typeof value === 'number' &&
          Number.isSafeInteger(value) &&
          value >= 0 &&
          value <= 4_294_967_295,
      ) ||
      draft.endMilliseconds! <= draft.startMilliseconds!
    )
      return 'Enter a zero-based start time and a greater, exclusive end time to millisecond precision.';
  } else if (
    ![draft.startFrame, draft.endFrameExclusive].every(
      (value) =>
        typeof value === 'number' &&
        Number.isInteger(value) &&
        value >= 0 &&
        value <= 4_294_967_295,
    ) ||
    draft.endFrameExclusive! <= draft.startFrame!
  )
    return 'Enter a zero-based start frame and a greater, exclusive end frame.';
  if (
    streams.some(
      (stream) =>
        stream.kind === 'audio' &&
        included.includes(stream.index) &&
        (!audio.find((track) => track.streamIndex === stream.index) ||
          audio.find((track) => track.streamIndex === stream.index)?.codec === 'copy'),
    )
  )
    return 'Choose an audio conversion for each included audio track when trimming. Copy cannot cut compressed packets at exact sample boundaries.';
  if (
    streams.some(
      (stream) =>
        stream.kind === 'subtitle' &&
        included.includes(stream.index) &&
        !['ass', 'subrip', 'webvtt'].includes(stream.codec ?? ''),
    )
  )
    return 'Trimming supports ASS, SubRip and WebVTT text subtitles. Exclude unsupported subtitle tracks.';
  return null;
}
export function trimSummary(trim: VideoTrim | undefined | null): string {
  if (!trim) return '';
  if (trim.time)
    return ` · Time ${(trim.time.startMilliseconds / 1_000).toFixed(3)}–${(trim.time.endMilliseconds / 1_000).toFixed(3)} s (end excluded)`;
  return ` · Frames ${trim.startFrame}–${trim.endFrameExclusive} (end excluded)`;
}
