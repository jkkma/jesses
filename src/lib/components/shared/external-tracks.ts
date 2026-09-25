import type {
  ExternalAudioSettings,
  ExternalTrack,
  EncodeBackend,
  MediaFile,
  MediaStream,
} from '$lib/ipc/generated';
import { audioCodecLabels, validAudio } from './audio-options';
import { subtitleError, subtitleLabels } from './subtitle-options';

export type ExternalAudioDraft = Omit<ExternalAudioSettings, 'bitrateKbps'> & {
  bitrateKbps: number | undefined;
};

export type ExternalTrackChoice = Omit<ExternalTrack, 'audio'> & {
  audio?: ExternalAudioDraft;
  sourceId: string;
  streamSnapshot: string;
  invalidated?: boolean;
};

export const externalKinds = new Set(['audio', 'subtitle', 'attachment']);

export function copyExternalChoices(choices: ExternalTrackChoice[]): ExternalTrackChoice[] {
  return choices.map((choice) => ({
    ...choice,
    ...(choice.audio
      ? {
          audio: {
            ...choice.audio,
            ...(choice.audio.gain ? { gain: { ...choice.audio.gain } } : {}),
          },
        }
      : {}),
  }));
}

export function requestExternalTracks(choices: ExternalTrackChoice[]): ExternalTrack[] {
  return choices.map(
    ({
      inputPath,
      streamIndex,
      audio,
      offsetMilliseconds,
      subtitleMode,
      title,
      language,
      default: defaultFlag,
      forced,
    }) => ({
      inputPath,
      streamIndex,
      ...(offsetMilliseconds ? { offsetMilliseconds } : {}),
      ...(audio && audio.codec !== 'copy'
        ? {
            audio: {
              codec: audio.codec,
              bitrateKbps: audio.bitrateKbps ?? 128,
              channels: audio.channels,
              ...(audio.gain ? { gain: { ...audio.gain } } : {}),
            } satisfies ExternalAudioSettings,
          }
        : {}),
      ...(subtitleMode && subtitleMode !== 'copy' ? { subtitleMode } : {}),
      ...(title !== undefined ? { title } : {}),
      ...(language !== undefined ? { language } : {}),
      ...(defaultFlag !== undefined ? { default: defaultFlag } : {}),
      ...(forced !== undefined ? { forced } : {}),
    }),
  );
}

export function externalTrackSummary(choices: ExternalTrackChoice[]): string {
  const converted = choices.filter((choice) => choice.audio && choice.audio.codec !== 'copy');
  const subtitles = choices.filter(
    (choice) => choice.subtitleMode && choice.subtitleMode !== 'copy',
  );
  const copied = choices.length - converted.length - subtitles.length;
  const shifted = choices.filter((choice) => choice.offsetMilliseconds).length;
  return [
    copied ? `${copied} copied track${copied === 1 ? '' : 's'} from other files` : '',
    converted.length
      ? `${converted.length} external audio track${converted.length === 1 ? '' : 's'} converted (${converted.map((choice) => audioCodecLabels[choice.audio!.codec]).join(', ')})`
      : '',
    subtitles.length
      ? `${subtitles.length} external subtitle action${subtitles.length === 1 ? '' : 's'} (${subtitles.map((choice) => subtitleLabels[choice.subtitleMode!]).join(', ')})`
      : '',
    shifted ? `${shifted} timing offset${shifted === 1 ? '' : 's'}` : '',
  ]
    .filter(Boolean)
    .join(' · ');
}

export function externalChoice(source: MediaFile, stream: MediaStream): ExternalTrackChoice {
  return {
    sourceId: source.id,
    inputPath: source.path,
    streamIndex: stream.index,
    streamSnapshot: JSON.stringify(stream),
  };
}

export function sameExternalTrack(
  a: Pick<ExternalTrack, 'inputPath' | 'streamIndex'>,
  b: Pick<ExternalTrack, 'inputPath' | 'streamIndex'>,
): boolean {
  return a.inputPath === b.inputPath && a.streamIndex === b.streamIndex;
}

export function invalidateExternalChoices(
  choices: ExternalTrackChoice[],
  files: MediaFile[],
): ExternalTrackChoice[] {
  let changed = false;
  const result = choices.map((choice) => {
    const source = files.find(
      (entry) => entry.id === choice.sourceId && entry.path === choice.inputPath,
    );
    const stream = source?.streams.find((entry) => entry.index === choice.streamIndex);
    if (
      choice.invalidated ||
      (stream && externalKinds.has(stream.kind) && JSON.stringify(stream) === choice.streamSnapshot)
    )
      return choice;
    changed = true;
    return { ...choice, invalidated: true };
  });
  return changed ? result : choices;
}

export type ExternalTrackContext = {
  trimming: boolean;
  backend: EncodeBackend;
  video?: MediaStream;
  toneMapped: boolean;
  primaryBurnCount: number;
};

export function externalTrackIssue(
  choices: ExternalTrackChoice[],
  files: MediaFile[],
  primary: MediaFile | undefined,
  context: ExternalTrackContext,
): string | null {
  if (!choices.length) return null;
  if (choices.length > 100) return 'Choose no more than 100 tracks from other files.';
  if (new Set(choices.map((choice) => choice.sourceId)).size > 32)
    return 'Choose tracks from no more than 32 other files.';
  if (
    context.primaryBurnCount + choices.filter((choice) => choice.subtitleMode === 'burnIn').length >
    1
  )
    return 'Choose at most one subtitle track to burn into the video across all source files.';
  const seen = new Set<string>();
  for (const choice of choices) {
    if (choice.invalidated)
      return 'A selected track source was removed or changed. Remove its selection and choose a current track.';
    const key = JSON.stringify([choice.inputPath, choice.streamIndex]);
    if (seen.has(key)) return 'The same track from another file was selected twice.';
    seen.add(key);
    const source = files.find(
      (entry) => entry.id === choice.sourceId && entry.path === choice.inputPath,
    );
    if (!source)
      return 'A selected track source was removed. Remove its selection and choose a current file.';
    if (source.id === primary?.id || source.path === primary?.path)
      return 'Tracks from the video source belong in the primary track list. Remove this external selection.';
    const stream = source.streams.find((entry) => entry.index === choice.streamIndex);
    if (
      !stream ||
      !externalKinds.has(stream.kind) ||
      JSON.stringify(stream) !== choice.streamSnapshot
    )
      return 'A selected track changed after it was added. Remove it and choose the current track.';
    if (
      choice.offsetMilliseconds !== undefined &&
      (!Number.isSafeInteger(choice.offsetMilliseconds) ||
        Math.abs(choice.offsetMilliseconds) > 86_400_000)
    )
      return 'Enter a timing offset from −86400 to +86400 seconds, to millisecond precision.';
    if (stream.kind === 'attachment' && choice.offsetMilliseconds)
      return 'Timing offsets apply only to audio and subtitle tracks.';
    if (choice.audio) {
      if (stream.kind !== 'audio')
        return 'Only audio tracks from other files can be converted. Remove the invalid track settings.';
      if (
        choice.audio.codec === 'copy' ||
        !validAudio(
          [{ streamIndex: choice.streamIndex, ...choice.audio }],
          [choice.streamIndex],
          source.streams,
        )
      )
        return 'Check the selected external audio codec, channels, and bitrate beside its source track.';
    }
    if (choice.subtitleMode && choice.subtitleMode !== 'copy') {
      if (stream.kind !== 'subtitle')
        return 'Subtitle actions apply only to subtitle tracks from other files.';
      const issue = subtitleError(
        [{ streamIndex: choice.streamIndex, mode: choice.subtitleMode }],
        [choice.streamIndex],
        source.streams,
        context.backend,
        context.video,
        context.toneMapped,
      );
      if (issue) return issue;
    }
    if (context.trimming) {
      if (stream.kind === 'audio' && !choice.audio)
        return 'Choose audio conversion for every external audio track when trimming. Copy cannot cut compressed packets at exact sample boundaries.';
      if (
        stream.kind === 'subtitle' &&
        !['ass', 'subrip', 'webvtt', 'mov_text'].includes(stream.codec ?? '') &&
        !(
          ['hdmv_pgs_subtitle', 'dvd_subtitle', 'dvb_subtitle', 'xsub'].includes(
            stream.codec ?? '',
          ) && choice.subtitleMode === 'burnIn'
        )
      )
        return 'Trimming supports text subtitles and burned bitmap subtitles from other files. Exclude unsupported subtitle tracks or burn bitmap subtitles into video.';
    }
  }
  return null;
}
