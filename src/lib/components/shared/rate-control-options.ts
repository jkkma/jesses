import type { VideoEncoder, VideoRateControl } from '$lib/ipc/generated';

export type RateDraft = {
  mode: 'quality' | 'lossless' | 'bitrate' | 'targetSize' | 'constantQp';
  constantQp: number | undefined;
  bitrate: number | undefined;
  targetSize: number | undefined;
  twoPass: boolean;
};
export const defaultRate = (): RateDraft => ({
  mode: 'quality',
  constantQp: 23,
  bitrate: 1500,
  targetSize: 50,
  twoPass: true,
});
export function validRate(value: RateDraft): boolean {
  if (value.mode === 'quality' || value.mode === 'lossless') return true;
  if (value.mode === 'constantQp')
    return Number.isInteger(value.constantQp) && value.constantQp! >= 0 && value.constantQp! <= 51;
  if (value.mode === 'bitrate')
    return (
      typeof value.bitrate === 'number' &&
      Number.isInteger(value.bitrate) &&
      value.bitrate >= 10 &&
      value.bitrate <= 100000
    );
  return (
    typeof value.targetSize === 'number' &&
    Number.isFinite(value.targetSize) &&
    value.targetSize >= 0 &&
    value.targetSize <= 8192 &&
    Math.abs(value.targetSize * 10 - Math.round(value.targetSize * 10)) < 1e-7
  );
}
export function rateEncoderError(value: RateDraft, encoder: VideoEncoder): string | null {
  return value.mode === 'constantQp'
    ? 'This historical hardware rate-control mode is unsupported in CPU-only Jesses.'
    : null;
}
export function selectedRate(
  value: RateDraft,
  encoder?: VideoEncoder,
): VideoRateControl | undefined {
  if (value.mode === 'quality' || value.mode === 'lossless' || value.mode === 'constantQp')
    return undefined;
  return value.mode === 'bitrate'
    ? { mode: 'bitrate', bitrateKbps: value.bitrate!, twoPass: encoder ? true : value.twoPass }
    : { mode: 'targetSizeDecimal', targetSizeTenthsMb: Math.round(value.targetSize! * 10) };
}
export function rateSummary(
  rate: VideoRateControl | undefined,
  crf: number | undefined,
  lossless = false,
  constantQuantizer?: number | null,
): string {
  if (lossless) return 'Lossless';
  if (constantQuantizer != null) return `Constant QP ${constantQuantizer}`;
  if (!rate) return `CRF ${crf ?? '—'}`;
  return rate.mode === 'bitrate'
    ? `${rate.bitrateKbps} kb/s video · ${rate.twoPass ? 'Two passes' : 'One pass'}`
    : `Target ${rate.mode === 'targetSizeDecimal' ? rate.targetSizeTenthsMb / 10 : rate.targetSizeMib} MiB · Two passes`;
}
