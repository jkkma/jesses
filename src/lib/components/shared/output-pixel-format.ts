import type { Av1anPixelFormat, VideoEncoder } from '$lib/ipc/generated';

export const outputPixelFormats: { value: Av1anPixelFormat; label: string }[] = [
  { value: 'yuv420p', label: '4:2:0 · 8-bit' },
  { value: 'yuv420p10le', label: '4:2:0 · 10-bit' },
  { value: 'yuv422p', label: '4:2:2 · 8-bit' },
  { value: 'yuv422p10le', label: '4:2:2 · 10-bit' },
  { value: 'yuv444p', label: '4:4:4 · 8-bit' },
  { value: 'yuv444p10le', label: '4:4:4 · 10-bit' },
  { value: 'yuva420p', label: '4:2:0 · 8-bit with alpha (VP9)' },
];

export function allowedOutputPixelFormats(encoder: VideoEncoder): Av1anPixelFormat[] {
  if (['svtAv1', 'svtAv1FiveFish', 'svtAv1Hdr', 'h264Nvenc', 'hevcNvenc'].includes(encoder))
    return ['yuv420p', 'yuv420p10le'];
  if (encoder === 'vpxStandalone') return ['yuv420p', 'yuv420p10le', 'yuv444p', 'yuv444p10le'];
  if (encoder === 'vp9') return ['yuv420p', 'yuv420p10le', 'yuv444p', 'yuv444p10le', 'yuva420p'];
  return outputPixelFormats.filter(({ value }) => value !== 'yuva420p').map(({ value }) => value);
}
