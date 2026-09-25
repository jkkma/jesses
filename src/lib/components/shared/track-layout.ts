import type { EncodeTrackRef, MediaFile } from '$lib/ipc/generated';
import type { ExternalTrackChoice } from './external-tracks';

export type TrackTags = {
  title?: string;
  language?: string;
  default?: boolean;
  forced?: boolean;
};

export function trackTagsIssue(tags: TrackTags): string | null {
  if (
    tags.title !== undefined &&
    (tags.title.includes('\0') || new TextEncoder().encode(tags.title).length > 4096)
  )
    return 'Track titles must be at most 4096 UTF-8 bytes and cannot contain a NUL character.';
  if (tags.language !== undefined && tags.language !== '' && !/^[A-Za-z]{3}$/.test(tags.language))
    return 'Track language must be a three-letter code, or blank to clear it.';
  return null;
}

export type SourceDonor = {
  sourceId: string;
  inputPath: string;
  sourceSnapshot: string;
  invalidated?: boolean;
};

export function sourceDonor(file: MediaFile): SourceDonor {
  return { sourceId: file.id, inputPath: file.path, sourceSnapshot: JSON.stringify(file) };
}

export function invalidateSourceDonor(
  donor: SourceDonor | undefined,
  files: MediaFile[],
): SourceDonor | undefined {
  if (!donor || donor.invalidated) return donor;
  const current = files.find((file) => file.id === donor.sourceId && file.path === donor.inputPath);
  return current && JSON.stringify(current) === donor.sourceSnapshot
    ? donor
    : { ...donor, invalidated: true };
}

export function sourceDonorIssue(
  donor: SourceDonor | undefined,
  files: MediaFile[],
  label: string,
): string | null {
  if (!donor) return null;
  const current = files.find((file) => file.id === donor.sourceId && file.path === donor.inputPath);
  return donor.invalidated || !current || JSON.stringify(current) !== donor.sourceSnapshot
    ? `${label} source was removed or changed. Choose a current source.`
    : null;
}

export function sameTrackRef(a: EncodeTrackRef, b: EncodeTrackRef): boolean {
  return (a.inputPath ?? '') === (b.inputPath ?? '') && a.streamIndex === b.streamIndex;
}

function refKey(ref: EncodeTrackRef): string {
  return JSON.stringify([ref.inputPath ?? '', ref.streamIndex]);
}

export function defaultTrackOrder(
  primary: MediaFile | undefined,
  videoIndex: number | undefined,
  included: number[],
  external: ExternalTrackChoice[],
  files: MediaFile[],
): { media: EncodeTrackRef[]; attachments: EncodeTrackRef[] } {
  const media: EncodeTrackRef[] = [];
  const attachments: EncodeTrackRef[] = [];
  if (videoIndex !== undefined) media.push({ streamIndex: videoIndex });
  for (const stream of primary?.streams ?? []) {
    if (stream.kind === 'video' || !included.includes(stream.index)) continue;
    (stream.kind === 'attachment' ? attachments : media).push({ streamIndex: stream.index });
  }
  for (const choice of external) {
    const source = files.find(
      (file) => file.id === choice.sourceId && file.path === choice.inputPath,
    );
    const stream = source?.streams.find((entry) => entry.index === choice.streamIndex);
    (stream?.kind === 'attachment' ? attachments : media).push({
      inputPath: choice.inputPath,
      streamIndex: choice.streamIndex,
    });
  }
  return { media, attachments };
}

export function effectiveTrackOrder(
  order: EncodeTrackRef[],
  defaults: { media: EncodeTrackRef[]; attachments: EncodeTrackRef[] },
): EncodeTrackRef[] {
  const retain = (group: EncodeTrackRef[]) => {
    const keys = new Set(group.map(refKey));
    const kept = order.filter((ref) => keys.has(refKey(ref)));
    return [...kept, ...group.filter((ref) => !kept.some((entry) => sameTrackRef(entry, ref)))];
  };
  return [...retain(defaults.media), ...retain(defaults.attachments)];
}

export function selectedTrackOrder(
  order: EncodeTrackRef[],
  defaults: { media: EncodeTrackRef[]; attachments: EncodeTrackRef[] },
): EncodeTrackRef[] | undefined {
  const base = [...defaults.media, ...defaults.attachments];
  const effective = effectiveTrackOrder(order, defaults);
  return effective.every((ref, index) => sameTrackRef(ref, base[index]))
    ? undefined
    : effective.map((ref) => ({ ...ref }));
}
