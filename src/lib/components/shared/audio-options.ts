import type { AudioTrackSettings, MediaStream } from '$lib/ipc/generated';

export type AudioTrackDraft = Omit<AudioTrackSettings, 'bitrateKbps'> & {
  bitrateKbps: number | undefined;
};

export function defaultAudio(streams: MediaStream[]): AudioTrackDraft[] {
  return streams
    .filter((stream) => stream.kind === 'audio')
    .map((stream) => ({
      streamIndex: stream.index,
      codec: 'copy',
      bitrateKbps: 128,
      channels: 'preserve',
    }));
}

export function audioBitrateMax(track: AudioTrackDraft, stream: MediaStream | undefined): number {
  const channels =
    track.channels === 'mono' ? 1 : track.channels === 'stereo' ? 2 : stream?.channels;
  if (track.codec === 'opus' && channels === 1) return 256;
  if (track.codec !== 'aac' || !stream?.sampleRate || !channels) return 512;
  return Math.min(512, Math.floor((stream.sampleRate * channels * 6) / 1000));
}

export function validAudio(
  audio: AudioTrackDraft[],
  included: number[],
  streams: MediaStream[],
): boolean {
  return audio.every(
    (track) =>
      !included.includes(track.streamIndex) ||
      track.codec === 'copy' ||
      (typeof track.bitrateKbps === 'number' &&
        Number.isInteger(track.bitrateKbps) &&
        track.bitrateKbps >= 32 &&
        track.bitrateKbps <=
          audioBitrateMax(
            track,
            streams.find((stream) => stream.index === track.streamIndex),
          )),
  );
}

export function selectedAudio(audio: AudioTrackDraft[], included: number[]): AudioTrackSettings[] {
  return audio
    .filter((track) => included.includes(track.streamIndex))
    .map((track) => ({
      ...track,
      bitrateKbps: track.codec === 'copy' ? 128 : track.bitrateKbps!,
      channels: track.codec === 'copy' ? 'preserve' : track.channels,
    }));
}

export function audioSummary(audio: AudioTrackDraft[] | undefined): string {
  if (!audio?.length) return 'Audio copied when selected';
  return audio
    .map((track) =>
      track.codec === 'copy'
        ? `Audio #${track.streamIndex} copied`
        : `Audio #${track.streamIndex} → ${track.codec === 'opus' ? 'Opus' : 'AAC'} ${track.bitrateKbps ?? '—'} kb/s · ${track.channels === 'preserve' ? 'source channels' : track.channels}`,
    )
    .join(' · ');
}
