import type { EncodeTrackRef, MediaFile, MediaStream } from '$lib/ipc/generated';
import type { TemporalDraft } from './temporal-options';

export type MovTimecodeChoice = {
  sourceId: string;
  inputPath: string;
  streamIndex: number;
  streamSnapshot: string;
  invalidated?: boolean;
};

export function isMovTimecode(stream: MediaStream): boolean {
  return stream.kind === 'data' && stream.codecTag?.toLowerCase() === 'tmcd';
}

export function movTimecodeChoice(file: MediaFile, stream: MediaStream): MovTimecodeChoice {
  return {
    sourceId: file.id,
    inputPath: file.path,
    streamIndex: stream.index,
    streamSnapshot: JSON.stringify(stream),
  };
}

export function invalidateMovTimecodeChoice(
  choice: MovTimecodeChoice | undefined,
  files: MediaFile[],
): MovTimecodeChoice | undefined {
  if (!choice || choice.invalidated) return choice;
  const file = files.find(
    (entry) => entry.id === choice.sourceId && entry.path === choice.inputPath,
  );
  const stream = file?.streams.find((entry) => entry.index === choice.streamIndex);
  return stream && isMovTimecode(stream) && JSON.stringify(stream) === choice.streamSnapshot
    ? choice
    : { ...choice, invalidated: true };
}

export function movTimecodeIssue(
  choice: MovTimecodeChoice | undefined,
  files: MediaFile[],
  outputIsMov: boolean,
  trimming: boolean,
  temporal: TemporalDraft,
): string | null {
  if (!choice) return null;
  const file = files.find(
    (entry) => entry.id === choice.sourceId && entry.path === choice.inputPath,
  );
  const stream = file?.streams.find((entry) => entry.index === choice.streamIndex);
  if (
    choice.invalidated ||
    !stream ||
    !isMovTimecode(stream) ||
    JSON.stringify(stream) !== choice.streamSnapshot
  )
    return 'The selected MOV timecode source was removed or changed. Choose a current timecode track.';
  if (!outputIsMov) return 'A copied timecode track requires MOV output.';
  if (trimming) return 'A copied MOV timecode track cannot be combined with trimming.';
  if (temporal.changeRate || temporal.deinterlace !== 'off')
    return 'A copied MOV timecode track cannot be combined with frame-rate, deinterlace or cadence changes.';
  return null;
}

export function selectedMovTimecode(
  choice: MovTimecodeChoice | undefined,
  primary: MediaFile | undefined,
): EncodeTrackRef | undefined {
  if (!choice) return undefined;
  return {
    ...(choice.inputPath !== primary?.path ? { inputPath: choice.inputPath } : {}),
    streamIndex: choice.streamIndex,
  };
}
