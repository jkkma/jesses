import type {
  ContainerFormat,
  EncodeBackend,
  MediaStream,
  SubtitleMode,
  SubtitleTrackSettings,
} from '$lib/ipc/generated';
import { knownHdr } from './media-color';

export function defaultSubtitles(streams: MediaStream[]): SubtitleTrackSettings[] {
  return streams
    .filter((stream) => stream.kind === 'subtitle')
    .map((stream) => ({ streamIndex: stream.index, mode: 'copy' }));
}

/**
 * Matroska cannot store MP4 timed text (mov_text). In copy mode, standalone
 * encoding converts just those tracks to SubRip, which keeps their text and
 * timing, instead of refusing to run. Other tracks are still copied.
 */
export function storableSubtitleMode(
  mode: SubtitleMode,
  stream: MediaStream | undefined,
  container: ContainerFormat | undefined,
  backend: EncodeBackend,
): SubtitleMode {
  return mode === 'copy' &&
    container === 'matroska' &&
    backend === 'standalone' &&
    stream?.kind === 'subtitle' &&
    stream.codec === 'mov_text'
    ? 'subRip'
    : mode;
}

export function selectedSubtitles(
  tracks: SubtitleTrackSettings[],
  included: number[],
): SubtitleTrackSettings[] {
  return tracks
    .filter(
      (track) =>
        (included.includes(track.streamIndex) || track.mode === 'burnIn') &&
        (track.mode !== 'copy' || track.burnIn),
    )
    .map((track) => ({ ...track }));
}

export const subtitleLabels: Record<SubtitleMode, string> = {
  copy: 'Copy source',
  movText: 'MOV_TEXT',
  subRip: 'SubRip (SRT)',
  ass: 'ASS',
  webVtt: 'WebVTT',
  burnIn: 'Burn into video',
};

export function subtitleError(
  tracks: SubtitleTrackSettings[],
  included: number[],
  streams: MediaStream[],
  backend: EncodeBackend,
  video?: MediaStream,
  toneMapped = false,
  container?: ContainerFormat,
): string | null {
  // Matroska cannot store MP4 timed text, so copying it fails before encoding.
  const copiedMovText =
    container === 'matroska'
      ? streams.find((stream) => {
          if (stream.kind !== 'subtitle' || stream.codec !== 'mov_text') return false;
          if (!included.includes(stream.index)) return false;
          const track = tracks.find((track) => track.streamIndex === stream.index);
          return !track || (track.mode === 'copy' && !track.burnIn);
        })
      : undefined;
  if (copiedMovText)
    return backend === 'standalone'
      ? `Subtitle #${copiedMovText.index} is MP4 text (mov_text), which MKV cannot store. Choose SubRip, ASS or WebVTT for it in Subtitles, or deselect it.`
      : `Subtitle #${copiedMovText.index} is MP4 text (mov_text), which MKV cannot store. Deselect it; subtitle conversion requires standalone encoding.`;
  const selected = selectedSubtitles(tracks, included);
  if (selected.length && backend !== 'standalone')
    return 'Subtitle conversion and burn-in require standalone encoding.';
  if (selected.filter((track) => track.mode === 'burnIn' || track.burnIn).length > 1)
    return 'Choose at most one subtitle track to burn into the video.';
  if (
    selected.some((track) => track.mode === 'burnIn' || track.burnIn) &&
    knownHdr(video) &&
    !toneMapped
  )
    return 'Burn-in requires SDR video; HDR rendering needs an explicit tone-map workflow.';
  for (const track of selected) {
    const stream = streams.find(
      (stream) => stream.index === track.streamIndex && stream.kind === 'subtitle',
    );
    if (!stream) return 'Choose a subtitle track from this source.';
    if (
      !['ass', 'subrip', 'webvtt', 'mov_text'].includes(stream.codec ?? '') &&
      !(
        (track.mode === 'burnIn' || track.burnIn) &&
        ['hdmv_pgs_subtitle', 'dvd_subtitle', 'dvb_subtitle', 'xsub'].includes(stream.codec ?? '')
      )
    )
      return 'This subtitle format supports copying only.';
  }
  return null;
}

export function subtitleSummary(tracks: SubtitleTrackSettings[] | undefined): string {
  return (
    tracks
      ?.filter((track) => track.mode !== 'copy')
      .map((track) => `Subtitle #${track.streamIndex}: ${subtitleLabels[track.mode]}`)
      .join(' · ') ?? ''
  );
}
