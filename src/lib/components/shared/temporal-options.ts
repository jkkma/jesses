import type {
  EncodeBackend,
  FieldOrder,
  MediaStream,
  QtgmcPreset,
  ResizeFilter,
  TemporalSettings,
} from '$lib/ipc/generated';

export type TemporalDraft = {
  deinterlace:
    'off' | 'frame' | 'bob' | 'qtgmcFrame' | 'qtgmcBob' | 'inverseTelecine' | 'exactDuplicates';
  fieldOrder: FieldOrder;
  qtgmcPreset: QtgmcPreset;
  combedFallback: boolean;
  changeRate: boolean;
  numerator: number | undefined;
  denominator: number | undefined;
  resizeFilter: ResizeFilter;
  aspect: 'off' | 'sample' | 'display';
  aspectNumerator: number | undefined;
  aspectDenominator: number | undefined;
};
export function defaultTemporal(video?: MediaStream): TemporalDraft {
  return {
    deinterlace: 'off',
    fieldOrder: video?.fieldOrder === 'bb' ? 'bottomFirst' : 'topFirst',
    qtgmcPreset: 'fast',
    combedFallback: false,
    changeRate: false,
    numerator: 30000,
    denominator: 1001,
    resizeFilter: 'lanczos',
    aspect: 'off',
    aspectNumerator: 1,
    aspectDenominator: 1,
  };
}
export function temporalError(value: TemporalDraft, _backend: EncodeBackend): string | null {
  if (value.deinterlace === 'exactDuplicates' && !value.changeRate)
    return 'Padded-capture repair requires the intended constant output frame rate.';
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
  if (value.aspect !== 'off') {
    const { aspectNumerator: n, aspectDenominator: d } = value;
    if (
      n === undefined ||
      d === undefined ||
      !Number.isInteger(n) ||
      !Number.isInteger(d) ||
      n < 1 ||
      n > 65535 ||
      d < 1 ||
      d > 65535
    )
      return 'Use positive integer aspect-ratio terms no greater than 65535.';
  }
  return null;
}
export function selectedTemporal(value: TemporalDraft): TemporalSettings | undefined {
  if (
    value.deinterlace === 'off' &&
    !value.changeRate &&
    value.resizeFilter === 'lanczos' &&
    value.aspect === 'off'
  )
    return undefined;
  return {
    ...(['frame', 'bob'].includes(value.deinterlace)
      ? {
          deinterlace: {
            mode: value.deinterlace === 'bob' ? ('bob' as const) : ('frame' as const),
            fieldOrder: value.fieldOrder,
          },
        }
      : {}),
    ...(value.deinterlace === 'qtgmcFrame' || value.deinterlace === 'qtgmcBob'
      ? {
          qtgmc: {
            mode: value.deinterlace === 'qtgmcBob' ? ('bob' as const) : ('frame' as const),
            fieldOrder: value.fieldOrder,
            preset: value.qtgmcPreset,
          },
        }
      : {}),
    ...(value.deinterlace === 'inverseTelecine' || value.deinterlace === 'exactDuplicates'
      ? {
          cadenceRepair: {
            kind:
              value.deinterlace === 'exactDuplicates'
                ? ('exactDuplicates' as const)
                : ('inverseTelecine' as const),
            fieldOrder: value.fieldOrder,
            combedFallback: value.combedFallback,
          },
        }
      : {}),
    ...(value.changeRate
      ? { frameRate: { numerator: value.numerator!, denominator: value.denominator! } }
      : {}),
    resizeFilter: value.resizeFilter,
    ...(value.aspect !== 'off'
      ? {
          aspectRatio: {
            kind: value.aspect,
            numerator: value.aspectNumerator!,
            denominator: value.aspectDenominator!,
          },
        }
      : {}),
  };
}
export function temporalSummary(value: TemporalSettings | undefined): string {
  if (!value) return '';
  const detail = [
    value.deinterlace
      ? `BWDIF ${value.deinterlace.mode === 'bob' ? 'bob' : 'single rate'} (${value.deinterlace.fieldOrder === 'topFirst' ? 'TFF' : 'BFF'})`
      : '',
    value.qtgmc
      ? `QTGMC ${value.qtgmc.mode === 'bob' ? 'bob' : 'single rate'} ${value.qtgmc.preset} (${value.qtgmc.fieldOrder === 'topFirst' ? 'TFF' : 'BFF'})`
      : '',
    value.cadenceRepair
      ? value.cadenceRepair.kind === 'exactDuplicates'
        ? 'guarded exact-duplicate cadence repair'
        : `inverse telecine (${value.cadenceRepair.fieldOrder === 'topFirst' ? 'TFF' : 'BFF'}${value.cadenceRepair.combedFallback ? ', combed fallback' : ''})`
      : '',
    value.frameRate
      ? `${value.frameRate.numerator}/${value.frameRate.denominator} fps, duplicate/drop`
      : '',
    value.resizeFilter !== 'lanczos' ? `${value.resizeFilter} resize` : '',
    value.aspectRatio
      ? `${value.aspectRatio.kind === 'sample' ? 'SAR' : 'DAR'} ${value.aspectRatio.numerator}:${value.aspectRatio.denominator}`
      : '',
  ]
    .filter(Boolean)
    .join(' · ');
  return detail ? ` · ${detail}` : '';
}
