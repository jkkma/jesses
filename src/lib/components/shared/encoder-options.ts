import type { EncodeBackend, EncodeSettings, MediaStream, VideoEncoder } from '$lib/ipc/generated';

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
  return encoder === 'x264'
    ? {
        name: 'x264',
        codec: 'H.264',
        tool: 'x264',
        crfMin: 0,
        crfMax: 51,
        defaultCrf: 23,
        defaultPreset: 5,
        suffix: '_x264.mkv',
        presets: x264Presets.map((label, value) => ({ label, value })),
        presetHelp: 'Ultrafast is quickest; slower presets spend more time compressing.',
      }
    : {
        name: 'SVT-AV1',
        codec: 'AV1',
        tool: 'svt-av1',
        crfMin: 1,
        crfMax: 63,
        defaultCrf: 30,
        defaultPreset: 4,
        suffix: '_av1.mkv',
        presets: Array.from({ length: 14 }, (_, value) => ({ label: String(value), value })),
        presetHelp: '0–13 · Higher values encode faster',
      };
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
    return `Standalone x264 · H.264 · Source bit depth · CRF ${settings.crf} · Preset ${presetLabel('x264', settings.preset)}`;
  }
  return `${settings.backend === 'av1an' ? `av1an / SVT-AV1 · ${settings.workers ?? 2} parallel chunks` : 'Standalone SVT-AV1'} · 10-bit · CRF ${settings.crf} · Preset ${settings.preset} · Grain ${settings.filmGrain ?? 0} · HDR10 fallback ${settings.hdr10Fallback ? 'allowed' : 'off'}`;
}
