import type {
  EncodeBackend,
  FieldOrder,
  MediaStream,
  ResizeFilter,
  TemporalSettings,
} from '$lib/ipc/generated';

export type TemporalDraft = {
  deinterlace: 'off' | 'frame' | 'bob';
  fieldOrder: FieldOrder;
  changeRate: boolean;
  numerator: number | undefined;
  denominator: number | undefined;
  resizeFilter: ResizeFilter;
};
export function defaultTemporal(video?: MediaStream): TemporalDraft {
  return {
    deinterlace: 'off',
    fieldOrder: video?.fieldOrder === 'bb' ? 'bottomFirst' : 'topFirst',
    changeRate: false,
    numerator: 30000,
    denominator: 1001,
    resizeFilter: 'lanczos',
  };
}
export function temporalError(value: TemporalDraft, backend: EncodeBackend): string | null {
  if (backend === 'av1an' && (value.deinterlace !== 'off' || value.changeRate))
    return 'Deinterlacing and frame-rate conversion require standalone encoding.';
  if (value.changeRate) {
    const { numerator: n, denominator: d } = value;
    if (
      n === undefined ||
      d === undefined ||
      !Number.isInteger(n) ||
      !Number.isInteger(d) ||
      n < 1 ||
      n > 12000000 ||
      d < 1 ||
      d > 100000 ||
      n < d ||
      n > 120 * d
    )
      return 'Use an integer numerator and denominator describing 1–120 output frames per second.';
  }
  return null;
}
export function selectedTemporal(value: TemporalDraft): TemporalSettings | undefined {
  if (value.deinterlace === 'off' && !value.changeRate && value.resizeFilter === 'lanczos')
    return undefined;
  return {
    ...(value.deinterlace !== 'off'
      ? { deinterlace: { mode: value.deinterlace, fieldOrder: value.fieldOrder } }
      : {}),
    ...(value.changeRate
      ? { frameRate: { numerator: value.numerator!, denominator: value.denominator! } }
      : {}),
    resizeFilter: value.resizeFilter,
  };
}
export function temporalSummary(value: TemporalSettings | undefined): string {
  if (!value) return '';
  const detail = [
    value.deinterlace
      ? `BWDIF ${value.deinterlace.mode === 'bob' ? 'bob' : 'single rate'} (${value.deinterlace.fieldOrder === 'topFirst' ? 'TFF' : 'BFF'})`
      : '',
    value.frameRate
      ? `${value.frameRate.numerator}/${value.frameRate.denominator} fps, duplicate/drop`
      : '',
    value.resizeFilter !== 'lanczos' ? `${value.resizeFilter} resize` : '',
  ]
    .filter(Boolean)
    .join(' · ');
  return detail ? ` · ${detail}` : '';
}
