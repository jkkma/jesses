import type { EncodeBackend, MediaStream, ToneMapSettings } from '$lib/ipc/generated';

export type ToneMapDraft = {
  enabled: boolean;
  sourcePeakNits: number | undefined;
  hdr10BaseLayer: boolean;
};
export const defaultToneMap = (): ToneMapDraft => ({
  enabled: false,
  sourcePeakNits: 1000,
  hdr10BaseLayer: false,
});
export function selectedToneMap(draft: ToneMapDraft): ToneMapSettings | undefined {
  return draft.enabled
    ? { sourcePeakNits: draft.sourcePeakNits!, hdr10BaseLayer: draft.hdr10BaseLayer }
    : undefined;
}
export function toneMapError(
  draft: ToneMapDraft,
  backend: EncodeBackend,
  video: MediaStream | undefined,
  hdr10Fallback = false,
): string | null {
  if (!draft.enabled) return null;
  if (backend !== 'standalone') return 'HDR-to-SDR tone mapping requires standalone encoding.';
  if (
    typeof draft.sourcePeakNits !== 'number' ||
    !Number.isInteger(draft.sourcePeakNits) ||
    draft.sourcePeakNits < 100 ||
    draft.sourcePeakNits > 10000
  )
    return 'Enter a signal peak from 100 to 10000 nits.';
  if (hdr10Fallback)
    return 'Turn off HDR10 output fallback and use the tone mapping base-layer option when needed.';
  if (
    !video ||
    video.colorPrimaries !== 'bt2020' ||
    video.colorSpace !== 'bt2020nc' ||
    video.colorRange !== 'tv' ||
    video.pixelFormat !== 'yuv420p10le' ||
    !['smpte2084', 'arib-std-b67'].includes(video.colorTransfer ?? '')
  )
    return 'Tone mapping requires tagged limited-range 10-bit 4:2:0 BT.2020 PQ/HDR10 or HLG video.';
  if (video.colorTransfer === 'arib-std-b67' && draft.hdr10BaseLayer)
    return 'HDR10 base-layer fallback does not apply to HLG.';
  return null;
}
export function toneMapSummary(tone: ToneMapSettings | undefined | null): string {
  return tone
    ? ` · SDR BT.709 · Hable ${tone.sourcePeakNits} → 100 nits${tone.hdr10BaseLayer ? ' · HDR10 base layer' : ''}`
    : '';
}
