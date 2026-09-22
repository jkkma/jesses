import type { AudioTrackSettings, MediaStream } from '$lib/ipc/generated';

export type AudioTrackDraft = Omit<AudioTrackSettings, 'bitrateKbps'> & {
  bitrateKbps: number | undefined;
};

export const audioCodecLabels = {
  copy: 'Copy source',
  opus: 'Opus',
  aac: 'AAC',
  flac: 'FLAC (24-bit)',
  mp3: 'MP3',
  vorbis: 'Vorbis',
  eac3: 'E-AC-3',
} as const;

function outputChannels(
  track: AudioTrackDraft,
  stream: MediaStream | undefined,
): number | undefined {
  return track.channels === 'mono'
    ? 1
    : track.channels === 'stereo'
      ? 2
      : track.channels === 'surround51'
        ? 6
        : track.channels === 'surround71'
          ? 8
          : (stream?.channels ?? undefined);
}

export function audioBitrateChoices(
  track: AudioTrackDraft,
  stream: MediaStream | undefined,
): number[] | null {
  if (track.codec !== 'mp3') return null;
  return (stream?.sampleRate ?? 48000) >= 32000
    ? [32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320]
    : [32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
}

export function audioCompatibilityError(
  track: AudioTrackDraft,
  stream: MediaStream | undefined,
): string | null {
  const rate = stream?.sampleRate;
  const channels = outputChannels(track, stream);
  if (track.codec === 'mp3') {
    if (channels && channels > 2) return 'MP3 supports mono or stereo. Choose an explicit downmix.';
    if (rate && ![8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000].includes(rate))
      return 'MP3 cannot retain this source sample rate. Choose another codec.';
  }
  if (track.codec === 'eac3' && rate && ![32000, 44100, 48000].includes(rate))
    return 'E-AC-3 retains 32, 44.1, or 48 kHz sources. Choose another codec for this source.';
  if (track.codec === 'eac3' && track.channels === 'surround71')
    return 'E-AC-3 conversion supports up to 5.1 channels. Choose 5.1 or another codec.';
  if (track.codec === 'vorbis' && rate && (rate < 8000 || rate > 192000))
    return 'Vorbis supports source rates from 8 to 192 kHz in this workflow.';
  if (track.channels === 'preserve' && stream?.channelLayout) {
    const unsupported =
      track.codec === 'opus' || track.codec === 'vorbis'
        ? ['4.0', 'quad(side)', '5.0(side)', '5.1(side)']
        : track.codec === 'flac'
          ? ['4.0', 'quad(side)']
          : track.codec === 'eac3'
            ? ['quad', '5.0', '5.1', '6.1', '7.1']
            : [];
    if (unsupported.includes(stream.channelLayout))
      return `${audioCodecLabels[track.codec]} cannot preserve ${stream.channelLayout}. Choose mono, stereo, or another codec.`;
  }
  return null;
}

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
  const channels = outputChannels(track, stream);
  if (track.codec === 'opus' && channels === 1) return 256;
  if (track.codec === 'mp3') return (stream?.sampleRate ?? 48000) >= 32000 ? 320 : 160;
  if (track.codec === 'eac3') return Math.floor((6144 * (stream?.sampleRate ?? 48000)) / 48000);
  if (track.codec !== 'aac' || !stream?.sampleRate || !channels) return 512;
  return Math.min(512, Math.floor((stream.sampleRate * channels * 6) / 1000));
}

export function validAudio(
  audio: AudioTrackDraft[],
  included: number[],
  streams: MediaStream[],
): boolean {
  return audio.every((track) => {
    if (!included.includes(track.streamIndex)) return true;
    if (track.codec === 'copy') return !track.gain;
    if (
      track.gain &&
      (!Number.isInteger(track.gain.tenthsDb) ||
        track.gain.tenthsDb < -600 ||
        track.gain.tenthsDb > 240)
    )
      return false;
    const stream = streams.find((stream) => stream.index === track.streamIndex);
    if (audioCompatibilityError(track, stream)) return false;
    if (track.codec === 'flac') return true;
    const choices = audioBitrateChoices(track, stream);
    return (
      typeof track.bitrateKbps === 'number' &&
      Number.isInteger(track.bitrateKbps) &&
      track.bitrateKbps >= 32 &&
      track.bitrateKbps <= audioBitrateMax(track, stream) &&
      (!choices || choices.includes(track.bitrateKbps))
    );
  });
}

export function selectedAudio(audio: AudioTrackDraft[], included: number[]): AudioTrackSettings[] {
  return audio
    .filter((track) => included.includes(track.streamIndex))
    .map((track) => ({
      ...track,
      bitrateKbps: track.codec === 'copy' || track.codec === 'flac' ? 128 : track.bitrateKbps!,
      channels: track.codec === 'copy' ? 'preserve' : track.channels,
      ...(track.codec === 'copy' ? { gain: undefined } : {}),
    }));
}

export function audioSummary(audio: AudioTrackDraft[] | undefined): string {
  if (!audio?.length) return 'Audio copied when selected';
  return audio
    .map((track) =>
      track.codec === 'copy'
        ? `Audio #${track.streamIndex} copied`
        : `Audio #${track.streamIndex} → ${audioCodecLabels[track.codec]}${track.codec === 'flac' ? '' : ` ${track.bitrateKbps ?? '—'} kb/s`} · ${track.channels === 'preserve' ? 'source channels' : track.channels === 'surround51' ? '5.1' : track.channels === 'surround71' ? '7.1' : track.channels}${track.gain ? ` · Gain ${(track.gain.tenthsDb / 10).toFixed(1)} dB` : ''}`,
    )
    .join(' · ');
}
