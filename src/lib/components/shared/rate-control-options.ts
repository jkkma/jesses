import type { VideoEncoder, VideoRateControl } from '$lib/ipc/generated';

export type RateDraft = {
  mode: 'quality' | 'lossless' | 'bitrate' | 'targetSize';
  bitrate: number | undefined;
  targetSize: number | undefined;
  twoPass: boolean;
};
export const defaultRate = (): RateDraft => ({
  mode: 'quality',
  bitrate: 2000,
  targetSize: 700,
  twoPass: true,
});
export function validRate(value: RateDraft): boolean {
  if (value.mode === 'quality' || value.mode === 'lossless') return true;
  const scalar = value.mode === 'bitrate' ? value.bitrate : value.targetSize;
  return (
    typeof scalar === 'number' &&
    Number.isInteger(scalar) &&
    scalar >= 1 &&
    scalar <= (value.mode === 'bitrate' ? 100000 : 1048576)
  );
}
export function rateEncoderError(value: RateDraft, encoder: VideoEncoder): string | null {
  if (encoder !== 'h264Nvenc' && encoder !== 'hevcNvenc') return null;
  if (value.mode === 'targetSize')
    return 'NVENC does not support target-size mode in this workflow. Choose quality, lossless, or one-pass bitrate.';
  if (value.mode === 'bitrate' && value.twoPass)
    return 'NVENC bitrate mode is one pass. Turn off Two passes to continue.';
  return null;
}
export function selectedRate(value: RateDraft): VideoRateControl | undefined {
  if (value.mode === 'quality' || value.mode === 'lossless') return undefined;
  return value.mode === 'bitrate'
    ? { mode: 'bitrate', bitrateKbps: value.bitrate!, twoPass: value.twoPass }
    : { mode: 'targetSize', targetSizeMib: value.targetSize! };
}
export function rateSummary(
  rate: VideoRateControl | undefined,
  crf: number | undefined,
  lossless = false,
): string {
  if (lossless) return 'Lossless';
  if (!rate) return `CRF ${crf ?? '—'}`;
  return rate.mode === 'bitrate'
    ? `${rate.bitrateKbps} kb/s video · ${rate.twoPass ? 'Two passes' : 'One pass'}`
    : `Target ${rate.targetSizeMib} MiB · Two passes`;
}
