import { parameterSummary } from './encoder-parameters';
import { av1anSummary } from './av1an-options';
import { temporalSummary } from './temporal-options';
import { rateSummary } from './rate-control-options';
import { subtitleSummary } from './subtitle-options';
import { toneMapSummary } from './tone-map-options';
export { knownHdr } from './media-color';
import type { EncodeBackend, EncodeSettings, MediaStream, VideoEncoder } from '$lib/ipc/generated';
import { audioSummary } from './audio-options';
import { framingSummary } from './framing-options';
import { trimSummary } from './trim-options';

export type HdrTune = EncodeSettings['hdrTune'];
export const encoderChoices: { value: VideoEncoder; label: string }[] = [
  { value: 'svtAv1Hdr', label: 'SVT-AV1-HDR · HDR movies' },
  { value: 'svtAv1FiveFish', label: 'SVT-AV1 5fish · Anime' },
  { value: 'svtAv1', label: 'SVT-AV1 · Standard' },
  { value: 'x264', label: 'x264 · H.264' },
  { value: 'x265', label: 'x265 · HEVC' },
  { value: 'vp9', label: 'VP9 · libvpx' },
];

export function isSvtEncoder(encoder: VideoEncoder): boolean {
  return encoder === 'svtAv1' || encoder === 'svtAv1FiveFish' || encoder === 'svtAv1Hdr';
}

const x264Presets = [
  'ultrafast',
  'superfast',
  'veryfast',
  'faster',
  'fast',
  'medium',
  'slow',
  'slower',
  'veryslow',
  'placebo',
];

export function encoderOptions(encoder: VideoEncoder) {
  const forkDefaults = {
    defaultLineartPsyBias: 0,
    defaultTexturePsyBias: 0,
    defaultHdrTune: 'visualQuality' as HdrTune,
  };
  if (encoder === 'x264' || encoder === 'x265')
    return {
      ...forkDefaults,
      name: encoder,
      codec: encoder === 'x264' ? 'H.264' : 'HEVC',
      tool: encoder === 'x264' ? 'x264' : 'ffmpeg',
      crfMin: 0,
      crfMax: 51,
      defaultCrf: encoder === 'x264' ? 23 : 28,
      defaultPreset: 5,
      suffix: `_${encoder}.mkv`,
      av1anSuffix: `_${encoder}.mkv`,
      presets: x264Presets.map((label, value) => ({ label, value })),
      presetHelp: 'Ultrafast is quickest; slower presets spend more time compressing.',
    };
  if (encoder === 'vp9')
    return {
      ...forkDefaults,
      name: 'VP9',
      codec: 'VP9',
      tool: 'ffmpeg',
      crfMin: 0,
      crfMax: 63,
      defaultCrf: 32,
      defaultPreset: 2,
      suffix: '_vp9.mkv',
      av1anSuffix: '_vp9.mkv',
      presets: Array.from({ length: 6 }, (_, value) => ({ label: String(value), value })),
      presetHelp: 'Speed 0–5 · Higher values encode faster. Constant quality, good deadline.',
    };
  const standard = {
    ...forkDefaults,
    name: 'SVT-AV1',
    codec: 'AV1',
    tool: 'svt-av1',
    crfMin: 1,
    crfMax: 63,
    defaultCrf: 30,
    defaultPreset: 4,
    suffix: '_av1.mkv',
    av1anSuffix: '_av1an.mkv',
    presets: Array.from({ length: 14 }, (_, value) => ({ label: String(value), value })),
    presetHelp: '0–13 · Higher values encode faster',
  };
  if (encoder === 'svtAv1FiveFish')
    return {
      ...standard,
      name: 'SVT-AV1 5fish',
      tool: 'svt-av1-5fish',
      defaultCrf: 18,
      defaultPreset: 2,
      defaultLineartPsyBias: 5,
      defaultTexturePsyBias: 4,
      suffix: '_av1_5fish.mkv',
      av1anSuffix: '_av1an_5fish.mkv',
    };
  if (encoder === 'svtAv1Hdr')
    return {
      ...standard,
      name: 'SVT-AV1-HDR',
      tool: 'svt-av1-hdr',
      defaultPreset: 2,
      defaultHdrTune: 'filmGrain' as HdrTune,
      suffix: '_av1_hdr.mkv',
      av1anSuffix: '_av1an_hdr.mkv',
    };
  return standard;
}

export function validForkSettings(
  encoder: VideoEncoder,
  lineart: number | undefined,
  texture: number | undefined,
  hdrTune: HdrTune,
): boolean {
  return (
    (encoder !== 'svtAv1FiveFish' ||
      [lineart, texture].every(
        (value) => typeof value === 'number' && Number.isInteger(value) && value >= 0 && value <= 7,
      )) &&
    (encoder !== 'svtAv1Hdr' || hdrTune === 'visualQuality' || hdrTune === 'filmGrain')
  );
}

export function forkSettingsSummary(
  settings: Pick<EncodeSettings, 'encoder' | 'hdrTune'> & {
    lineartPsyBias: number | undefined;
    texturePsyBias: number | undefined;
  },
): string {
  if (settings.encoder === 'svtAv1FiveFish')
    return ` · Lineart ${settings.lineartPsyBias ?? '—'} · Texture ${settings.texturePsyBias ?? '—'}`;
  if (settings.encoder === 'svtAv1Hdr')
    return ` · HDR tune ${settings.hdrTune === 'filmGrain' ? 'film grain' : 'visual quality'}`;
  return '';
}

export function requiredEncoderTools(backend: EncodeBackend, encoder: VideoEncoder): string[] {
  return [
    'ffmpeg',
    'ffprobe',
    encoderOptions(encoder).tool,
    ...(backend === 'av1an' ? ['av1an'] : []),
  ];
}

export function presetLabel(encoder: VideoEncoder, preset: number): string {
  return encoderOptions(encoder).presets[preset]?.label ?? String(preset);
}

export function sourceBitDepth(stream: MediaStream | undefined): number | null {
  if (stream?.bitDepth === 8 || stream?.bitDepth === 10) return stream.bitDepth;
  if (stream?.pixelFormat === 'yuv420p' || stream?.pixelFormat === 'yuvj420p') return 8;
  if (stream?.pixelFormat === 'yuv420p10le') return 10;
  return null;
}

export function encodeSummary(settings: EncodeSettings): string {
  if (!isSvtEncoder(settings.encoder)) {
    const options = encoderOptions(settings.encoder);
    return `${settings.encoder === 'x264' ? 'Standalone' : 'FFmpeg'} ${options.name} · ${options.codec} · Source bit depth · ${settings.av1anOptions?.targetQuality ? 'Perceptual quality target' : rateSummary(settings.rateControl, settings.crf)} · Preset ${presetLabel(settings.encoder, settings.preset)} · ${framingSummary(settings.framing)} · ${audioSummary(settings.audio)}${trimSummary(settings.trim)}${subtitleSummary(settings.subtitles) ? ` · ${subtitleSummary(settings.subtitles)}` : ''}${toneMapSummary(settings.toneMap)}${temporalSummary(settings.temporal)}${av1anSummary(settings.av1anOptions)}${parameterSummary(settings.parameters)}`;
  }
  const name = encoderOptions(settings.encoder).name;
  return `${settings.backend === 'av1an' ? `av1an / ${name} · ${settings.workers ?? 2} parallel chunks` : `Standalone ${name}`} · 10-bit · ${settings.av1anOptions?.targetQuality ? 'Perceptual quality target' : rateSummary(settings.rateControl, settings.crf)} · Preset ${settings.preset}${forkSettingsSummary(settings)} · Grain ${settings.filmGrain ?? 0} · HDR10 fallback ${settings.hdr10Fallback ? 'allowed' : 'off'} · ${framingSummary(settings.framing)} · ${audioSummary(settings.audio)}${trimSummary(settings.trim)}${subtitleSummary(settings.subtitles) ? ` · ${subtitleSummary(settings.subtitles)}` : ''}${toneMapSummary(settings.toneMap)}${temporalSummary(settings.temporal)}${av1anSummary(settings.av1anOptions)}${parameterSummary(settings.parameters)}`;
}
