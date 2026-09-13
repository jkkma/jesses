import type { EncodeBackend, MediaStream, VideoTrim } from '$lib/ipc/generated';
import type { AudioTrackDraft } from './audio-options';

export type TrimDraft = {
  enabled: boolean;
  startFrame: number | undefined;
  endFrameExclusive: number | undefined;
};
export const defaultTrim = (): TrimDraft => ({
  enabled: false,
  startFrame: 0,
  endFrameExclusive: undefined,
});
export function selectedTrim(draft: TrimDraft): VideoTrim | undefined {
  return draft.enabled
    ? { startFrame: draft.startFrame!, endFrameExclusive: draft.endFrameExclusive! }
    : undefined;
}
export function trimError(
  draft: TrimDraft,
  backend: EncodeBackend,
  audio: AudioTrackDraft[],
  included: number[],
  streams: MediaStream[],
): string | null {
  if (!draft.enabled) return null;
  if (backend !== 'standalone') return 'Frame intervals require standalone encoding.';
  if (
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
  return trim ? ` · Frames ${trim.startFrame}–${trim.endFrameExclusive} (end excluded)` : '';
}
