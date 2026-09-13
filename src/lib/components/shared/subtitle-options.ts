import type {
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

export function selectedSubtitles(
  tracks: SubtitleTrackSettings[],
  included: number[],
): SubtitleTrackSettings[] {
  return tracks
    .filter((track) => included.includes(track.streamIndex) && track.mode !== 'copy')
    .map((track) => ({ ...track }));
}

export const subtitleLabels: Record<SubtitleMode, string> = {
  copy: 'Copy source',
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
): string | null {
  const selected = selectedSubtitles(tracks, included);
  if (selected.length && backend !== 'standalone')
    return 'Subtitle conversion and burn-in require standalone encoding.';
  if (selected.filter((track) => track.mode === 'burnIn').length > 1)
    return 'Choose at most one subtitle track to burn into the video.';
  if (selected.some((track) => track.mode === 'burnIn') && knownHdr(video) && !toneMapped)
    return 'Burn-in requires SDR video; HDR rendering needs an explicit tone-map workflow.';
  for (const track of selected) {
    const stream = streams.find(
      (stream) => stream.index === track.streamIndex && stream.kind === 'subtitle',
    );
    if (!stream) return 'Choose a subtitle track from this source.';
    if (
      !['ass', 'subrip', 'webvtt', 'mov_text'].includes(stream.codec ?? '') &&
      !(
        track.mode === 'burnIn' &&
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
