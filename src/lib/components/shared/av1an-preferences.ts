import type { Av1anDraft } from './av1an-options';
import type { AudioChannels, AudioCodec, MediaStream } from '$lib/ipc/generated';
import {
  audioBitrateChoices,
  audioBitrateMax,
  defaultAudio,
  validAudio,
  type AudioTrackDraft,
} from './audio-options';

const key = 'jesses.av1an.preferences.v1';
const chunkMethods = ['lsmash', 'ffms2', 'bestsource', 'select', 'hybrid', 'segment'] as const;
const splitMethods = ['sceneDetection', 'fixedChunks'] as const;
const chunkOrders = ['longToShort', 'shortToLong', 'sequential', 'random'] as const;
const concatMethods = ['ffmpeg', 'mkvmerge'] as const;
const audioCodecs = ['copy', 'opus', 'aac', 'flac', 'mp3', 'vorbis', 'eac3'] as const;
const audioChannels = ['preserve', 'mono', 'stereo', 'surround51', 'surround71'] as const;
const integer = (value: unknown, minimum: number, maximum: number): value is number =>
  typeof value === 'number' && Number.isInteger(value) && value >= minimum && value <= maximum;
const choice = <T extends string>(value: unknown, choices: readonly T[]): value is T =>
  typeof value === 'string' && choices.includes(value as T);

export type Av1anAudioPreference = {
  codec: AudioCodec;
  bitrateKbps: number;
  channels: AudioChannels;
};

export type Av1anPreferences = Pick<
  Av1anDraft,
  | 'chunkMethod'
  | 'splitMethod'
  | 'chunkOrder'
  | 'concatMethod'
  | 'encoderThreads'
  | 'sceneDetectionSlices'
> & {
  workers: number;
  workerBaseline: number;
  filters: string[];
  audio: Av1anAudioPreference | null;
  stripAudio: boolean;
  copySubs: boolean;
  copyData: boolean;
  copyAttachments: boolean;
};

const defaults = (): Av1anPreferences => {
  const cores =
    typeof navigator === 'undefined' ? 4 : Math.max(1, navigator.hardwareConcurrency || 4);
  const baseline = Math.max(2, Math.min(32, Math.ceil(cores * 0.4)));
  return {
    chunkMethod: 'lsmash',
    splitMethod: 'sceneDetection',
    chunkOrder: 'longToShort',
    concatMethod: 'mkvmerge',
    encoderThreads: Math.max(
      2,
      Math.min(16, Math.ceil(Math.max(2, Math.round(cores * 0.8)) / baseline)),
    ),
    sceneDetectionSlices: Math.max(1, Math.min(8, Math.floor(cores / 2))),
    workers: Math.max(1, baseline - 2),
    workerBaseline: baseline,
    filters: [],
    audio: null,
    stripAudio: false,
    copySubs: true,
    copyData: false,
    copyAttachments: true,
  };
};

function safeAudio(value: unknown): value is Av1anAudioPreference {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
  const audio = value as Record<string, unknown>;
  return (
    choice(audio.codec, audioCodecs) &&
    choice(audio.channels, audioChannels) &&
    integer(audio.bitrateKbps, 0, 8192)
  );
}

function safeFilters(value: unknown, source = false): value is string[] {
  if (source)
    return (
      Array.isArray(value) && value.every((row) => typeof row === 'string' && !row.includes('\0'))
    );
  return (
    Array.isArray(value) &&
    value.length <= 16 &&
    value.every(
      (row) =>
        typeof row === 'string' &&
        row.length > 0 &&
        row.length <= 4096 &&
        !/[\r\n\\]/.test(row) &&
        !/[a-z]:\//i.test(row) &&
        !/(?:^|[=,:])\s*[/~]/.test(row) &&
        !/\b(?:file|filename|path)\s*=/.test(row.toLowerCase()),
    )
  );
}

export function readAv1anPreferences(sourceFilters = false): Av1anPreferences {
  const result = defaults();
  try {
    if (typeof localStorage === 'undefined') return result;
    const raw = localStorage.getItem(key);
    if (!raw || (!sourceFilters && raw.length > 70_000)) return result;
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return result;
    const saved = parsed as Record<string, unknown>;
    if (saved.version !== 1) return result;
    if (choice(saved.chunkMethod, chunkMethods)) result.chunkMethod = saved.chunkMethod;
    if (choice(saved.splitMethod, splitMethods)) result.splitMethod = saved.splitMethod;
    if (choice(saved.chunkOrder, chunkOrders)) result.chunkOrder = saved.chunkOrder;
    if (choice(saved.concatMethod, concatMethods)) result.concatMethod = saved.concatMethod;
    if (integer(saved.encoderThreads, 0, 64)) result.encoderThreads = saved.encoderThreads;
    if (integer(saved.sceneDetectionSlices, 1, 16))
      result.sceneDetectionSlices = saved.sceneDetectionSlices;
    if (integer(saved.workerBaseline, 1, 66)) result.workerBaseline = saved.workerBaseline;
    else if (integer(saved.workers, 1, 64)) result.workerBaseline = saved.workers + 2;
    result.workers = Math.max(1, result.workerBaseline - 2);
    for (const name of ['stripAudio', 'copySubs', 'copyData', 'copyAttachments'] as const)
      if (typeof saved[name] === 'boolean') result[name] = saved[name];
    if (safeFilters(saved.filters, sourceFilters)) result.filters = [...saved.filters];
    if (safeAudio(saved.audio))
      result.audio = {
        codec: saved.audio.codec,
        bitrateKbps: saved.audio.bitrateKbps,
        channels: saved.audio.channels,
      };
  } catch {
    // Corrupt or unavailable browser storage only resets these suggestions.
  }
  return result;
}

export function saveAv1anPreferences(
  draft: Av1anDraft,
  workers: number | undefined,
  filters: string[],
  workerBaseline?: number,
  sourceFilters = false,
): string | undefined {
  if (
    !choice(draft.chunkMethod, chunkMethods) ||
    !choice(draft.splitMethod, splitMethods) ||
    !choice(draft.chunkOrder, chunkOrders) ||
    !choice(draft.concatMethod ?? 'ffmpeg', concatMethods) ||
    (draft.encoderThreads !== undefined && !integer(draft.encoderThreads, 0, 64)) ||
    (draft.sceneDetectionSlices !== undefined && !integer(draft.sceneDetectionSlices, 1, 16)) ||
    !integer(workers, 1, 64) ||
    !safeFilters(filters, sourceFilters)
  )
    return;
  try {
    if (typeof localStorage === 'undefined') return;
    localStorage.setItem(
      key,
      JSON.stringify({
        version: 1,
        chunkMethod: draft.chunkMethod,
        splitMethod: draft.splitMethod,
        chunkOrder: draft.chunkOrder,
        concatMethod: draft.concatMethod ?? 'ffmpeg',
        encoderThreads: draft.encoderThreads,
        sceneDetectionSlices: draft.sceneDetectionSlices,
        workers,
        workerBaseline: workerBaseline ?? workers + 2,
        filters,
        audio: readAv1anPreferences(sourceFilters).audio,
        ...Object.fromEntries(
          ['stripAudio', 'copySubs', 'copyData', 'copyAttachments'].map((name) => [
            name,
            readAv1anPreferences(sourceFilters)[
              name as 'stripAudio' | 'copySubs' | 'copyData' | 'copyAttachments'
            ],
          ]),
        ),
      }),
    );
  } catch (cause) {
    // Jobs remain usable when browser storage is disabled or full.
    return `AV1AN preferences could not be saved: ${cause instanceof Error ? cause.message : String(cause)}`;
  }
}

export function defaultAv1anAudio(
  streams: MediaStream[],
  sourceFilters = false,
): AudioTrackDraft[] {
  const saved = readAv1anPreferences(sourceFilters).audio;
  const tracks = defaultAudio(streams);
  if (!saved) return tracks;
  return tracks.map((track) => {
    const stream = streams.find((candidate) => candidate.index === track.streamIndex);
    if (!stream) return track;
    const candidate: AudioTrackDraft = { ...track, ...saved };
    const choices = audioBitrateChoices(candidate, stream);
    if (choices && !choices.includes(candidate.bitrateKbps!))
      candidate.bitrateKbps =
        choices.filter((choice) => choice <= saved.bitrateKbps).at(-1) ?? choices[0];
    else if (candidate.bitrateKbps! > audioBitrateMax(candidate, stream))
      candidate.bitrateKbps = audioBitrateMax(candidate, stream);
    return validAudio([candidate], [track.streamIndex], streams) ? candidate : track;
  });
}

export function saveAv1anAudioPreference(track: AudioTrackDraft, stream: MediaStream): void {
  if (!validAudio([track], [track.streamIndex], [stream])) return;
  const audio: Av1anAudioPreference = {
    codec: track.codec,
    bitrateKbps: track.codec === 'copy' || track.codec === 'flac' ? 128 : track.bitrateKbps!,
    channels: track.codec === 'copy' ? 'preserve' : track.channels,
  };
  if (!safeAudio(audio)) return;
  try {
    if (typeof localStorage === 'undefined') return;
    localStorage.setItem(key, JSON.stringify({ ...readAv1anPreferences(), version: 1, audio }));
  } catch {
    // Audio edits remain in the current draft when browser storage is unavailable.
  }
}

export function saveAv1anAudioState(
  audio: Av1anAudioPreference,
  flags: { stripAudio: boolean; copySubs: boolean; copyData: boolean; copyAttachments: boolean },
  sourceFilters = false,
): string | undefined {
  if (!safeAudio(audio)) return;
  try {
    if (typeof localStorage !== 'undefined')
      localStorage.setItem(
        key,
        JSON.stringify({ ...readAv1anPreferences(sourceFilters), version: 1, audio, ...flags }),
      );
  } catch (cause) {
    return `AV1AN preferences could not be saved: ${cause instanceof Error ? cause.message : String(cause)}`;
  }
}
