import type { EncodeBackend, EncodeSettings, MediaStream, VideoEncoder } from '$lib/ipc/generated';
import { audioSummary } from './audio-options';

export type HdrTune = EncodeSettings['hdrTune'];
export const encoderChoices: { value: VideoEncoder; label: string }[] = [
  { value: 'svtAv1', label: 'SVT-AV1 · AV1' },
  { value: 'svtAv1FiveFish', label: 'SVT-AV1 5fish · Anime' },
  { value: 'svtAv1Hdr', label: 'SVT-AV1-HDR · HDR movies' },
  { value: 'x264', label: 'x264 · H.264' },
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
  if (encoder === 'x264')
    return {
      ...forkDefaults,
      name: 'x264',
      codec: 'H.264',
      tool: 'x264',
      crfMin: 0,
      crfMax: 51,
      defaultCrf: 23,
      defaultPreset: 5,
      suffix: '_x264.mkv',
      av1anSuffix: '_x264.mkv',
      presets: x264Presets.map((label, value) => ({ label, value })),
      presetHelp: 'Ultrafast is quickest; slower presets spend more time compressing.',
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

export function knownHdr(stream: MediaStream | undefined): boolean {
  return (
    !!stream?.hdrFormat ||
    ['smpte2084', 'arib-std-b67'].includes(stream?.colorTransfer ?? '') ||
    !!stream?.dynamicHdrFormats?.length
  );
}

export function encodeSummary(settings: EncodeSettings): string {
  if (settings.encoder === 'x264') {
    return `Standalone x264 · H.264 · Source bit depth · CRF ${settings.crf} · Preset ${presetLabel('x264', settings.preset)} · ${audioSummary(settings.audio)}`;
  }
  const name = encoderOptions(settings.encoder).name;
  return `${settings.backend === 'av1an' ? `av1an / ${name} · ${settings.workers ?? 2} parallel chunks` : `Standalone ${name}`} · 10-bit · CRF ${settings.crf} · Preset ${settings.preset}${forkSettingsSummary(settings)} · Grain ${settings.filmGrain ?? 0} · HDR10 fallback ${settings.hdr10Fallback ? 'allowed' : 'off'} · ${audioSummary(settings.audio)}`;
}
